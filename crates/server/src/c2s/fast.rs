//! FAST tokens (XEP-0484): after one full sign-in, a device reconnects with a single round trip
//! and no password hashing. Tokens are per device, expire, and rotate.

use base64::Engine;
use rand::RngCore;

use crate::db::{params, Db, Migration};

pub static MIGRATION: Migration = Migration {
    module: "fast",
    steps: &[
        "CREATE TABLE fast_tokens (
        host TEXT NOT NULL,
        user TEXT NOT NULL,
        agent TEXT NOT NULL,
        mechanism TEXT NOT NULL,
        token BLOB NOT NULL,
        expires INTEGER NOT NULL,
        previous BLOB,
        previous_expires INTEGER,
        PRIMARY KEY (host, user, agent),
        FOREIGN KEY (host, user) REFERENCES accounts(host, user) ON DELETE CASCADE
    ) WITHOUT ROWID",
        // XEP-0484 §5.1's two slots: `token` is the newest ("new"), `previous` the one in use
        // ("current"), each bound to its own mechanism; `used` says the newest has been used.
        "ALTER TABLE fast_tokens ADD COLUMN previous_mechanism TEXT;
         ALTER TABLE fast_tokens ADD COLUMN used INTEGER NOT NULL DEFAULT 0;
         UPDATE fast_tokens SET previous_mechanism = mechanism WHERE previous IS NOT NULL;",
    ],
};

/// How long a token stays valid.
pub const LIFETIME_MS: i64 = 30 * 86_400_000;

#[derive(Clone)]
pub struct FastTokens {
    db: Db,
}

impl FastTokens {
    pub fn new(db: Db) -> Self {
        FastTokens { db }
    }

    /// Issues a new token for a device. The token it has been using keeps working until the new
    /// one is used (in case the device never got it); a newer one it never used is replaced.
    /// Returns (token, expiry).
    pub async fn issue(&self, host: &str, user: &str, agent: &str, mechanism: &str) -> Result<(String, i64), crate::db::DbError> {
        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
        let now = quark_core::time::now_ms();
        let expires = now + LIFETIME_MS;
        let (h, u, a, m, t) = (host.to_string(), user.to_string(), agent.to_string(), mechanism.to_string(), token.clone());
        self.db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO fast_tokens (host, user, agent, mechanism, token, expires) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(host, user, agent) DO UPDATE SET
                        previous = CASE WHEN used THEN token ELSE previous END,
                        previous_expires = CASE WHEN used THEN expires ELSE previous_expires END,
                        previous_mechanism = CASE WHEN used THEN mechanism ELSE previous_mechanism END,
                        token = excluded.token, expires = excluded.expires, mechanism = excluded.mechanism, used = 0",
                    params![h, u, a, m, t.as_bytes(), expires],
                )
            })
            .await?;
        Ok((token, expires))
    }

    /// Forgets the token a device just signed in with (`<fast invalidate='true'/>`), whichever
    /// slot it's in.
    pub async fn revoke(&self, host: &str, user: &str, agent: &str, secret: &[u8]) -> Result<(), crate::db::DbError> {
        let (h, u, a, t) = (host.to_string(), user.to_string(), agent.to_string(), secret.to_vec());
        self.db
            .write(move |tx| {
                tx.execute(
                    "UPDATE fast_tokens SET previous = NULL, previous_expires = NULL, previous_mechanism = NULL
                     WHERE host = ?1 AND user = ?2 AND agent = ?3 AND previous = ?4",
                    params![h, u, a, t],
                )?;
                tx.execute("DELETE FROM fast_tokens WHERE host = ?1 AND user = ?2 AND agent = ?3 AND token = ?4", params![h, u, a, t])
            })
            .await
            .map(|_| ())
    }

    /// Forgets every token of an account (password change, account turned off).
    pub async fn revoke_all(&self, host: &str, user: &str) -> Result<(), crate::db::DbError> {
        let (h, u) = (host.to_string(), user.to_string());
        self.db.write(move |tx| tx.execute("DELETE FROM fast_tokens WHERE host = ?1 AND user = ?2", params![h, u])).await.map(|_| ())
    }
}

#[async_trait::async_trait]
impl crate::sasl::TokenCheck for FastTokens {
    async fn tokens(&self, host: &str, user: &str, mechanism: &str) -> Vec<crate::sasl::Token> {
        let (h, u, m) = (host.to_string(), user.to_string(), mechanism.to_string());
        let now = quark_core::time::now_ms();
        self.db
            .read(move |c| {
                let mut st = c.prepare_cached(
                    "SELECT token, expires, mechanism, previous, previous_expires, previous_mechanism FROM fast_tokens
                     WHERE host = ?1 AND user = ?2",
                )?;
                let rows = st.query_map(params![h, u], |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<Vec<u8>>>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                })?;
                // Newest first: XEP-0484 §5.1 checks the "new" slot before the "current" one.
                let mut newest = Vec::new();
                let mut older = Vec::new();
                for row in rows {
                    let (t, e, tm, p, pe, pm) = row?;
                    if e > now && tm == m {
                        newest.push(crate::sasl::Token { secret: t, newest: true });
                    }
                    if let (Some(p), Some(pe), Some(pm)) = (p, pe, pm) {
                        if pe > now && pm == m {
                            older.push(crate::sasl::Token { secret: p, newest: false });
                        }
                    }
                }
                newest.append(&mut older);
                Ok(newest)
            })
            .await
            .unwrap_or_default()
    }

    /// The newest token was used: every earlier one of that device stops working (§3.5).
    async fn used(&self, host: &str, user: &str, secret: &[u8]) {
        let (h, u, t) = (host.to_string(), user.to_string(), secret.to_vec());
        let r = self
            .db
            .write(move |tx| {
                tx.execute(
                    "UPDATE fast_tokens SET used = 1, previous = NULL, previous_expires = NULL, previous_mechanism = NULL
                     WHERE host = ?1 AND user = ?2 AND token = ?3",
                    params![h, u, t],
                )
            })
            .await;
        if let Err(e) = r {
            tracing::warn!(error = %e, "couldn't retire older FAST tokens");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sasl::TokenCheck;

    async fn store() -> FastTokens {
        let db = Db::memory().unwrap();
        db.migrate(&[&crate::accounts::MIGRATION, &MIGRATION]).unwrap();
        crate::accounts::Accounts::new(db.clone()).create("a.test", "sam", "password1", crate::accounts::Role::Member).await.unwrap();
        FastTokens::new(db)
    }

    async fn valid(s: &FastTokens) -> Vec<Vec<u8>> {
        s.tokens("a.test", "sam", "HT-SHA-256-NONE").await.into_iter().map(|t| t.secret).collect()
    }

    #[tokio::test]
    async fn rotation_keeps_the_used_token_until_the_new_one_is_used() {
        let s = store().await;
        let (t1, _) = s.issue("a.test", "sam", "phone", "HT-SHA-256-NONE").await.unwrap();
        s.used("a.test", "sam", t1.as_bytes()).await;
        let (t2, _) = s.issue("a.test", "sam", "phone", "HT-SHA-256-NONE").await.unwrap();
        assert_eq!(valid(&s).await, vec![t2.clone().into_bytes(), t1.clone().into_bytes()]);
        s.used("a.test", "sam", t2.as_bytes()).await;
        assert_eq!(valid(&s).await, vec![t2.into_bytes()], "using the new token retires the old one");
    }

    #[tokio::test]
    async fn an_unused_token_is_replaced_by_the_next() {
        let s = store().await;
        let (t1, _) = s.issue("a.test", "sam", "phone", "HT-SHA-256-NONE").await.unwrap();
        let (t2, _) = s.issue("a.test", "sam", "phone", "HT-SHA-256-NONE").await.unwrap();
        assert_eq!(valid(&s).await, vec![t2.into_bytes()]);
        assert!(!valid(&s).await.contains(&t1.into_bytes()));
    }

    #[tokio::test]
    async fn revoke_all_clears_tokens() {
        let s = store().await;
        s.issue("a.test", "sam", "phone", "HT-SHA-256-NONE").await.unwrap();
        s.revoke_all("a.test", "sam").await.unwrap();
        assert!(s.tokens("a.test", "sam", "HT-SHA-256-NONE").await.is_empty());
    }

    #[tokio::test]
    async fn invalidating_the_token_in_use_keeps_the_newer_one() {
        // XEP-0484 §3.6: only the token that signed in stops working.
        let s = store().await;
        let (t1, _) = s.issue("a.test", "sam", "phone", "HT-SHA-256-NONE").await.unwrap();
        s.used("a.test", "sam", t1.as_bytes()).await;
        let (t2, _) = s.issue("a.test", "sam", "phone", "HT-SHA-256-NONE").await.unwrap();
        s.revoke("a.test", "sam", "phone", t1.as_bytes()).await.unwrap();
        assert_eq!(valid(&s).await, vec![t2.clone().into_bytes()]);
        s.revoke("a.test", "sam", "phone", t2.as_bytes()).await.unwrap();
        assert!(valid(&s).await.is_empty());
    }
}
