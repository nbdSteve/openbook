//! The public demo: anonymous questions about one demo course (`docs/api.md`, "Demo").
//!
//! This module holds the parts that need no AWS call and are easy to test: the visitor ID, the three
//! limits, the fixed reply, and the types of the reserved DynamoDB item. `Store` implements the
//! counters and the salt with DynamoDB (`store/demo.rs`).

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::config::DemoConfig;
use crate::quota::usage_date;
use crate::store::StoreError;

/// The owner of the demo course. It is not a Cognito user (those IDs are UUIDs), so nobody can
/// sign in as the owner. The demo course has no membership and no invite code.
pub const OWNER_ID: &str = "demo";
/// The longest question, in characters after trim.
pub const MAX_MESSAGE_CHARS: usize = 300;
/// The output limit of a demo answer. The server fixes it.
pub const MAX_OUTPUT_TOKENS: i32 = 700;
/// The length of the short window of the per-visitor limit.
pub const WINDOW_SECONDS: i64 = 600;
/// Counters and salts expire this long after the day they belong to starts.
const DAY_ITEM_TTL_SECONDS: i64 = 2 * 24 * 3600;
/// The size of the daily salt.
pub const SALT_BYTES: usize = 32;
/// The stop reason of a fixed reply (no model call).
pub const NOT_COVERED_STOP_REASON: &str = "not_covered";

/// Where a demo source comes from, for the credit line in the UI. The licence needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attribution {
    pub title: String,
    /// The authors as the publisher lists them.
    pub authors: String,
    pub publisher: String,
    pub copyright: String,
    pub license: String,
    pub license_url: String,
    pub source_url: String,
    /// What was changed (for CC BY: "indicate if changes were made").
    pub changes: String,
}

/// The active demo course. It is stored as one reserved item, so the API needs no configured ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoCourse {
    pub course_id: String,
    /// Hash of the manifest and the PDFs. The same value means the same content.
    pub fingerprint: String,
    /// What the course covers, as part of a sentence: "exponents and quadratic equations".
    pub coverage: String,
    /// Two example questions for the fixed reply.
    pub examples: Vec<String>,
    pub attribution: Vec<Attribution>,
    pub provisioned_at: String,
}

/// The fixed reply for a question that the materials do not cover. It makes no model call.
pub fn not_covered_reply(coverage: &str, examples: &[String]) -> String {
    let mut reply =
        format!("I could not find that in the demo course materials. This demo covers {coverage}.");
    if !examples.is_empty() {
        reply.push_str(" Try one of these questions:\n\n");
        let lines: Vec<String> = examples.iter().take(2).map(|e| format!("- {e}")).collect();
        reply.push_str(&lines.join("\n"));
    }
    reply
}

// ---------------------------------------------------------------------------------------------
// The visitor
// ---------------------------------------------------------------------------------------------

/// Reads the address of the viewer from `CloudFront-Viewer-Address`: `203.0.113.7:46532` for IPv4.
/// For IPv6 the port is the part after the last colon, with or without brackets around the address.
/// It returns `None` for anything else, and the caller must then fail closed.
pub fn parse_viewer_address(value: &str) -> Option<IpAddr> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(socket) = value.parse::<SocketAddr>() {
        return Some(socket.ip());
    }
    // An address without a port is also fine.
    if let Ok(ip) = value.parse::<IpAddr>()
        && !value.contains(':')
    {
        return Some(ip);
    }
    // IPv6 without brackets: `2001:db8::1:46532`. The port is always the last group, because a
    // port can look like a valid group, so the text cannot be read as one address.
    if value.matches(':').count() >= 2 {
        let (address, port) = value.rsplit_once(':')?;
        port.parse::<u16>().ok()?;
        return address
            .trim_matches(['[', ']'])
            .parse::<Ipv6Addr>()
            .ok()
            .map(IpAddr::V6);
    }
    None
}

/// The address that stands for one visitor. An IPv6 visitor has a whole `/64`, so the address is cut
/// to that prefix. An IPv4-mapped IPv6 address is the IPv4 address.
pub fn visitor_address(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut octets = v6.octets();
                octets[8..].fill(0);
                IpAddr::V6(Ipv6Addr::from(octets))
            }
        },
    }
}

/// `HMAC-SHA256(salt, address)` as 32 hex characters. The salt changes every day, so a hash from
/// one day cannot be linked to a hash from another day.
pub fn visitor_hash(salt: &[u8], ip: IpAddr) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(salt)
        .unwrap_or_else(|_| unreachable!("HMAC accepts a key of any length"));
    mac.update(visitor_address(ip).to_string().as_bytes());
    let bytes = mac.finalize().into_bytes();
    hex(&bytes[..16])
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// Where the daily salts live. `Store` implements it with DynamoDB.
#[async_trait]
pub trait SaltStore: Send + Sync {
    /// Returns the salt of `date`. When there is none, it stores `fresh` (expiring at `expires_at`,
    /// epoch seconds) and returns it. Two callers at once get the same salt.
    async fn salt_for(
        &self,
        date: &str,
        fresh: [u8; SALT_BYTES],
        expires_at: i64,
    ) -> Result<Vec<u8>, StoreError>;
}

/// The salt of the current day, kept in memory after the first read. A salt never changes once it
/// exists, so the cache is always right for its day.
#[derive(Default)]
pub struct Visitors {
    cache: Mutex<Option<(String, Vec<u8>)>>,
}

impl Visitors {
    /// The ID of the visitor behind `ip` for the day of `now`.
    pub async fn id(
        &self,
        store: &dyn SaltStore,
        now: DateTime<Utc>,
        ip: IpAddr,
    ) -> Result<String, StoreError> {
        let date = usage_date(now);
        let cached = self
            .cache
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .filter(|(day, _)| *day == date)
            .map(|(_, salt)| salt);
        let salt = match cached {
            Some(salt) => salt,
            None => {
                let mut fresh = [0u8; SALT_BYTES];
                getrandom::fill(&mut fresh)
                    .map_err(|e| StoreError::Internal(format!("no random bytes: {e}")))?;
                let expires_at = now.timestamp() + DAY_ITEM_TTL_SECONDS;
                let salt = store.salt_for(&date, fresh, expires_at).await?;
                if let Ok(mut slot) = self.cache.lock() {
                    *slot = Some((date, salt.clone()));
                }
                salt
            }
        };
        Ok(visitor_hash(&salt, ip))
    }
}

// ---------------------------------------------------------------------------------------------
// The limits
// ---------------------------------------------------------------------------------------------

/// One counter item in the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counter {
    pub pk: String,
    pub sk: String,
    /// Epoch seconds. DynamoDB removes the item some time after this.
    pub expires_at: i64,
}

/// Which limit stopped a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitHit {
    /// The visitor asked too often in the last 10 minutes.
    VisitorWindow,
    /// The visitor used all of today's questions.
    VisitorDay,
    /// The whole demo used all of today's model calls.
    Global,
}

impl LimitHit {
    pub fn as_str(self) -> &'static str {
        match self {
            LimitHit::VisitorWindow => "visitor_window",
            LimitHit::VisitorDay => "visitor_day",
            LimitHit::Global => "global_day",
        }
    }
}

/// The counters behind the limits. `Store` implements it with DynamoDB.
#[async_trait]
pub trait DemoCounters: Send + Sync {
    /// Adds one to the counter unless it is at `limit`. Returns whether it reserved.
    async fn reserve(&self, counter: &Counter, limit: u32) -> Result<bool, StoreError>;
    /// Gives one reservation back.
    async fn release(&self, counter: &Counter) -> Result<(), StoreError>;
}

/// The three counters of one question. Keep it to give them back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    window: Counter,
    day: Counter,
    global: Counter,
}

impl Reservation {
    fn new(visitor: &str, now: DateTime<Utc>) -> Self {
        let window = now.timestamp().div_euclid(WINDOW_SECONDS);
        let date = usage_date(now);
        let day_expiry = now.timestamp() + DAY_ITEM_TTL_SECONDS;
        Self {
            window: Counter {
                pk: format!("DEMO#V#{visitor}"),
                sk: format!("W#{window}"),
                // One window of margin after the window ends.
                expires_at: (window + 2) * WINDOW_SECONDS,
            },
            day: Counter {
                pk: format!("DEMO#V#{visitor}"),
                sk: format!("D#{date}"),
                expires_at: day_expiry,
            },
            global: Counter {
                pk: "DEMO#GLOBAL".to_owned(),
                sk: format!("D#{date}"),
                expires_at: day_expiry,
            },
        }
    }

    /// Gives the global reservation back. A question that made no model call does this.
    pub async fn release_global(&self, counters: &dyn DemoCounters) {
        if let Err(error) = counters.release(&self.global).await {
            tracing::error!(%error, "could not give back the demo global reservation");
        }
    }

    /// Gives every reservation back. A question that the model did not answer does this.
    pub async fn release_all(&self, counters: &dyn DemoCounters) {
        for counter in [&self.window, &self.day, &self.global] {
            if let Err(error) = counters.release(counter).await {
                tracing::error!(%error, "could not give back a demo reservation");
            }
        }
    }
}

/// Reserves one question: the visitor's 10-minute window, the visitor's day, then the global day.
/// A question that hits a limit leaves no counter changed. The windows are fixed 10-minute slots.
pub async fn reserve(
    counters: &dyn DemoCounters,
    visitor: &str,
    now: DateTime<Utc>,
    limits: &DemoConfig,
) -> Result<Result<Reservation, LimitHit>, StoreError> {
    let reservation = Reservation::new(visitor, now);
    let steps = [
        (
            &reservation.window,
            limits.visitor_per_window,
            LimitHit::VisitorWindow,
        ),
        (
            &reservation.day,
            limits.visitor_per_day,
            LimitHit::VisitorDay,
        ),
        (&reservation.global, limits.global_per_day, LimitHit::Global),
    ];
    let mut done: Vec<&Counter> = Vec::new();
    for (counter, limit, hit) in steps {
        match counters.reserve(counter, limit).await {
            Ok(true) => done.push(counter),
            Ok(false) => {
                give_back(counters, &done).await;
                return Ok(Err(hit));
            }
            Err(error) => {
                give_back(counters, &done).await;
                return Err(error);
            }
        }
    }
    Ok(Ok(reservation))
}

async fn give_back(counters: &dyn DemoCounters, done: &[&Counter]) {
    for counter in done {
        if let Err(error) = counters.release(counter).await {
            tracing::error!(%error, "could not give back a demo reservation");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct Memory {
        counts: Mutex<HashMap<(String, String), u32>>,
        salts: Mutex<HashMap<String, Vec<u8>>>,
        salt_calls: Mutex<u32>,
    }

    impl Memory {
        fn count(&self, pk: &str, sk: &str) -> u32 {
            *self
                .counts
                .lock()
                .unwrap()
                .get(&(pk.to_owned(), sk.to_owned()))
                .unwrap_or(&0)
        }
    }

    #[async_trait]
    impl DemoCounters for Memory {
        async fn reserve(&self, counter: &Counter, limit: u32) -> Result<bool, StoreError> {
            let mut counts = self.counts.lock().unwrap();
            let slot = counts
                .entry((counter.pk.clone(), counter.sk.clone()))
                .or_insert(0);
            if *slot >= limit {
                return Ok(false);
            }
            *slot += 1;
            Ok(true)
        }
        async fn release(&self, counter: &Counter) -> Result<(), StoreError> {
            let mut counts = self.counts.lock().unwrap();
            if let Some(slot) = counts.get_mut(&(counter.pk.clone(), counter.sk.clone())) {
                *slot = slot.saturating_sub(1);
            }
            Ok(())
        }
    }

    #[async_trait]
    impl SaltStore for Memory {
        async fn salt_for(
            &self,
            date: &str,
            fresh: [u8; SALT_BYTES],
            _expires_at: i64,
        ) -> Result<Vec<u8>, StoreError> {
            *self.salt_calls.lock().unwrap() += 1;
            Ok(self
                .salts
                .lock()
                .unwrap()
                .entry(date.to_owned())
                .or_insert_with(|| fresh.to_vec())
                .clone())
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn limits() -> DemoConfig {
        DemoConfig {
            enabled: true,
            visitor_per_window: 5,
            visitor_per_day: 20,
            global_per_day: 200,
        }
    }

    #[tokio::test]
    async fn a_visitor_gets_five_questions_in_ten_minutes() {
        let memory = Memory::default();
        let now = at("2026-11-17T10:00:30Z");
        for _ in 0..5 {
            assert!(
                reserve(&memory, "v1", now, &limits())
                    .await
                    .unwrap()
                    .is_ok()
            );
        }
        assert_eq!(
            reserve(&memory, "v1", now, &limits())
                .await
                .unwrap()
                .unwrap_err(),
            LimitHit::VisitorWindow
        );
        // A hit changes no counter: the day and the global counter stay at 5.
        assert_eq!(memory.count("DEMO#V#v1", "D#2026-11-17"), 5);
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 5);
        // Another visitor is not affected.
        assert!(
            reserve(&memory, "v2", now, &limits())
                .await
                .unwrap()
                .is_ok()
        );
        // The next window starts fresh.
        let later = at("2026-11-17T10:10:00Z");
        assert!(
            reserve(&memory, "v1", later, &limits())
                .await
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_visitor_gets_twenty_questions_a_day() {
        let memory = Memory::default();
        let mut now = at("2026-11-17T00:00:00Z");
        for i in 0..20 {
            // One question every 11 minutes stays under the window limit.
            now += chrono::Duration::minutes(11);
            assert!(
                reserve(&memory, "v1", now, &limits())
                    .await
                    .unwrap()
                    .is_ok(),
                "question {i}"
            );
        }
        now += chrono::Duration::minutes(11);
        assert_eq!(
            reserve(&memory, "v1", now, &limits())
                .await
                .unwrap()
                .unwrap_err(),
            LimitHit::VisitorDay
        );
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 20);
    }

    #[tokio::test]
    async fn the_whole_demo_gets_a_fixed_number_of_model_calls_a_day() {
        let memory = Memory::default();
        let now = at("2026-11-17T10:00:00Z");
        let limits = DemoConfig {
            global_per_day: 3,
            ..limits()
        };
        for visitor in ["a", "b", "c"] {
            assert!(
                reserve(&memory, visitor, now, &limits)
                    .await
                    .unwrap()
                    .is_ok()
            );
        }
        assert_eq!(
            reserve(&memory, "d", now, &limits)
                .await
                .unwrap()
                .unwrap_err(),
            LimitHit::Global
        );
        // The visitor of the refused question keeps the question for later.
        assert_eq!(memory.count("DEMO#V#d", "D#2026-11-17"), 0);
        let window = now.timestamp() / WINDOW_SECONDS;
        assert_eq!(memory.count("DEMO#V#d", &format!("W#{window}")), 0);
        // The next day has a new global counter.
        assert!(
            reserve(&memory, "d", at("2026-11-18T00:00:00Z"), &limits)
                .await
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn giving_back_makes_room() {
        let memory = Memory::default();
        let now = at("2026-11-17T10:00:00Z");
        let limits = DemoConfig {
            visitor_per_window: 1,
            ..limits()
        };
        let first = reserve(&memory, "v", now, &limits).await.unwrap().unwrap();
        // A question with no model call gives back only the global call.
        first.release_global(&memory).await;
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 0);
        assert_eq!(memory.count("DEMO#V#v", "D#2026-11-17"), 1);
        assert!(reserve(&memory, "v", now, &limits).await.unwrap().is_err());
        first.release_all(&memory).await;
        assert!(reserve(&memory, "v", now, &limits).await.unwrap().is_ok());
    }

    #[test]
    fn counters_expire() {
        let now = at("2026-11-17T10:03:00Z");
        let r = Reservation::new("v", now);
        assert!(r.window.expires_at > now.timestamp() + WINDOW_SECONDS);
        assert!(r.window.expires_at <= now.timestamp() + 3 * WINDOW_SECONDS);
        assert_eq!(r.day.expires_at, now.timestamp() + DAY_ITEM_TTL_SECONDS);
        assert_eq!(r.global.expires_at, r.day.expires_at);
    }

    #[tokio::test]
    async fn the_visitor_hash_depends_on_the_day_and_hides_the_address() {
        let memory = Visitors::default();
        let store = Memory::default();
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let day1 = at("2026-11-17T10:00:00Z");
        let a = memory.id(&store, day1, ip).await.unwrap();
        assert_eq!(a.len(), 32);
        assert!(!a.contains("203"), "no address in the hash: {a}");
        // Same day, same address: same ID, and the salt is read once.
        assert_eq!(memory.id(&store, day1, ip).await.unwrap(), a);
        assert_eq!(*store.salt_calls.lock().unwrap(), 1);
        // Another address: another ID.
        let other: IpAddr = "203.0.113.8".parse().unwrap();
        assert_ne!(memory.id(&store, day1, other).await.unwrap(), a);
        // The next day has a new salt, so the same address has an ID that cannot be linked.
        let b = memory
            .id(&store, at("2026-11-18T10:00:00Z"), ip)
            .await
            .unwrap();
        assert_ne!(a, b);
        assert_eq!(*store.salt_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn two_servers_share_the_salt_of_a_day() {
        let store = Memory::default();
        let one = Visitors::default();
        let two = Visitors::default();
        let ip: IpAddr = "198.51.100.1".parse().unwrap();
        let now = at("2026-11-17T10:00:00Z");
        assert_eq!(
            one.id(&store, now, ip).await.unwrap(),
            two.id(&store, now, ip).await.unwrap()
        );
    }

    #[test]
    fn an_ipv6_visitor_is_a_whole_slash_64() {
        let salt = [7u8; 32];
        let a: IpAddr = "2001:db8:1:2:aaaa:bbbb:cccc:dddd".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:1111:2222:3333:4444".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(visitor_hash(&salt, a), visitor_hash(&salt, b));
        assert_ne!(visitor_hash(&salt, a), visitor_hash(&salt, c));
        // An IPv4-mapped address is the IPv4 address.
        let mapped: IpAddr = "::ffff:203.0.113.7".parse().unwrap();
        let plain: IpAddr = "203.0.113.7".parse().unwrap();
        assert_eq!(visitor_hash(&salt, mapped), visitor_hash(&salt, plain));
    }

    #[test]
    fn reads_the_viewer_address_header() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        assert_eq!(
            parse_viewer_address("203.0.113.7:46532"),
            Some(ip("203.0.113.7"))
        );
        assert_eq!(
            parse_viewer_address(" 203.0.113.7:46532 "),
            Some(ip("203.0.113.7"))
        );
        assert_eq!(parse_viewer_address("203.0.113.7"), Some(ip("203.0.113.7")));
        assert_eq!(
            parse_viewer_address("[2001:db8::1]:46532"),
            Some(ip("2001:db8::1"))
        );
        // The port is the last group even when it looks like a group.
        assert_eq!(
            parse_viewer_address("2001:db8::1:46532"),
            Some(ip("2001:db8::1"))
        );
        assert_eq!(
            parse_viewer_address("2001:db8::1:1234"),
            Some(ip("2001:db8::1"))
        );
        for bad in [
            "",
            "   ",
            "unknown",
            "not-an-ip:80",
            "203.0.113.7:99999",
            "::1:abc",
        ] {
            assert_eq!(parse_viewer_address(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_fixed_reply_names_the_topics_and_two_questions() {
        let reply = not_covered_reply(
            "exponents and the US Constitution",
            &["What is a quadratic?".into(), "What is federalism?".into()],
        );
        assert!(reply.contains("This demo covers exponents and the US Constitution."));
        assert!(reply.contains("- What is a quadratic?"));
        assert!(reply.contains("- What is federalism?"));
        assert!(!reply.contains('\u{2014}'));
        // The same input always gives the same text.
        assert_eq!(
            reply,
            not_covered_reply(
                "exponents and the US Constitution",
                &["What is a quadratic?".into(), "What is federalism?".into()]
            )
        );
        assert!(!not_covered_reply("x", &[]).contains("Try one"));
    }
}
