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

/// The address that stands for one visitor. An IPv6 visitor has a whole `/56` (providers give a home
/// a `/56` to a `/64`), so the address is cut to that prefix. An IPv4-mapped IPv6 address is the IPv4 address.
pub fn visitor_address(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut octets = v6.octets();
                octets[7..].fill(0);
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
    /// The visitor used the dollar cap of today.
    VisitorBudget,
}

impl LimitHit {
    /// How long until this visitor can ask again: the end of the 10-minute window, or midnight UTC.
    /// Always at least 1.
    pub fn retry_after_seconds(self, now: DateTime<Utc>) -> u64 {
        let ts = now.timestamp();
        let seconds = match self {
            LimitHit::VisitorWindow => (ts.div_euclid(WINDOW_SECONDS) + 1) * WINDOW_SECONDS - ts,
            LimitHit::VisitorDay | LimitHit::VisitorBudget => 86_400 - ts.rem_euclid(86_400),
        };
        u64::try_from(seconds.max(1)).unwrap_or(1)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LimitHit::VisitorWindow => "visitor_window",
            LimitHit::VisitorDay => "visitor_day",
            LimitHit::VisitorBudget => "visitor_budget",
        }
    }
}

/// The counters behind the question limits. `Store` implements it with DynamoDB.
#[async_trait]
pub trait DemoCounters: Send + Sync {
    /// Adds `amount` to the counter when the result stays at or below `limit`. One atomic step.
    /// Returns whether it added.
    async fn reserve(&self, counter: &Counter, amount: u64, limit: u64)
    -> Result<bool, StoreError>;
    /// Takes `amount` back. It is an error when the counter holds less than `amount`: the caller
    /// logs it, and nothing is changed.
    async fn release(&self, counter: &Counter, amount: u64) -> Result<(), StoreError>;
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

/// Which budget has no room for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetHit {
    /// The whole demo used its budget of the day.
    Global,
    /// This visitor used the dollar cap of the day.
    Visitor,
}

/// The money of one turn: a reservation on the global counter of the day and on the visitor's
/// counter of the day, with one item for the turn that makes the settlement happen once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnReservation {
    /// A random ID for the turn. It is the idempotency token of the reservation and of the settlement.
    pub turn_id: String,
    pub global: Counter,
    pub visitor: Counter,
    /// The reserved cost in millionths of a US dollar.
    pub amount: u64,
    pub global_limit: u64,
    pub visitor_limit: u64,
    /// The item that says "this turn holds a reservation". The settlement deletes it.
    pub item: Counter,
}

/// The budget counters. `Store` implements it with DynamoDB transactions.
#[async_trait]
pub trait DemoBudgets: Send + Sync {
    /// In one transaction: adds `amount` to both counters when each stays at or below its limit, and
    /// writes the item of the turn. The same turn ID gives the same answer again (a retry).
    async fn reserve_turn(
        &self,
        turn: &TurnReservation,
    ) -> Result<Result<(), BudgetHit>, StoreError>;
    /// In one transaction: deletes the item of the turn and takes `refund` back from both counters.
    /// Returns false (and changes nothing) when the item is already gone: the turn was settled before.
    async fn settle_turn(&self, turn: &TurnReservation, refund: u64) -> Result<bool, StoreError>;
}

/// The money that one turn may still spend. It is not `Clone`: `settle` takes it, so a turn settles
/// once in the code too.
#[derive(Debug, PartialEq, Eq)]
pub struct BudgetReservation {
    turn: TurnReservation,
}

impl BudgetReservation {
    pub fn reserved_micro_usd(&self) -> u64 {
        self.turn.amount
    }

    /// Replaces the reservation with the real cost: it gives back the part that the turn did not use.
    /// A cost above the reservation is a bug of the estimate: it is logged, and the reservation is
    /// kept as the charge, so the counter never passes the budget.
    pub async fn settle(self, budgets: &dyn DemoBudgets, actual_micro_usd: u64) {
        let reserved = self.turn.amount;
        if actual_micro_usd > reserved {
            tracing::error!(
                reserved,
                actual = actual_micro_usd,
                "a demo turn cost more than its reservation"
            );
        }
        let refund = reserved.saturating_sub(actual_micro_usd);
        // A transaction that conflicts with another one is cancelled, and the SDK does not retry it.
        for attempt in 0..CONFLICT_ATTEMPTS {
            match budgets.settle_turn(&self.turn, refund).await {
                Ok(true) => return,
                Ok(false) => {
                    tracing::warn!("a demo turn was settled before");
                    return;
                }
                Err(StoreError::Conflict) if attempt + 1 < CONFLICT_ATTEMPTS => {
                    conflict_backoff(attempt).await;
                }
                Err(StoreError::Conflict) => {
                    // The reservation stays: the money is kept, never given back by mistake.
                    tracing::error!(
                        attempts = CONFLICT_ATTEMPTS,
                        "the demo settlement kept conflicting; the reservation stays"
                    );
                }
                Err(error) => {
                    tracing::error!(%error, "could not settle the demo budget");
                    return;
                }
            }
        }
    }

    /// Keeps the whole reservation as the cost. A turn does this when it cannot know what the model
    /// charged: a stream that failed, or a model that gave no usage. It never refunds on an estimate.
    pub async fn keep(self, budgets: &dyn DemoBudgets) {
        let reserved = self.turn.amount;
        self.settle(budgets, reserved).await;
    }
}

/// How often a reservation or a settlement is tried when transactions conflict.
const CONFLICT_ATTEMPTS: u32 = 5;

/// A short, jittered pause before the next try: 10 ms, 20 ms, ... plus up to the same again.
async fn conflict_backoff(attempt: u32) {
    let mut random = [0u8; 1];
    let _ = getrandom::fill(&mut random);
    let base = 10 * u64::from(attempt + 1);
    let jitter = u64::from(random[0]) % (base + 1);
    tokio::time::sleep(std::time::Duration::from_millis(base + jitter)).await;
}

/// The global counter of a day: the money in millionths of a US dollar.
pub fn budget_counter(now: DateTime<Utc>) -> Counter {
    Counter {
        pk: "DEMO#GLOBAL".to_owned(),
        sk: format!("D#{}", usage_date(now)),
        expires_at: now.timestamp() + DAY_ITEM_TTL_SECONDS,
    }
}

/// The dollar counter of one visitor for a day.
pub fn visitor_budget_counter(visitor: &str, now: DateTime<Utc>) -> Counter {
    Counter {
        pk: format!("DEMO#V#{visitor}"),
        sk: format!("B#{}", usage_date(now)),
        expires_at: now.timestamp() + DAY_ITEM_TTL_SECONDS,
    }
}

/// Reserves `amount` of the day's budget and of the visitor's cap. `Err(hit)` says which does not fit.
pub async fn reserve_budget(
    budgets: &dyn DemoBudgets,
    visitor: &str,
    now: DateTime<Utc>,
    amount_micro_usd: u64,
    limits: &DemoConfig,
) -> Result<Result<BudgetReservation, BudgetHit>, StoreError> {
    if amount_micro_usd > limits.budget_micro_usd_per_day {
        return Ok(Err(BudgetHit::Global));
    }
    if amount_micro_usd > limits.visitor_budget_micro_usd_per_day {
        return Ok(Err(BudgetHit::Visitor));
    }
    // Every try has a new turn ID, so a cancelled try leaves nothing behind.
    for attempt in 0..CONFLICT_ATTEMPTS {
        let turn_id = new_turn_id()?;
        let turn = TurnReservation {
            item: Counter {
                pk: format!("DEMO#RES#{turn_id}"),
                sk: "R".to_owned(),
                // A turn ends in 2 minutes. If the server dies, the item expires after the day.
                expires_at: now.timestamp() + DAY_ITEM_TTL_SECONDS,
            },
            turn_id,
            global: budget_counter(now),
            visitor: visitor_budget_counter(visitor, now),
            amount: amount_micro_usd,
            global_limit: limits.budget_micro_usd_per_day,
            visitor_limit: limits.visitor_budget_micro_usd_per_day,
        };
        match budgets.reserve_turn(&turn).await {
            // Another transaction runs on the same counter. The caller turns a last conflict into `demo_busy`.
            Err(StoreError::Conflict) if attempt + 1 < CONFLICT_ATTEMPTS => {
                conflict_backoff(attempt).await;
            }
            other => return Ok(other?.map(|()| BudgetReservation { turn })),
        }
    }
    Err(StoreError::Conflict)
}

fn new_turn_id() -> Result<String, StoreError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| StoreError::Internal(format!("no random bytes: {e}")))?;
    Ok(hex(&bytes))
}

/// An upper bound of the tokens in a text that the server did not count. ASCII letters and spaces
/// are at most one token per 3 characters. Digits, punctuation, and symbols (LaTeX) are one token per
/// character. Any other character is counted by its UTF-8 bytes (up to 4), because a byte-level
/// tokenizer can spend one token on each byte of a rare character or an emoji.
pub fn token_bound(text: &str) -> u64 {
    let (mut plain, mut dense) = (0u64, 0u64);
    for c in text.chars() {
        if c.is_ascii_alphabetic() || c == ' ' || c == '\n' {
            plain += 1;
        } else {
            dense += c.len_utf8() as u64;
        }
    }
    plain.div_ceil(3) + dense
}

/// An upper bound of the tokens in `chars` characters of course text (the sources of the prompt).
/// The text is not known before retrieval, so the bound is one token per 2 characters, which holds
/// for English and math text and is far above the usual 1 per 4.
pub fn source_token_bound(chars: usize) -> u64 {
    (chars as u64).div_ceil(2)
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

/// One stored message of a demo conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMessage {
    pub role: MessageRole,
    pub text: String,
    /// For an answer: the output tokens that the model reported. The next turn counts the history
    /// with them.
    pub output_tokens: Option<u64>,
}

/// A stored demo conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConversation {
    /// Questions and answers so far.
    pub turns: u32,
    /// Epoch seconds. After this the conversation does not exist, although DynamoDB may keep the item.
    pub expires_at: i64,
    /// The messages, oldest first.
    pub messages: Vec<StoredMessage>,
}

/// Where demo conversations live. `Store` implements it with DynamoDB.
#[async_trait]
pub trait DemoConversations: Send + Sync {
    async fn load(&self, id: &str) -> Result<Option<StoredConversation>, StoreError>;
    /// Adds one turn. It returns false (and writes nothing) when the conversation already has
    /// `MAX_TURNS` turns. `answer_tokens` are the output tokens that the model reported.
    async fn append(
        &self,
        id: &str,
        question: &str,
        answer: &str,
        answer_tokens: u64,
        expires_at: i64,
    ) -> Result<bool, StoreError>;
}

/// The conversation that a request continues, or a fresh one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resumed {
    pub id: String,
    /// The earlier messages, oldest first. Empty for a fresh conversation.
    pub history: Vec<(MessageRole, String)>,
    /// An upper bound of the tokens of `history`: the real output tokens of the stored answers, and
    /// `token_bound` of the questions.
    pub history_tokens: u64,
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
        let history_tokens = found
            .messages
            .iter()
            .map(|m| match (m.role, m.output_tokens) {
                (MessageRole::Assistant, Some(tokens)) => tokens,
                _ => token_bound(&m.text),
            })
            .sum();
        return Ok(Resumed {
            id: id.to_owned(),
            history: found
                .messages
                .into_iter()
                .map(|m| (m.role, m.text))
                .collect(),
            history_tokens,
            expires_at: found.expires_at,
        });
    }
    Ok(Resumed {
        id: new_conversation_id()?,
        history: Vec::new(),
        history_tokens: 0,
        expires_at: now.timestamp() + CONVERSATION_TTL_SECONDS,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    /// Counters in memory. The mutex makes each step atomic, like a conditional update.
    #[derive(Default)]
    struct Memory {
        counts: Mutex<HashMap<(String, String), u64>>,
        open: Mutex<HashSet<String>>,
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
            let slot = counts.entry((c.pk.clone(), c.sk.clone())).or_insert(0);
            if *slot < amount {
                return Err(StoreError::Internal("counter below the amount".into()));
            }
            *slot -= amount;
            Ok(())
        }
    }

    #[async_trait]
    impl DemoBudgets for Memory {
        async fn reserve_turn(
            &self,
            t: &TurnReservation,
        ) -> Result<Result<(), BudgetHit>, StoreError> {
            let mut counts = self.counts.lock().unwrap();
            let global = *counts
                .get(&(t.global.pk.clone(), t.global.sk.clone()))
                .unwrap_or(&0);
            let visitor = *counts
                .get(&(t.visitor.pk.clone(), t.visitor.sk.clone()))
                .unwrap_or(&0);
            if global + t.amount > t.global_limit {
                return Ok(Err(BudgetHit::Global));
            }
            if visitor + t.amount > t.visitor_limit {
                return Ok(Err(BudgetHit::Visitor));
            }
            counts.insert(
                (t.global.pk.clone(), t.global.sk.clone()),
                global + t.amount,
            );
            counts.insert(
                (t.visitor.pk.clone(), t.visitor.sk.clone()),
                visitor + t.amount,
            );
            self.open.lock().unwrap().insert(t.turn_id.clone());
            Ok(Ok(()))
        }
        async fn settle_turn(&self, t: &TurnReservation, refund: u64) -> Result<bool, StoreError> {
            if !self.open.lock().unwrap().remove(&t.turn_id) {
                return Ok(false);
            }
            let mut counts = self.counts.lock().unwrap();
            for c in [&t.global, &t.visitor] {
                let slot = counts.entry((c.pk.clone(), c.sk.clone())).or_insert(0);
                *slot = slot.saturating_sub(refund);
            }
            Ok(true)
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
            answer_tokens: u64,
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
            conversation.messages.push(StoredMessage {
                role: MessageRole::User,
                text: question.to_owned(),
                output_tokens: None,
            });
            conversation.messages.push(StoredMessage {
                role: MessageRole::Assistant,
                text: answer.to_owned(),
                output_tokens: Some(answer_tokens),
            });
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
            visitor_budget_micro_usd_per_day: 300_000,
            max_output_tokens: 1000,
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
        assert!(
            reserve_visitor(&memory, "v2", now, &limits())
                .await
                .unwrap()
                .is_ok()
        );
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

    fn small() -> DemoConfig {
        DemoConfig {
            budget_micro_usd_per_day: 1000,
            visitor_budget_micro_usd_per_day: 800,
            ..limits()
        }
    }

    #[tokio::test]
    async fn the_budget_reserves_atomically_so_turns_at_once_cannot_overspend() {
        let memory = std::sync::Arc::new(Memory::default());
        let now = at("2026-11-17T10:00:00Z");
        // Room for one reservation of 600 out of 1000, tried by 8 visitors at once.
        let mut tasks = Vec::new();
        for i in 0..8 {
            let memory = memory.clone();
            tasks.push(tokio::spawn(async move {
                reserve_budget(&*memory, &format!("v{i}"), now, 600, &small())
                    .await
                    .unwrap()
                    .ok()
            }));
        }
        let mut held = Vec::new();
        for task in tasks {
            held.extend(task.await.unwrap());
        }
        assert_eq!(held.len(), 1, "exactly one reservation fits");
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 600);
        assert_eq!(
            reserve_budget(&*memory, "other", now, 600, &small())
                .await
                .unwrap()
                .unwrap_err(),
            BudgetHit::Global
        );
        assert!(
            reserve_budget(&*memory, "other", now, 400, &small())
                .await
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn one_visitor_cannot_use_up_the_day() {
        let memory = Memory::default();
        let now = at("2026-11-17T10:00:00Z");
        let first = reserve_budget(&memory, "greedy", now, 500, &small())
            .await
            .unwrap()
            .unwrap();
        // The visitor cap is 800: a second 500 does not fit for the visitor, though the day has room.
        assert_eq!(
            reserve_budget(&memory, "greedy", now, 500, &small())
                .await
                .unwrap()
                .unwrap_err(),
            BudgetHit::Visitor
        );
        // A refused reservation changes nothing, and another visitor still has the day.
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 500);
        assert!(
            reserve_budget(&memory, "polite", now, 300, &small())
                .await
                .unwrap()
                .is_ok()
        );
        // Settling the first one makes room for the visitor again.
        first.settle(&memory, 100).await;
        assert_eq!(memory.count("DEMO#V#greedy", "B#2026-11-17"), 100);
        assert!(
            reserve_budget(&memory, "greedy", now, 500, &small())
                .await
                .unwrap()
                .is_ok()
        );
        // A reservation above a whole cap never fits.
        assert_eq!(
            reserve_budget(&memory, "x", now, 900, &small())
                .await
                .unwrap()
                .unwrap_err(),
            BudgetHit::Visitor
        );
        assert_eq!(
            reserve_budget(&memory, "x", now, 1100, &small())
                .await
                .unwrap()
                .unwrap_err(),
            BudgetHit::Global
        );
    }

    #[tokio::test]
    async fn settling_refunds_the_unused_part_once() {
        let memory = Memory::default();
        let now = at("2026-11-17T10:00:00Z");
        let held = reserve_budget(&memory, "v", now, 600, &small())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held.reserved_micro_usd(), 600);
        // A copy of the turn, to try a second settlement (a retry after a lost answer).
        let again = TurnReservation {
            ..held.turn.clone()
        };
        held.settle(&memory, 150).await;
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 150);
        assert_eq!(memory.count("DEMO#V#v", "B#2026-11-17"), 150);
        // No double refund.
        assert!(!memory.settle_turn(&again, 450).await.unwrap());
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 150);
    }

    #[tokio::test]
    async fn keeping_the_reservation_never_refunds_on_an_estimate() {
        let memory = Memory::default();
        let now = at("2026-11-17T10:00:00Z");
        let held = reserve_budget(&memory, "v", now, 600, &small())
            .await
            .unwrap()
            .unwrap();
        held.keep(&memory).await;
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 600);
        assert_eq!(memory.count("DEMO#V#v", "B#2026-11-17"), 600);
        // The item of the turn is gone, so the money is final.
        assert!(memory.open.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_cost_above_the_reservation_is_capped_and_never_passes_the_budget() {
        let memory = Memory::default();
        let now = at("2026-11-17T10:00:00Z");
        let held = reserve_budget(&memory, "v", now, 600, &small())
            .await
            .unwrap()
            .unwrap();
        held.settle(&memory, 5000).await;
        assert_eq!(
            memory.count("DEMO#GLOBAL", "D#2026-11-17"),
            600,
            "capped at the reservation"
        );
    }

    #[tokio::test]
    async fn the_budget_starts_again_every_utc_day() {
        let memory = Memory::default();
        let late = at("2026-11-17T23:59:59Z");
        let held = reserve_budget(&memory, "v", late, 1000, &limits_one_visitor_big())
            .await
            .unwrap()
            .unwrap();
        assert!(
            reserve_budget(&memory, "w", late, 1, &limits_one_visitor_big())
                .await
                .unwrap()
                .is_err()
        );
        let next = at("2026-11-18T00:00:00Z");
        assert!(
            reserve_budget(&memory, "w", next, 1000, &limits_one_visitor_big())
                .await
                .unwrap()
                .is_ok()
        );
        // A turn that began before midnight settles on its own day.
        held.settle(&memory, 100).await;
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-17"), 100);
        assert_eq!(memory.count("DEMO#GLOBAL", "D#2026-11-18"), 1000);
    }

    fn limits_one_visitor_big() -> DemoConfig {
        DemoConfig {
            budget_micro_usd_per_day: 1000,
            visitor_budget_micro_usd_per_day: 1000,
            ..limits()
        }
    }

    /// A store whose transactions conflict a number of times before they work.
    struct Conflicting {
        conflicts: u32,
        calls: Mutex<u32>,
        settled: Mutex<u32>,
    }

    #[async_trait]
    impl DemoBudgets for Conflicting {
        async fn reserve_turn(
            &self,
            _t: &TurnReservation,
        ) -> Result<Result<(), BudgetHit>, StoreError> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if *calls <= self.conflicts {
                return Err(StoreError::Conflict);
            }
            Ok(Ok(()))
        }
        async fn settle_turn(
            &self,
            _t: &TurnReservation,
            _refund: u64,
        ) -> Result<bool, StoreError> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if *calls <= self.conflicts {
                return Err(StoreError::Conflict);
            }
            *self.settled.lock().unwrap() += 1;
            Ok(true)
        }
    }

    fn conflicting(conflicts: u32) -> Conflicting {
        Conflicting {
            conflicts,
            calls: Mutex::new(0),
            settled: Mutex::new(0),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_reservation_tries_again_after_a_transaction_conflict() {
        let now = at("2026-11-17T10:00:00Z");
        let store = conflicting(3);
        let held = reserve_budget(&store, "v", now, 100, &limits_one_visitor_big())
            .await
            .unwrap();
        assert!(held.is_ok(), "the fourth try works");
        assert_eq!(*store.calls.lock().unwrap(), 4);
        // After five conflicts in a row, the caller gets the conflict and turns it into demo_busy.
        let store = conflicting(100);
        let error = reserve_budget(&store, "v", now, 100, &limits_one_visitor_big())
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::Conflict));
        assert_eq!(
            *store.calls.lock().unwrap(),
            5,
            "three to five tries, not a loop without end"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_settlement_tries_again_and_logs_a_final_conflict() {
        let now = at("2026-11-17T10:00:00Z");
        let ok = conflicting(0);
        let held = reserve_budget(&ok, "v", now, 100, &limits_one_visitor_big())
            .await
            .unwrap()
            .unwrap();
        let store = conflicting(2);
        held.settle(&store, 10).await;
        assert_eq!(
            *store.settled.lock().unwrap(),
            1,
            "settled after two conflicts, once"
        );
        // A final conflict gives up after five tries and keeps the reservation (nothing is settled).
        let held = reserve_budget(&ok, "v", now, 100, &limits_one_visitor_big())
            .await
            .unwrap()
            .unwrap();
        let store = conflicting(100);
        held.settle(&store, 10).await;
        assert_eq!(*store.calls.lock().unwrap(), 5);
        assert_eq!(*store.settled.lock().unwrap(), 0);
    }

    #[test]
    fn token_bounds_are_safe_for_dense_and_non_latin_text() {
        // Plain English: one token per 3 characters at most.
        assert_eq!(token_bound("abc def"), 3);
        // Digits, punctuation, and LaTeX symbols: one token per character.
        assert_eq!(token_bound("1234567890"), 10);
        assert_eq!(token_bound("$$x^2+y^2=z^2$$"), 1 + 12);
        // Other characters count by their UTF-8 bytes: 3 for Japanese, 4 for an emoji, 2 for an accent.
        assert_eq!(token_bound("日本語のテキスト"), 8 * 3);
        assert_eq!(token_bound("😀😀😀"), 12);
        assert_eq!(token_bound("é"), 2);
        assert_eq!(token_bound(""), 0);
        assert_eq!(source_token_bound(2400), 1200);
        assert_eq!(usd_to_micro(2.0), Some(2_000_000));
        assert_eq!(usd_to_micro(0.3), Some(300_000));
        assert_eq!(usd_to_micro(0.0), None);
        assert_eq!(usd_to_micro(f64::NAN), None);
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
        assert_eq!(visitor_budget_counter("v", now).sk, "B#2026-11-17");
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
        assert_eq!(memory.id(&store, day1, ip).await.unwrap(), a);
        assert_eq!(*store.salt_calls.lock().unwrap(), 1);
        let other: IpAddr = "203.0.113.8".parse().unwrap();
        assert_ne!(memory.id(&store, day1, other).await.unwrap(), a);
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
    fn an_ipv6_visitor_is_a_whole_slash_56() {
        let salt = [7u8; 32];
        let same = |a: &str, b: &str| {
            visitor_hash(&salt, a.parse().unwrap()) == visitor_hash(&salt, b.parse().unwrap())
        };
        // Same /56 (the first 7 bytes), different /64: the same visitor.
        assert!(same("2001:db8:1:2:aaaa::1", "2001:db8:1:2:1111::2"));
        assert!(same("2001:db8:1:2::1", "2001:db8:1:ff::1"));
        // Another /56: another visitor.
        assert!(!same("2001:db8:1:2::1", "2001:db8:1:100::1"));
        assert!(!same("2001:db8:1:2::1", "2001:db8:2:2::1"));
        // An IPv4-mapped address is the IPv4 address.
        assert!(same("::ffff:203.0.113.7", "203.0.113.7"));
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
        let first = resume(&store, None, now).await.unwrap();
        assert!(first.history.is_empty());
        assert_eq!(first.history_tokens, 0);
        assert_eq!(first.expires_at, now.timestamp() + CONVERSATION_TTL_SECONDS);
        assert!(
            store
                .append(&first.id, "q1", "a1", 37, first.expires_at)
                .await
                .unwrap()
        );
        // The ID continues it, with the stored messages and a token bound that uses the real output.
        let again = resume(&store, Some(&first.id), now).await.unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.history.len(), 2);
        assert_eq!(again.history[0], (MessageRole::User, "q1".to_owned()));
        assert_eq!(again.history_tokens, token_bound("q1") + 37);
        let unknown = new_conversation_id().unwrap();
        let fresh = resume(&store, Some(&unknown), now).await.unwrap();
        assert_ne!(fresh.id, unknown);
        assert!(fresh.history.is_empty());
        let junk = resume(&store, Some("../x"), now).await.unwrap();
        assert!(junk.history.is_empty());
        let late = now + chrono::Duration::hours(24) + chrono::Duration::seconds(1);
        let expired = resume(&store, Some(&first.id), late).await.unwrap();
        assert_ne!(expired.id, first.id);
        for turn in 2..=MAX_TURNS {
            assert!(
                store
                    .append(&first.id, &format!("q{turn}"), "a", 1, first.expires_at)
                    .await
                    .unwrap()
            );
        }
        assert!(
            !store
                .append(&first.id, "q11", "a", 1, first.expires_at)
                .await
                .unwrap()
        );
        let full = resume(&store, Some(&first.id), now).await.unwrap();
        assert_ne!(full.id, first.id);
        assert!(full.history.is_empty());
    }
}
