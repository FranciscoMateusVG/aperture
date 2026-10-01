//! Boot-local browser authority. Only digests persist in memory; never Debug tokens.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

type Key = [u8; 32];
const EXCHANGE_TTL: Duration = Duration::from_secs(30);
const IDLE: Duration = Duration::from_secs(12 * 3600);
const ABSOLUTE: Duration = Duration::from_secs(24 * 3600);
const MAX_EXCHANGES: usize = 64;
const MAX_SESSIONS: usize = 256;

pub(super) fn credential() -> Result<String, ()> {
    let mut bytes = [0u8; 32];
    OsRng.try_fill_bytes(&mut bytes).map_err(|_| ())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn digest(value: &str) -> Option<Key> {
    let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
    if bytes.len() != 32 || URL_SAFE_NO_PAD.encode(&bytes) != value {
        return None;
    }
    Some(Sha256::digest(value.as_bytes()).into())
}
fn same(a: &Key, b: &Key) -> bool {
    bool::from(a.ct_eq(b))
}
struct Session {
    key: Key,
    born: Duration,
    last: Duration,
}
struct Exchange {
    key: Key,
    expires: Duration,
    parent: Option<Key>,
}

pub(super) struct BrowserAuth {
    open: Key,
    start: Instant,
    sessions: Vec<Session>,
    exchanges: Vec<Exchange>,
    mints: Vec<Duration>,
    #[cfg(test)]
    offset: Duration,
}
impl BrowserAuth {
    pub fn new(open: &str) -> Result<Self, ()> {
        Ok(Self {
            open: digest(open).ok_or(())?,
            start: Instant::now(),
            sessions: vec![],
            exchanges: vec![],
            mints: vec![],
            #[cfg(test)]
            offset: Duration::ZERO,
        })
    }
    fn now(&self) -> Duration {
        let t = self.start.elapsed();
        #[cfg(test)]
        {
            return t + self.offset;
        }
        #[cfg(not(test))]
        {
            t
        }
    }
    fn prune(&mut self) {
        let now = self.now();
        self.sessions
            .retain(|s| now - s.born < ABSOLUTE && now - s.last < IDLE);
        self.exchanges.retain(|e| {
            now < e.expires
                && e.parent
                    .map_or(true, |p| self.sessions.iter().any(|s| same(&p, &s.key)))
        });
        self.mints.retain(|t| now - *t < Duration::from_secs(60));
    }
    pub fn valid(&mut self, bearer: &str) -> bool {
        self.prune();
        let Some(key) = digest(bearer) else {
            return false;
        };
        let now = self.now();
        if let Some(s) = self.sessions.iter_mut().find(|s| same(&s.key, &key)) {
            s.last = now;
            true
        } else {
            false
        }
    }
    pub fn native_operator(&self, capability: &str) -> bool {
        digest(capability).is_some_and(|k| same(&self.open, &k))
    }
    pub fn mint_open(&mut self, capability: &str) -> Result<String, u16> {
        if !self.native_operator(capability) {
            return Err(401);
        }
        self.mint(None)
    }
    pub fn link(&mut self, bearer: &str) -> Result<String, u16> {
        if !self.valid(bearer) {
            return Err(401);
        }
        self.mint(digest(bearer))
    }
    fn mint(&mut self, parent: Option<Key>) -> Result<String, u16> {
        self.prune();
        if self.exchanges.len() >= MAX_EXCHANGES || self.mints.len() >= 16 {
            return Err(429);
        }
        let value = credential().map_err(|_| 503u16)?;
        self.exchanges.push(Exchange {
            key: digest(&value).ok_or(503u16)?,
            expires: self.now() + EXCHANGE_TTL,
            parent,
        });
        self.mints.push(self.now());
        Ok(value)
    }
    pub fn redeem(&mut self, exchange: &str) -> Result<String, u16> {
        self.prune();
        let key = digest(exchange).ok_or(410u16)?;
        let index = self
            .exchanges
            .iter()
            .position(|e| same(&e.key, &key))
            .ok_or(410u16)?;
        // Under the single caller-held mutex: even a failed issuance consumes it.
        self.exchanges.remove(index);
        if self.sessions.len() >= MAX_SESSIONS {
            return Err(429);
        }
        let value = credential().map_err(|_| 503u16)?;
        let now = self.now();
        self.sessions.push(Session {
            key: digest(&value).ok_or(503u16)?,
            born: now,
            last: now,
        });
        Ok(value)
    }
    pub fn logout(&mut self, bearer: &str) -> bool {
        if !self.valid(bearer) {
            return false;
        }
        let key = digest(bearer).expect("validated digest");
        self.sessions.retain(|s| !same(&s.key, &key));
        self.prune();
        true
    }
    #[cfg(test)]
    pub fn advance(&mut self, duration: Duration) {
        self.offset += duration;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expiry_caps_parent_and_boot_are_factual() {
        let open = credential().unwrap();
        let mut auth = BrowserAuth::new(&open).unwrap();
        assert!(!auth.valid(&open));
        let expired = auth.mint_open(&open).unwrap();
        auth.advance(EXCHANGE_TTL);
        assert_eq!(auth.redeem(&expired), Err(410));
        let e = auth.mint_open(&open).unwrap();
        let session = auth.redeem(&e).unwrap();
        assert_eq!(auth.redeem(&e), Err(410));
        let child = auth.link(&session).unwrap();
        assert!(auth.logout(&session));
        assert_eq!(auth.redeem(&child), Err(410));
        assert!(!auth.valid(&session));
        for _ in 0..13 {
            auth.mint_open(&open).unwrap();
        }
        assert_eq!(auth.mint_open(&open), Err(429));
        let mut next = BrowserAuth::new(&credential().unwrap()).unwrap();
        assert_eq!(next.mint_open(&open), Err(401));
        assert!(!next.valid(&session));
    }
    #[test]
    fn idle_and_absolute_expiry_cannot_be_renewed_forever() {
        let open = credential().unwrap();
        let mut a = BrowserAuth::new(&open).unwrap();
        let e = a.mint_open(&open).unwrap();
        let s = a.redeem(&e).unwrap();
        a.advance(IDLE);
        assert!(!a.valid(&s));
        let e = a.mint_open(&open).unwrap();
        let s = a.redeem(&e).unwrap();
        for _ in 0..3 {
            a.advance(Duration::from_secs(7 * 3600));
            assert!(a.valid(&s));
        }
        a.advance(Duration::from_secs(3 * 3600));
        assert!(!a.valid(&s));
    }
}
