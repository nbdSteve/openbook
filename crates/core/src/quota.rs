//! Daily message quotas.
//!
//! Before the model call, the API reserves one message on the user's daily item, then on the global
//! daily item. If the global reservation fails, the user reservation goes back. After the call, token
//! counts are added to the daily, monthly, and global items.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::store::StoreError;

/// Whose counter to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageScope {
    User(String),
    Global,
}

/// The counters behind the quotas. `Store` implements it with DynamoDB.
#[async_trait]
pub trait UsageCounter: Send + Sync {
    /// Adds one message to the daily counter unless it is at `limit`. Returns whether it reserved.
    async fn reserve(&self, scope: &UsageScope, date: &str, limit: u32)
    -> Result<bool, StoreError>;
    /// Input plus output tokens that the whole deployment used on `date`.
    async fn global_tokens(&self, date: &str) -> Result<u64, StoreError>;
    /// Gives one reserved message back.
    async fn release(&self, scope: &UsageScope, date: &str) -> Result<(), StoreError>;
}

/// The limits that a reservation checks.
#[derive(Debug, Clone, Copy)]
pub struct ReserveLimits {
    pub user_messages: u32,
    pub global_messages: u32,
    pub global_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reservation {
    Reserved,
    /// The user used all of today's messages (HTTP 429).
    UserLimit,
    /// The global daily cap is reached (HTTP 503).
    GlobalCap,
}

/// Reserves one message for `user_id`.
/// The global token cap is checked first (read only), then the user counter, then the global counter.
pub async fn reserve_message(
    counter: &dyn UsageCounter,
    user_id: &str,
    date: &str,
    limits: ReserveLimits,
) -> Result<Reservation, StoreError> {
    let ReserveLimits {
        user_messages: user_limit,
        global_messages: global_limit,
        global_tokens: token_cap,
    } = limits;
    if counter.global_tokens(date).await? >= token_cap {
        return Ok(Reservation::GlobalCap);
    }
    let user = UsageScope::User(user_id.to_owned());
    if !counter.reserve(&user, date, user_limit).await? {
        return Ok(Reservation::UserLimit);
    }
    match counter
        .reserve(&UsageScope::Global, date, global_limit)
        .await
    {
        Ok(true) => Ok(Reservation::Reserved),
        Ok(false) => {
            counter.release(&user, date).await?;
            Ok(Reservation::GlobalCap)
        }
        Err(e) => {
            if let Err(release_error) = counter.release(&user, date).await {
                tracing::error!(%release_error, "could not give back the user reservation");
            }
            Err(e)
        }
    }
}

/// Gives back a reservation (both counters) when the model produced nothing.
pub async fn release_message(counter: &dyn UsageCounter, user_id: &str, date: &str) {
    for scope in [UsageScope::User(user_id.to_owned()), UsageScope::Global] {
        if let Err(error) = counter.release(&scope, date).await {
            tracing::error!(%error, ?scope, "could not give back a reservation");
        }
    }
}

/// `YYYY-MM-DD` in UTC.
pub fn usage_date(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d").to_string()
}

/// `YYYY-MM` in UTC.
pub fn usage_month(at: DateTime<Utc>) -> String {
    at.format("%Y-%m").to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Memory {
        counts: Mutex<HashMap<UsageKey, i64>>,
        fail_global: bool,
        tokens: u64,
    }

    fn limits(user: u32, global: u32) -> ReserveLimits {
        ReserveLimits {
            user_messages: user,
            global_messages: global,
            global_tokens: u64::MAX,
        }
    }

    #[derive(Hash, PartialEq, Eq, Clone)]
    struct UsageKey(String);

    fn key(scope: &UsageScope, date: &str) -> UsageKey {
        match scope {
            UsageScope::User(id) => UsageKey(format!("U#{id}#{date}")),
            UsageScope::Global => UsageKey(format!("G#{date}")),
        }
    }

    impl Memory {
        fn count(&self, scope: &UsageScope, date: &str) -> i64 {
            *self
                .counts
                .lock()
                .unwrap()
                .get(&key(scope, date))
                .unwrap_or(&0)
        }
    }

    #[async_trait]
    impl UsageCounter for Memory {
        async fn reserve(
            &self,
            scope: &UsageScope,
            date: &str,
            limit: u32,
        ) -> Result<bool, StoreError> {
            if self.fail_global && *scope == UsageScope::Global {
                return Err(StoreError::Internal("boom".into()));
            }
            let mut counts = self.counts.lock().unwrap();
            let n = counts.entry(key(scope, date)).or_insert(0);
            if *n < i64::from(limit) {
                *n += 1;
                Ok(true)
            } else {
                Ok(false)
            }
        }
        async fn global_tokens(&self, _date: &str) -> Result<u64, StoreError> {
            Ok(self.tokens)
        }
        async fn release(&self, scope: &UsageScope, date: &str) -> Result<(), StoreError> {
            *self
                .counts
                .lock()
                .unwrap()
                .entry(key(scope, date))
                .or_insert(0) -= 1;
            Ok(())
        }
    }

    const D: &str = "2026-11-16";

    #[tokio::test]
    async fn reserves_both_counters() {
        let m = Memory::default();
        assert_eq!(
            reserve_message(&m, "u", D, limits(2, 10)).await.unwrap(),
            Reservation::Reserved
        );
        assert_eq!(m.count(&UsageScope::User("u".into()), D), 1);
        assert_eq!(m.count(&UsageScope::Global, D), 1);
    }

    #[tokio::test]
    async fn user_limit_stops_before_the_global_counter() {
        let m = Memory::default();
        for _ in 0..2 {
            reserve_message(&m, "u", D, limits(2, 10)).await.unwrap();
        }
        assert_eq!(
            reserve_message(&m, "u", D, limits(2, 10)).await.unwrap(),
            Reservation::UserLimit
        );
        assert_eq!(m.count(&UsageScope::User("u".into()), D), 2);
        assert_eq!(
            m.count(&UsageScope::Global, D),
            2,
            "global counter must not change"
        );
    }

    #[tokio::test]
    async fn global_cap_gives_the_user_reservation_back() {
        let m = Memory::default();
        assert_eq!(
            reserve_message(&m, "a", D, limits(5, 1)).await.unwrap(),
            Reservation::Reserved
        );
        assert_eq!(
            reserve_message(&m, "b", D, limits(5, 1)).await.unwrap(),
            Reservation::GlobalCap
        );
        assert_eq!(m.count(&UsageScope::User("b".into()), D), 0);
        assert_eq!(m.count(&UsageScope::Global, D), 1);
    }

    #[tokio::test]
    async fn global_error_gives_the_user_reservation_back() {
        let m = Memory {
            fail_global: true,
            ..Memory::default()
        };
        assert!(reserve_message(&m, "u", D, limits(5, 5)).await.is_err());
        assert_eq!(m.count(&UsageScope::User("u".into()), D), 0);
    }

    #[tokio::test]
    async fn token_cap_stops_before_any_reservation() {
        let m = Memory {
            tokens: 1000,
            ..Memory::default()
        };
        let capped = ReserveLimits {
            global_tokens: 1000,
            ..limits(5, 5)
        };
        assert_eq!(
            reserve_message(&m, "u", D, capped).await.unwrap(),
            Reservation::GlobalCap
        );
        assert_eq!(m.count(&UsageScope::User("u".into()), D), 0);
        let roomy = ReserveLimits {
            global_tokens: 1001,
            ..limits(5, 5)
        };
        assert_eq!(
            reserve_message(&m, "u", D, roomy).await.unwrap(),
            Reservation::Reserved
        );
    }

    #[tokio::test]
    async fn users_and_days_are_separate() {
        let m = Memory::default();
        reserve_message(&m, "a", D, limits(1, 10)).await.unwrap();
        assert_eq!(
            reserve_message(&m, "b", D, limits(1, 10)).await.unwrap(),
            Reservation::Reserved
        );
        assert_eq!(
            reserve_message(&m, "a", "2026-11-17", limits(1, 10))
                .await
                .unwrap(),
            Reservation::Reserved
        );
    }

    #[tokio::test]
    async fn release_returns_both() {
        let m = Memory::default();
        reserve_message(&m, "u", D, limits(5, 5)).await.unwrap();
        release_message(&m, "u", D).await;
        assert_eq!(m.count(&UsageScope::User("u".into()), D), 0);
        assert_eq!(m.count(&UsageScope::Global, D), 0);
    }

    #[test]
    fn date_and_month_keys() {
        let at = DateTime::parse_from_rfc3339("2026-01-05T23:59:59Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(usage_date(at), "2026-01-05");
        assert_eq!(usage_month(at), "2026-01");
    }
}
