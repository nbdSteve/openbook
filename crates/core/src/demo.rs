//! The public demo: anonymous questions about one demo course (`docs/api.md`, "Demo").
//!
//! This module holds the parts that need no AWS call and are easy to test: the visitor ID, the
//! per-visitor limits, the daily budget in US dollars, the conversation rules, and the types of the
//! reserved DynamoDB item. `Store` implements the counters, the salt, and the conversations with
//! DynamoDB (`store/demo.rs`).

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::config::DemoConfig;
use crate::domain::MessageRole;
use crate::quota::usage_date;
use crate::store::StoreError;

/// The owner of the demo course. It is not a Cognito user (those IDs are UUIDs), so nobody can
/// sign in as the owner. The demo course has no membership and no invite code.
pub const OWNER_ID: &str = "demo";
/// The longest question, in characters after trim.
pub const MAX_MESSAGE_CHARS: usize = 300;
/// The length of the short window of the per-visitor limit.
pub const WINDOW_SECONDS: i64 = 600;
/// Counters and salts expire this long after the day they belong to starts.
const DAY_ITEM_TTL_SECONDS: i64 = 2 * 24 * 3600;
/// The size of the daily salt.
pub const SALT_BYTES: usize = 32;
/// A demo conversation expires this long after its first turn.
pub const CONVERSATION_TTL_SECONDS: i64 = 24 * 3600;
/// A demo conversation has at most this many turns (a question and its answer).
pub const MAX_TURNS: u32 = 10;
/// The size of the random part of a conversation ID.
pub const CONVERSATION_ID_BYTES: usize = 24;

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
    pub attribution: Vec<Attribution>,
    pub provisioned_at: String,
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

/// Which per-visitor limit stopped a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitHit {
    /// The visitor asked too often in the last 10 minutes.
    VisitorWindow,
    /// The visitor used all of today's questions.
    VisitorDay,
}

impl LimitHit {
    /// How long until this visitor can ask again: the end of the 10-minute window, or midnight UTC.
    /// Always at least 1.
    pub fn retry_after_seconds(self, now: DateTime<Utc>) -> u64 {
        let ts = now.timestamp();
        let seconds = match self {
            LimitHit::VisitorWindow => (ts.div_euclid(WINDOW_SECONDS) + 1) * WINDOW_SECONDS - ts,
            LimitHit::VisitorDay => 86_400 - ts.rem_euclid(86_400),
        };
        u64::try_from(seconds.max(1)).unwrap_or(1)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LimitHit::VisitorWindow => "visitor_window",
            LimitHit::VisitorDay => "visitor_day",
        }
    }
}

/// The counters behind the limits and the budget. `Store` implements it with DynamoDB.
/// A counter holds a number (questions, or millionths of a US dollar).
#[async_trait]
pub trait DemoCounters: Send + Sync {
    /// Adds `amount` to the counter when the result stays at or below `limit`. One atomic step.
    /// Returns whether it added.
    async fn reserve(&self, counter: &Counter, amount: u64, limit: u64)
    -> Result<bool, StoreError>;
    /// Takes `amount` back (never below zero).
    async fn release(&self, counter: &Counter, amount: u64) -> Result<(), StoreError>;
    /// Adds `amount` with no limit. Settlement uses it when a call cost more than it reserved.
    async fn charge(&self, counter: &Counter, amount: u64) -> Result<(), StoreError>;
}

/// The two per-visitor counters of one question. Keep it to give them back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisitorReservation {
    window: Counter,
    day: Counter,
}

impl VisitorReservation {
    fn new(visitor: &str, now: DateTime<Utc>) -> Self {
        let window = now.timestamp().div_euclid(WINDOW_SECONDS);
        Self {
            window: Counter {
                pk: format!("DEMO#V#{visitor}"),
                sk: format!("W#{window}"),
                // One window of margin after the window ends.
                expires_at: (window + 2) * WINDOW_SECONDS,
            },
            day: Counter {
                pk: format!("DEMO#V#{visitor}"),
                sk: format!("D#{}", usage_date(now)),
                expires_at: now.timestamp() + DAY_ITEM_TTL_SECONDS,
            },
        }
    }

    /// Gives both counters back. A question that the model did not answer does this.
    pub async fn release(&self, counters: &dyn DemoCounters) {
        for counter in [&self.window, &self.day] {
            if let Err(error) = counters.release(counter, 1).await {
                tracing::error!(%error, "could not give back a demo visitor reservation");
            }
        }
    }
}

/// Reserves one question: the visitor's 10-minute window, then the visitor's day. A question that
/// hits a limit leaves no counter changed. The windows are fixed 10-minute slots.
pub async fn reserve_visitor(
    counters: &dyn DemoCounters,
    visitor: &str,
    now: DateTime<Utc>,
    limits: &DemoConfig,
) -> Result<Result<VisitorReservation, LimitHit>, StoreError> {
    let reservation = VisitorReservation::new(visitor, now);
    if !counters
        .reserve(&reservation.window, 1, u64::from(limits.visitor_per_window))
        .await?
    {
        return Ok(Err(LimitHit::VisitorWindow));
    }
    match counters
        .reserve(&reservation.day, 1, u64::from(limits.visitor_per_day))
        .await
    {
        Ok(true) => Ok(Ok(reservation)),
        Ok(false) => {
            if let Err(error) = counters.release(&reservation.window, 1).await {
                tracing::error!(%error, "could not give back a demo window reservation");
            }
            Ok(Err(LimitHit::VisitorDay))
        }
        Err(error) => {
            let _ = counters.release(&reservation.window, 1).await;
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The budget
// ---------------------------------------------------------------------------------------------

/// The money that one turn may still spend: a reservation on the global counter of the day.
///
/// The turn reserves its worst case before it calls a model, and settles with the real cost after.
/// The counter is the sum of the settled costs and the open reservations of the day (UTC), so two
/// turns at once can never spend more than the budget by reservation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetReservation {
    counter: Counter,
    reserved: u64,
}

impl BudgetReservation {
    pub fn reserved_micro_usd(&self) -> u64 {
        self.reserved
    }

    /// Replaces the reservation with the real cost: it gives back the unused part, or charges the
    /// part that went over the reservation (the estimate of the input was too low).
    pub async fn settle(self, counters: &dyn DemoCounters, actual_micro_usd: u64) {
        let result = if actual_micro_usd <= self.reserved {
            counters
                .release(&self.counter, self.reserved - actual_micro_usd)
                .await
        } else {
            tracing::warn!(
                reserved = self.reserved,
                actual = actual_micro_usd,
                "a demo turn cost more than its reservation"
            );
            counters
                .charge(&self.counter, actual_micro_usd - self.reserved)
                .await
        };
        if let Err(error) = result {
            tracing::error!(%error, "could not settle the demo budget");
        }
    }
}

/// The global counter of a day: the money in millionths of a US dollar.
pub fn budget_counter(now: DateTime<Utc>) -> Counter {
    Counter {
        pk: "DEMO#GLOBAL".to_owned(),
        sk: format!("D#{}", usage_date(now)),
        expires_at: now.timestamp() + DAY_ITEM_TTL_SECONDS,
    }
}

/// Reserves `amount` of the day's budget. `None` means that it does not fit.
pub async fn reserve_budget(
    counters: &dyn DemoCounters,
    now: DateTime<Utc>,
    amount_micro_usd: u64,
    budget_micro_usd: u64,
) -> Result<Option<BudgetReservation>, StoreError> {
    if amount_micro_usd > budget_micro_usd {
        return Ok(None);
    }
    let counter = budget_counter(now);
    if counters
        .reserve(&counter, amount_micro_usd, budget_micro_usd)
        .await?
    {
        Ok(Some(BudgetReservation {
            counter,
            reserved: amount_micro_usd,
        }))
    } else {
        Ok(None)
    }
}

/// An upper bound of the tokens in a text. The real count is about one token per 4 characters for
/// English, and about one per 2 or 3 for dense LaTeX, so the bound uses 3 characters per token.
pub fn worst_case_tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(3)
}

/// Parses a price like `2` or `2.50` (US dollars) into millionths of a dollar.
pub fn usd_to_micro(usd: f64) -> Option<u64> {
    (usd.is_finite() && usd > 0.0 && usd <= 1_000_000.0).then(|| (usd * 1_000_000.0).round() as u64)
}

// ---------------------------------------------------------------------------------------------
// Conversations
// ---------------------------------------------------------------------------------------------

/// The ID of a demo conversation: 24 random bytes (192 bits) as 48 hex characters. It is the only
/// secret that gives access to the conversation, so it must not be guessable.
pub fn new_conversation_id() -> Result<String, StoreError> {
    let mut bytes = [0u8; CONVERSATION_ID_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|e| StoreError::Internal(format!("no random bytes: {e}")))?;
    Ok(hex(&bytes))
}

/// True for the shape of an ID from `new_conversation_id`. Anything else is not looked up.
pub fn is_conversation_id(id: &str) -> bool {
    id.len() == CONVERSATION_ID_BYTES * 2
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A stored demo conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConversation {
    /// Questions and answers so far.
    pub turns: u32,
    /// Epoch seconds. After this the conversation does not exist, although DynamoDB may keep the item.
    pub expires_at: i64,
    /// The messages, oldest first.
    pub messages: Vec<(MessageRole, String)>,
}

/// Where demo conversations live. `Store` implements it with DynamoDB.
#[async_trait]
pub trait DemoConversations: Send + Sync {
    async fn load(&self, id: &str) -> Result<Option<StoredConversation>, StoreError>;
    /// Adds one turn. It returns false (and writes nothing) when the conversation already has
    /// `MAX_TURNS` turns.
    async fn append(
        &self,
        id: &str,
        question: &str,
        answer: &str,
        expires_at: i64,
    ) -> Result<bool, StoreError>;
}

/// The conversation that a request continues, or a fresh one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resumed {
    pub id: String,
    /// The earlier messages, oldest first. Empty for a fresh conversation.
    pub history: Vec<(MessageRole, String)>,
    pub expires_at: i64,
}

/// Continues the conversation of `requested` when it exists, has not expired, and has room for one
/// more turn. In every other case the request starts a fresh conversation with a new ID.
pub async fn resume(
    store: &dyn DemoConversations,
    requested: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Resumed, StoreError> {
    if let Some(id) = requested.filter(|id| is_conversation_id(id))
        && let Some(found) = store.load(id).await?
        && found.expires_at > now.timestamp()
        && found.turns < MAX_TURNS
    {
        return Ok(Resumed {
            id: id.to_owned(),
            history: found.messages,
            expires_at: found.expires_at,
        });
    }
    Ok(Resumed {
        id: new_conversation_id()?,
        history: Vec::new(),
        expires_at: now.timestamp() + CONVERSATION_TTL_SECONDS,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Counters in memory. The mutex makes each step atomic, like a conditional update.
    #[derive(Default)]
    struct Memory {
        counts: Mutex<HashMap<(String, String), u64>>,
        salts: Mutex<HashMap<String, Vec<u8>>>,
        salt_calls: Mutex<u32>,
        conversations: Mutex<HashMap<String, StoredConversation>>,
    }

    impl Memory {
        fn count(&self, pk: &str, sk: &str) -> u64 {
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
        async fn reserve(&self, c: &Counter, amount: u64, limit: u64) -> Result<bool, StoreError> {
            let mut counts = self.counts.lock().unwrap();
            let slot = counts.entry((c.pk.clone(), c.sk.clone())).or_insert(0);
            if *slot + amount > limit {
                return Ok(false);
            }
            *slot += amount;
            Ok(true)
        }
        async fn release(&self, c: &Counter, amount: u64) -> Result<(), StoreError> {
            let mut counts = self.counts.lock().unwrap();
            if let Some(slot) = counts.get_mut(&(c.pk.clone(), c.sk.clone())) {
                *slot = slot.saturating_sub(amount);
            }
            Ok(())
        }
        async fn charge(&self, c: &Counter, amount: u64) -> Result<(), StoreError> {
            let mut counts = self.counts.lock().unwrap();
            *counts.entry((c.pk.clone(), c.sk.clone())).or_insert(0) += amount;
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

    #[async_trait]
    impl DemoConversations for Memory {
        async fn load(&self, id: &str) -> Result<Option<StoredConversation>, StoreError> {
            Ok(self.conversations.lock().unwrap().get(id).cloned())
        }
        async fn append(
            &self,
            id: &str,
            question: &str,
            answer: &str,
            expires_at: i64,
        ) -> Result<bool, StoreError> {
            let mut all = self.conversations.lock().unwrap();
            let conversation = all.entry(id.to_owned()).or_insert(StoredConversation {
                turns: 0,
                expires_at,
                messages: Vec::new(),
            });
            if conversation.turns >= MAX_TURNS {
                return Ok(false);
            }
            conversation.turns += 1;
            conversation
                .messages
                .push((MessageRole::User, question.to_owned()));
            conversation
                .messages
                .push((MessageRole::Assistant, answer.to_owned()));
            Ok(true)
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
            visitor_per_window: 10,
            visitor_per_day: 40,
            budget_micro_usd_per_day: 2_000_000,
        }
    }

    #[tokio::test]
    async fn a_visitor_gets_ten_questions_in_ten_minutes() {
        let memory = Memory::default();
        let now = at("2026-11-17T10:00:30Z");
        for _ in 0..10 {
            assert!(
                reserve_visitor(&memory, "v1", now, &limits())
                    .await
                    .unwrap()
                    .is_ok()
            );
        }
        assert_eq!(
            reserve_visitor(&memory, "v1", now, &limits())
                .await
                .unwrap()
                .unwrap_err(),
            LimitHit::VisitorWindow
        );
        // A hit changes no counter: the day counter stays at 10.
        assert_eq!(memory.count("DEMO#V#v1", "D#2026-11-17"), 10);
        // Another visitor is not affected.
        assert!(
            reserve_visitor(&memory, "v2", now, &limits())
                .await
                .unwrap()
                .is_ok()
        );
        // The next window starts fresh.
        let later = at("2026-11-17T10:10:00Z");
        assert!(
            reserve_visitor(&memory, "v1", later, &limits())
                .await
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_visitor_gets_forty_questions_a_day() {
        let memory = Memory::default();
        let mut now = at("2026-11-17T00:00:00Z");
        for i in 0..40 {
            // One question every 11 minutes stays under the window limit.
            now += chrono::Duration::minutes(11);
            assert!(
                reserve_visitor(&memory, "v1", now, &limits())
                    .await
                    .unwrap()
                    .is_ok(),
                "question {i}"
            );
        }
        now += chrono::Duration::minutes(11);
        assert_eq!(
            reserve_visitor(&memory, "v1", now, &limits())
                .await
                .unwrap()
                .unwrap_err(),
            LimitHit::VisitorDay
        );
        // The refusal gave the window slot back.
        let window = now.timestamp() / WINDOW_SECONDS;
        assert_eq!(memory.count("DEMO#V#v1", &format!("W#{window}")), 0);
    }

    #[tokio::test]
    async fn giving_back_a_visitor_question_makes_room() {
        let memory = Memory::default();
        let now = at("2026-11-17T10:00:00Z");
        let limits = DemoConfig {
            visitor_per_window: 1,
            ..limits()
        };
        let first = reserve_visitor(&memory, "v", now, &limits)
            .await
            .unwrap()
            .unwrap();
        assert!(
            reserve_visitor(&memory, "v", now, &limits)
                .await
                .unwrap()
                .is_err()
        );
        first.release(&memory).await;
        assert!(
            reserve_visitor(&memory, "v", now, &limits)
                .await
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn the_budget_reserves_atomically_so_two_turns_cannot_overspend() {
        let memory = std::sync::Arc::new(Memory::default());
        let now = at("2026-11-17T10:00:00Z");
        // Room for exactly one reservation of 600 out of 1000, tried by 8 turns at once.
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let memory = memory.clone();
            tasks.push(tokio::spawn(async move {
                reserve_budget(&*memory, now, 600, 1000).await.unwrap()
            }));
        }
        let mut won = 0;
        for task in tasks {
            if task.await.unwrap().is_some() {
                won += 1;
            }
        }
        assert_eq!(won, 1, "exactly one reservation fits");
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 600);
        // A smaller one still fits in the rest.
        assert!(
            reserve_budget(&*memory, now, 400, 1000)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            reserve_budget(&*memory, now, 1, 1000)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn settling_refunds_the_unused_part_and_charges_an_overrun() {
        let memory = Memory::default();
        let now = at("2026-11-17T10:00:00Z");
        let held = reserve_budget(&memory, now, 600, 1000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held.reserved_micro_usd(), 600);
        held.settle(&memory, 150).await;
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 150);
        // The refund made room for another.
        let second = reserve_budget(&memory, now, 800, 1000)
            .await
            .unwrap()
            .unwrap();
        // This call cost more than it reserved: the difference is charged.
        second.settle(&memory, 900).await;
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 150 + 900);
        // The overrun used the room: nothing more fits today.
        assert!(
            reserve_budget(&memory, now, 50, 1000)
                .await
                .unwrap()
                .is_none()
        );
        // A call that cost nothing gives everything back.
        let other = at("2026-11-18T10:00:00Z");
        let third = reserve_budget(&memory, other, 50, 1000)
            .await
            .unwrap()
            .unwrap();
        third.settle(&memory, 0).await;
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-18"), 0);
        // A reservation above the whole budget never fits.
        assert!(
            reserve_budget(&memory, now, 1001, 1000)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_budget_starts_again_every_utc_day() {
        let memory = Memory::default();
        let late = at("2026-11-17T23:59:59Z");
        let held = reserve_budget(&memory, late, 1000, 1000)
            .await
            .unwrap()
            .unwrap();
        assert!(
            reserve_budget(&memory, late, 1, 1000)
                .await
                .unwrap()
                .is_none()
        );
        // One second later it is a new day with a full budget.
        let next = at("2026-11-18T00:00:00Z");
        assert!(
            reserve_budget(&memory, next, 1000, 1000)
                .await
                .unwrap()
                .is_some()
        );
        // A turn that began before midnight settles on its own day.
        held.settle(&memory, 100).await;
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 100);
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-18"), 1000);
    }

    #[test]
    fn prices_and_bounds() {
        assert_eq!(usd_to_micro(2.0), Some(2_000_000));
        assert_eq!(usd_to_micro(0.5), Some(500_000));
        assert_eq!(usd_to_micro(0.0), None);
        assert_eq!(usd_to_micro(f64::NAN), None);
        assert_eq!(worst_case_tokens(9), 3);
        assert_eq!(worst_case_tokens(10), 4);
    }

    #[test]
    fn retry_after_points_at_the_end_of_the_window_or_the_day() {
        let now = at("2026-11-17T10:03:30Z");
        assert_eq!(LimitHit::VisitorWindow.retry_after_seconds(now), 390);
        assert_eq!(
            LimitHit::VisitorDay.retry_after_seconds(now),
            13 * 3600 + 56 * 60 + 30
        );
        assert_eq!(
            LimitHit::VisitorWindow.retry_after_seconds(at("2026-11-17T10:10:00Z")),
            600
        );
        assert_eq!(
            LimitHit::VisitorDay.retry_after_seconds(at("2026-11-17T23:59:59Z")),
            1
        );
    }

    #[test]
    fn counters_expire() {
        let now = at("2026-11-17T10:03:00Z");
        let r = VisitorReservation::new("v", now);
        assert!(r.window.expires_at > now.timestamp() + WINDOW_SECONDS);
        assert!(r.window.expires_at <= now.timestamp() + 3 * WINDOW_SECONDS);
        assert_eq!(r.day.expires_at, now.timestamp() + DAY_ITEM_TTL_SECONDS);
        assert_eq!(budget_counter(now).expires_at, r.day.expires_at);
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
    fn conversation_ids_have_enough_randomness_and_a_strict_shape() {
        let a = new_conversation_id().unwrap();
        let b = new_conversation_id().unwrap();
        assert_eq!(a.len(), 48, "192 bits as hex");
        assert_ne!(a, b);
        assert!(is_conversation_id(&a));
        for bad in [
            "",
            "abc",
            &a.to_uppercase(),
            &format!("{a}0"),
            &"g".repeat(48),
            "../../etc/passwd",
        ] {
            assert!(!is_conversation_id(bad), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn a_conversation_continues_until_it_is_unknown_expired_or_full() {
        let store = Memory::default();
        let now = at("2026-11-17T10:00:00Z");
        // No ID: fresh.
        let first = resume(&store, None, now).await.unwrap();
        assert!(first.history.is_empty());
        assert_eq!(first.expires_at, now.timestamp() + CONVERSATION_TTL_SECONDS);
        assert!(
            store
                .append(&first.id, "q1", "a1", first.expires_at)
                .await
                .unwrap()
        );
        // The ID continues it, with the stored messages.
        let again = resume(&store, Some(&first.id), now).await.unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.history.len(), 2);
        assert_eq!(again.history[0], (MessageRole::User, "q1".to_owned()));
        // An unknown ID starts fresh with a new ID, and nothing of the old one.
        let unknown = new_conversation_id().unwrap();
        let fresh = resume(&store, Some(&unknown), now).await.unwrap();
        assert_ne!(fresh.id, unknown);
        assert!(fresh.history.is_empty());
        // A bad shape is never looked up.
        let junk = resume(&store, Some("../x"), now).await.unwrap();
        assert!(junk.history.is_empty());
        // After 24 hours it has expired.
        let late = now + chrono::Duration::hours(24) + chrono::Duration::seconds(1);
        let expired = resume(&store, Some(&first.id), late).await.unwrap();
        assert_ne!(expired.id, first.id);
        assert!(expired.history.is_empty());
        // Ten turns is the most. The eleventh question starts a fresh conversation.
        for turn in 2..=MAX_TURNS {
            assert!(
                store
                    .append(&first.id, &format!("q{turn}"), "a", first.expires_at)
                    .await
                    .unwrap()
            );
        }
        assert!(
            !store
                .append(&first.id, "q11", "a", first.expires_at)
                .await
                .unwrap()
        );
        let full = resume(&store, Some(&first.id), now).await.unwrap();
        assert_ne!(full.id, first.id);
        assert!(full.history.is_empty());
    }
}
