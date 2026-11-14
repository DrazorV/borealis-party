//! Limits around every call: the per-token rate, the per-account heavy-call semaphore, and the idempotency cache of
//! writes. All in memory: they protect the server, and the combat API keeps its own request ids in the database too.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The registry's limits.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Calls a minute per token.
    pub calls_per_minute: usize,
    /// Heavy calls at a time per account.
    pub heavy_per_account: usize,
    /// How long a write's answer is kept for a retry with the same request id.
    pub idempotency_ttl: Duration,
    /// Request ids kept at most (oldest dropped first).
    pub idempotency_max: usize,
    /// Largest result, in bytes of JSON.
    pub max_result_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            calls_per_minute: 60,
            heavy_per_account: 2,
            idempotency_ttl: Duration::from_secs(24 * 60 * 60),
            idempotency_max: 20_000,
            max_result_bytes: 256 * 1024,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A sliding one-minute window per key.
#[derive(Debug, Default)]
pub struct Rate {
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Rate {
    /// Count a call; `false` when the key is over `per_minute`.
    pub fn take(&self, key: &str, per_minute: usize) -> bool {
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let mut hits = lock(&self.hits);
        if hits.len() > 10_000 {
            hits.retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) < window));
        }
        let q = hits.entry(key.to_string()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
            q.pop_front();
        }
        if q.len() >= per_minute {
            return false;
        }
        q.push_back(now);
        true
    }
}

/// One semaphore per account for heavy calls.
#[derive(Debug, Default)]
pub struct Heavy {
    by_account: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl Heavy {
    /// A permit, or `None` when the account already runs `max` heavy calls (never waits: the model retries).
    pub fn try_take(&self, account: &str, max: usize) -> Option<OwnedSemaphorePermit> {
        let sem = {
            let mut map = lock(&self.by_account);
            if map.len() > 10_000 {
                map.retain(|_, s| s.available_permits() < max);
            }
            Arc::clone(
                map.entry(account.to_string())
                    .or_insert_with(|| Arc::new(Semaphore::new(max))),
            )
        };
        sem.try_acquire_owned().ok()
    }
}

/// The answer kept for a write.
#[derive(Debug, Clone)]
struct Kept {
    at: Instant,
    /// A hash of the arguments: the same id with other arguments is refused, never answered with the old result.
    fingerprint: [u8; 32],
    value: Value,
}

/// Writes done, by (caller key, tool, request id), and a lock per key so a write and its retry never both run.
#[derive(Debug, Default)]
pub struct Idempotency {
    done: Mutex<HashMap<String, Kept>>,
    order: Mutex<VecDeque<String>>,
    running: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// What the cache says about a write.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    /// Not done yet.
    New,
    /// Done with these arguments: its answer.
    Done(Value),
    /// The id was used for other arguments.
    Reused,
}

impl Idempotency {
    /// The lock of one key, held for the whole write.
    pub fn lock_of(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut running = lock(&self.running);
        if running.len() > 1_000 {
            running.retain(|_, m| Arc::strong_count(m) > 1);
        }
        Arc::clone(running.entry(key.to_string()).or_default())
    }

    /// Look a key up.
    pub fn seen(&self, key: &str, fingerprint: &[u8; 32], ttl: Duration) -> Seen {
        let done = lock(&self.done);
        match done.get(key) {
            Some(k) if k.at.elapsed() < ttl => {
                if &k.fingerprint == fingerprint {
                    Seen::Done(k.value.clone())
                } else {
                    Seen::Reused
                }
            }
            _ => Seen::New,
        }
    }

    /// Keep a write's answer.
    pub fn keep(&self, key: &str, fingerprint: [u8; 32], value: Value, max: usize) {
        let mut done = lock(&self.done);
        let mut order = lock(&self.order);
        done.insert(
            key.to_string(),
            Kept {
                at: Instant::now(),
                fingerprint,
                value,
            },
        );
        order.push_back(key.to_string());
        while order.len() > max {
            if let Some(old) = order.pop_front() {
                done.remove(&old);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_stops_at_the_limit_per_key() {
        let r = Rate::default();
        assert!((0..3).all(|_| r.take("a", 3)));
        assert!(!r.take("a", 3));
        assert!(r.take("b", 3));
    }

    #[test]
    fn heavy_permits_are_per_account() {
        let h = Heavy::default();
        let p1 = h.try_take("a", 1);
        assert!(p1.is_some());
        assert!(h.try_take("a", 1).is_none());
        assert!(h.try_take("b", 1).is_some());
        drop(p1);
        assert!(h.try_take("a", 1).is_some());
    }

    #[test]
    fn a_reused_id_with_other_arguments_is_refused() {
        let i = Idempotency::default();
        let ttl = Duration::from_secs(60);
        assert_eq!(i.seen("k", &[1; 32], ttl), Seen::New);
        i.keep("k", [1; 32], serde_json::json!({"ok": 1}), 10);
        assert_eq!(
            i.seen("k", &[1; 32], ttl),
            Seen::Done(serde_json::json!({"ok": 1}))
        );
        assert_eq!(i.seen("k", &[2; 32], ttl), Seen::Reused);
    }
}
