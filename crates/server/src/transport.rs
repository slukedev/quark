//! How stanzas travel: XML streams over TCP (with STARTTLS or direct TLS) here, WebSocket in
//! `http::websocket`. Stream negotiation and sessions only see these traits.

use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use quark_core::xml::{stream_header, StreamEvent, StreamReader, XmlError};
use quark_core::{ns, Element};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufWriter, ReadBuf, ReadHalf, WriteHalf};
use tokio::net::TcpStream;

use crate::sasl::ChannelBindings;
use crate::tls::CertStore;

/// A `<stream:error/>` (RFC 6120 §4.9.3).
#[derive(Debug, Clone)]
pub struct StreamError {
    pub condition: &'static str,
    pub text: Option<String>,
    /// An application-specific condition element.
    pub app: Option<Box<Element>>,
}

impl StreamError {
    pub fn new(condition: &'static str) -> Self {
        StreamError { condition, text: None, app: None }
    }
    pub fn with_text(condition: &'static str, text: impl Into<String>) -> Self {
        StreamError { condition, text: Some(text.into()), app: None }
    }
    pub fn to_element(&self) -> Element {
        let mut e = Element::new("error", ns::STREAM).child(Element::new(self.condition, ns::STREAMS));
        if let Some(t) = &self.text {
            e.append(Element::new("text", ns::STREAMS).attr_set("xml:lang", "en").text(t.clone()));
        }
        if let Some(a) = &self.app {
            e.append((**a).clone());
        }
        e
    }
}

/// The receiving half of an established stream.
#[async_trait]
pub trait StanzaReader: Send {
    async fn read(&mut self) -> Result<Option<StreamEvent>, XmlError>;
    fn bytes_read(&self) -> u64;
}

/// The sending half. `write` may buffer; `flush` sends.
#[async_trait]
pub trait StanzaWriter: Send {
    /// Bytes the connection has taken so far, when the transport can tell: a slow link that
    /// keeps moving is then told from a stuck one (see `c2s::session::TimedWriter`).
    fn progress(&self) -> Option<Arc<AtomicU64>> {
        None
    }
    async fn write(&mut self, el: &Element) -> io::Result<()>;
    async fn flush(&mut self) -> io::Result<()>;
    /// Sends a keepalive the client ignores (whitespace on TCP).
    async fn keepalive(&mut self) -> io::Result<()>;
    /// Ends the stream, with an error first when given.
    async fn close(&mut self, err: Option<&StreamError>);
}

/// A stream during negotiation, before it splits into a reader and a writer.
#[async_trait]
pub trait Transport: Send {
    async fn read(&mut self) -> Result<Option<StreamEvent>, XmlError>;
    /// Sends and flushes.
    async fn send(&mut self, el: &Element) -> io::Result<()>;
    /// Sends our stream header (or WebSocket `<open/>`).
    async fn open(&mut self, from: &str, id: &str, to: Option<&str>) -> io::Result<()>;
    async fn close(&mut self, err: Option<&StreamError>);
    /// The client will open a new stream on the same connection (after SASL).
    fn restart(&mut self);
    fn secure(&self) -> bool;
    fn can_starttls(&self) -> bool;
    async fn starttls(&mut self) -> io::Result<()>;
    fn bindings(&self) -> &ChannelBindings;
    fn sni(&self) -> Option<&str>;
    fn peer(&self) -> Option<IpAddr>;
    fn kind(&self) -> &'static str;
    fn split(self: Box<Self>) -> (Box<dyn StanzaReader>, Box<dyn StanzaWriter>);
}

/// A connection that counts the bytes written to it (taken by the socket, not just buffered).
pub struct Counted<T> {
    inner: T,
    written: Arc<AtomicU64>,
}

impl<T> Counted<T> {
    pub fn new(inner: T, written: Arc<AtomicU64>) -> Self {
        Counted { inner, written }
    }
    pub fn into_inner(self) -> T {
        self.inner
    }
    fn count(&self, r: &Poll<io::Result<usize>>) {
        if let Poll::Ready(Ok(n)) = r {
            self.written.fetch_add(*n as u64, Ordering::Relaxed);
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Counted<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Counted<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        let r = Pin::new(&mut me.inner).poll_write(cx, buf);
        me.count(&r);
        r
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        let r = Pin::new(&mut me.inner).poll_write_vectored(cx, bufs);
        me.count(&r);
        r
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

type TcpOut = BufWriter<Counted<WriteHalf<Io>>>;

fn tcp_out(w: WriteHalf<Io>, written: &Arc<AtomicU64>) -> TcpOut {
    BufWriter::with_capacity(8 * 1024, Counted::new(w, written.clone()))
}

/// TCP, optionally upgraded to TLS.
pub enum Io {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl AsyncRead for Io {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Io {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_flush(cx),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
        }
    }
    fn is_write_vectored(&self) -> bool {
        match self {
            Io::Plain(s) => s.is_write_vectored(),
            Io::Tls(s) => s.is_write_vectored(),
        }
    }
}

/// An XML stream over TCP: client-to-server or server-to-server.
pub struct TcpTransport {
    reader: Option<StreamReader<ReadHalf<Io>>>,
    writer: Option<TcpOut>,
    /// Bytes the socket has taken, for the session's write timeout.
    written: Arc<AtomicU64>,
    peer: Option<IpAddr>,
    secure: bool,
    cb: ChannelBindings,
    sni: Option<String>,
    /// jabber:client or jabber:server.
    stream_ns: &'static str,
    max_stanza: u64,
    certs: Arc<CertStore>,
    tls_config: Option<Arc<rustls::ServerConfig>>,
}

impl TcpTransport {
    /// A plain connection that can upgrade with STARTTLS using `tls_config`.
    pub fn plain(
        tcp: TcpStream,
        stream_ns: &'static str,
        max_stanza: u64,
        certs: Arc<CertStore>,
        tls_config: Arc<rustls::ServerConfig>,
    ) -> Self {
        let peer = tcp.peer_addr().ok().map(|a| a.ip().to_canonical());
        tune_socket(&tcp);
        let (r, w) = tokio::io::split(Io::Plain(tcp));
        let written = Arc::default();
        let mut reader = StreamReader::new(r, max_stanza);
        reader.server_ns = stream_ns == ns::SERVER;
        TcpTransport {
            reader: Some(reader),
            writer: Some(tcp_out(w, &written)),
            written,
            peer,
            secure: false,
            cb: ChannelBindings::default(),
            sni: None,
            stream_ns,
            max_stanza,
            certs,
            tls_config: Some(tls_config),
        }
    }

    /// A connection that already did its TLS handshake (direct TLS, XEP-0368).
    pub fn tls(tls: tokio_rustls::server::TlsStream<TcpStream>, stream_ns: &'static str, max_stanza: u64, certs: Arc<CertStore>) -> Self {
        let peer = tls.get_ref().0.peer_addr().ok().map(|a| a.ip().to_canonical());
        tune_socket(tls.get_ref().0);
        let (sni, _, cb) = crate::tls::session_facts(&tls, &certs);
        let (r, w) = tokio::io::split(Io::Tls(Box::new(tls)));
        let written = Arc::default();
        let mut reader = StreamReader::new(r, max_stanza);
        reader.server_ns = stream_ns == ns::SERVER;
        TcpTransport {
            reader: Some(reader),
            writer: Some(tcp_out(w, &written)),
            written,
            peer,
            secure: true,
            cb,
            sni,
            stream_ns,
            max_stanza,
            certs,
            tls_config: None,
        }
    }

    fn writer(&mut self) -> io::Result<&mut TcpOut> {
        self.writer.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "stream closed"))
    }
}

/// TCP keepalive finds dead peers (phones that vanished) even when nothing is being sent, and
/// Nagle is off: stanzas are small and latency matters more than packet count.
fn tune_socket(tcp: &TcpStream) {
    let _ = tcp.set_nodelay(true);
    let sock = socket2::SockRef::from(tcp);
    let ka = socket2::TcpKeepalive::new().with_time(std::time::Duration::from_secs(120)).with_interval(std::time::Duration::from_secs(30));
    let _ = sock.set_tcp_keepalive(&ka);
}

#[async_trait]
impl Transport for TcpTransport {
    async fn read(&mut self) -> Result<Option<StreamEvent>, XmlError> {
        match self.reader.as_mut() {
            Some(r) => r.next().await,
            None => Ok(None),
        }
    }

    async fn send(&mut self, el: &Element) -> io::Result<()> {
        let xml = el.to_stream_xml(self.stream_ns);
        let w = self.writer()?;
        w.write_all(xml.as_bytes()).await?;
        w.flush().await
    }

    async fn open(&mut self, from: &str, id: &str, to: Option<&str>) -> io::Result<()> {
        let extra: &[(&str, &str)] = if self.stream_ns == ns::SERVER { &[("xmlns:db", ns::DIALBACK)] } else { &[] };
        let h = stream_header(self.stream_ns, Some(from), to, Some(id), extra);
        let w = self.writer()?;
        w.write_all(h.as_bytes()).await?;
        w.flush().await
    }

    async fn close(&mut self, err: Option<&StreamError>) {
        let ns = self.stream_ns;
        let Some(mut w) = self.writer.take() else { return };
        // During negotiation the stream only ends without an error when the client closed it
        // (or the connection is gone, which the linger notices at once).
        let peer_closed = if err.is_none() { self.reader.take().map(StreamReader::into_inner) } else { None };
        if tokio::time::timeout(Duration::from_secs(5), write_close(&mut w, err, ns)).await.is_ok_and(|r| r.is_ok()) {
            linger(w, peer_closed);
        }
    }

    fn restart(&mut self) {
        self.reader = self.reader.take().map(StreamReader::restart);
    }

    fn secure(&self) -> bool {
        self.secure
    }

    fn can_starttls(&self) -> bool {
        !self.secure && self.tls_config.is_some()
    }

    async fn starttls(&mut self) -> io::Result<()> {
        let cfg = self.tls_config.take().ok_or_else(|| io::Error::other("TLS isn't available"))?;
        let (Some(reader), Some(writer)) = (self.reader.take(), self.writer.take()) else {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "stream closed"));
        };
        if reader.has_buffered() {
            // Anything sent before the handshake would be injected into the encrypted stream.
            return Err(io::Error::new(io::ErrorKind::InvalidData, "data after <starttls/>"));
        }
        let tcp = match reader.into_inner().unsplit(writer.into_inner().into_inner()) {
            Io::Plain(tcp) => tcp,
            Io::Tls(_) => return Err(io::Error::other("already encrypted")),
        };
        // Some clients (go-xmpp) end <starttls/> with a newline, against RFC 6120 §5.3.3; it can
        // also arrive after we've read. A TLS ClientHello never starts with whitespace.
        skip_whitespace(&tcp).await?;
        let tls = tokio_rustls::TlsAcceptor::from(cfg).accept(tcp).await?;
        let (sni, _, cb) = crate::tls::session_facts(&tls, &self.certs);
        self.sni = sni;
        self.cb = cb;
        self.secure = true;
        let (r, w) = tokio::io::split(Io::Tls(Box::new(tls)));
        let mut reader = StreamReader::new(r, self.max_stanza);
        reader.server_ns = self.stream_ns == ns::SERVER;
        self.reader = Some(reader);
        self.writer = Some(tcp_out(w, &self.written));
        Ok(())
    }

    fn bindings(&self) -> &ChannelBindings {
        &self.cb
    }

    fn sni(&self) -> Option<&str> {
        self.sni.as_deref()
    }

    fn peer(&self) -> Option<IpAddr> {
        self.peer
    }

    fn kind(&self) -> &'static str {
        "tcp"
    }

    fn split(self: Box<Self>) -> (Box<dyn StanzaReader>, Box<dyn StanzaWriter>) {
        let me = *self;
        let peer_closed = PeerClosed::default();
        let reader = me
            .reader
            .map(|r| Box::new(TcpReader { r: Some(r), read: 0, peer_closed: peer_closed.clone() }) as Box<dyn StanzaReader>)
            .unwrap_or_else(|| Box::new(Closed));
        let writer = me
            .writer
            .map(|w| Box::new(TcpWriter { w: Some(w), stream_ns: me.stream_ns, written: me.written, peer_closed }) as Box<dyn StanzaWriter>)
            .unwrap_or_else(|| Box::new(Closed));
        (reader, writer)
    }
}

/// Where the reading half goes once the client has closed its stream, so the writer can wait for
/// the client to hang up before closing the connection.
type PeerClosed = Arc<Mutex<Option<ReadHalf<Io>>>>;

/// How long we wait for a client that closed its stream to close the connection.
const LINGER_PEER_CLOSE: Duration = Duration::from_secs(2);
/// How long our closing tag (or stream error) gets to arrive before TLS's own close does.
const LINGER_OURS: Duration = Duration::from_millis(100);

/// Our stream error, if any, and closing tag, flushed.
async fn write_close(w: &mut TcpOut, err: Option<&StreamError>, stream_ns: &str) -> io::Result<()> {
    let mut s = String::new();
    if let Some(e) = err {
        e.to_element().write_into_stream(&mut s, stream_ns);
    }
    s.push_str("</stream:stream>");
    w.write_all(s.as_bytes()).await?;
    w.flush().await
}

/// Closes the connection once the closing tag has had its chance to be read, in a task of its
/// own so nothing waits for it. Some TLS stacks (Smack's) drop text that arrives in the same
/// read as the TLS close, so after answering a client's `</stream:stream>` we let the client
/// hang up first (RFC 6120 §4.4), and after closing the stream ourselves we pause briefly.
/// Both waits are short and bounded, whatever the peer does.
fn linger(mut w: TcpOut, peer_closed: Option<ReadHalf<Io>>) {
    tokio::spawn(async move {
        match peer_closed {
            Some(mut r) => {
                let mut buf = [0u8; 1024];
                let _ = tokio::time::timeout(LINGER_PEER_CLOSE, async { while matches!(r.read(&mut buf).await, Ok(n) if n > 0) {} }).await;
            }
            None => tokio::time::sleep(LINGER_OURS).await,
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), w.shutdown()).await;
    });
}

struct TcpReader {
    /// None once the client closed its stream (the half is then in `peer_closed`).
    r: Option<StreamReader<ReadHalf<Io>>>,
    read: u64,
    peer_closed: PeerClosed,
}

#[async_trait]
impl StanzaReader for TcpReader {
    async fn read(&mut self) -> Result<Option<StreamEvent>, XmlError> {
        let Some(r) = self.r.as_mut() else { return Ok(None) };
        let ev = r.next().await;
        if matches!(ev, Ok(Some(StreamEvent::Close))) {
            if let Some(r) = self.r.take() {
                self.read = r.bytes_read();
                *self.peer_closed.lock() = Some(r.into_inner());
            }
        }
        ev
    }
    fn bytes_read(&self) -> u64 {
        self.r.as_ref().map_or(self.read, |r| r.bytes_read())
    }
}

pub struct TcpWriter {
    /// None once closed.
    w: Option<TcpOut>,
    stream_ns: &'static str,
    written: Arc<AtomicU64>,
    peer_closed: PeerClosed,
}

impl TcpWriter {
    fn w(&mut self) -> io::Result<&mut TcpOut> {
        self.w.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "stream closed"))
    }
}

#[async_trait]
impl StanzaWriter for TcpWriter {
    fn progress(&self) -> Option<Arc<AtomicU64>> {
        Some(self.written.clone())
    }
    async fn write(&mut self, el: &Element) -> io::Result<()> {
        let xml = el.to_stream_xml(self.stream_ns);
        self.w()?.write_all(xml.as_bytes()).await
    }
    async fn flush(&mut self) -> io::Result<()> {
        match self.w.as_mut() {
            Some(w) => w.flush().await,
            None => Ok(()),
        }
    }
    async fn keepalive(&mut self) -> io::Result<()> {
        let w = self.w()?;
        w.write_all(b" ").await?;
        w.flush().await
    }
    async fn close(&mut self, err: Option<&StreamError>) {
        let ns = self.stream_ns;
        let Some(mut w) = self.w.take() else { return };
        // Only an answer to the client's own closing tag waits for the client to hang up.
        let peer_closed = if err.is_none() { self.peer_closed.lock().take() } else { None };
        if tokio::time::timeout(Duration::from_secs(5), write_close(&mut w, err, ns)).await.is_ok_and(|r| r.is_ok()) {
            linger(w, peer_closed);
        }
    }
}

/// A stream that's already gone.
struct Closed;

#[async_trait]
impl StanzaReader for Closed {
    async fn read(&mut self) -> Result<Option<StreamEvent>, XmlError> {
        Ok(None)
    }
    fn bytes_read(&self) -> u64 {
        0
    }
}

#[async_trait]
impl StanzaWriter for Closed {
    async fn write(&mut self, _el: &Element) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::NotConnected, "stream closed"))
    }
    async fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
    async fn keepalive(&mut self) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::NotConnected, "stream closed"))
    }
    async fn close(&mut self, _err: Option<&StreamError>) {}
}

/// Reads away whitespace waiting at the start of the socket, without touching anything else.
async fn skip_whitespace(tcp: &tokio::net::TcpStream) -> io::Result<()> {
    let mut buf = [0u8; 64];
    loop {
        let n = match tokio::time::timeout(std::time::Duration::from_millis(50), tcp.peek(&mut buf)).await {
            Ok(r) => r?,
            Err(_) => return Ok(()),
        };
        let ws = buf[..n].iter().take_while(|b| b.is_ascii_whitespace()).count();
        if ws == 0 {
            return Ok(());
        }
        let mut sink = vec![0u8; ws];
        match tcp.try_read(&mut sink) {
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => return Err(e),
            _ => {}
        }
    }
}
