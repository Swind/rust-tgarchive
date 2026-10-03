use std::{sync::Mutex, time::Duration};

use chrono::{DateTime, Utc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Upper bound for the adaptive interval after repeated FLOOD_WAITs.
pub const MAX_ADAPTIVE_INTERVAL: Duration = Duration::from_secs(10);
/// Successful requests needed before an elevated interval is halved toward the base.
pub const DECAY_AFTER_SUCCESSES: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitStatus {
    pub interval_ms: u64,
    pub base_interval_ms: u64,
    pub last_flood_wait_secs: Option<u64>,
    pub last_flood_at: Option<DateTime<Utc>>,
}

struct State {
    interval: Duration,
    next_allowed: Instant,
    successes: u32,
    last_flood: Option<(u64, DateTime<Utc>)>,
}

/// Process-wide pacing of Telegram history requests. Every `getHistory` call (history sync,
/// catch-up, baseline probes) takes a slot here, so concurrent callers are spaced out too.
/// A base interval of zero disables pacing and adaptive slowdown; FLOOD_WAITs that are
/// waited out still hold back every caller.
pub struct RatePacer {
    base: Duration,
    state: Mutex<State>,
}

impl RatePacer {
    pub fn new(base: Duration) -> Self {
        Self {
            base,
            state: Mutex::new(State {
                interval: base,
                next_allowed: Instant::now(),
                successes: 0,
                last_flood: None,
            }),
        }
    }

    pub fn disabled() -> Self {
        Self::new(Duration::ZERO)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reserves the next request slot and waits for it. Returns `true` when cancelled.
    pub async fn acquire(&self, cancel: &CancellationToken) -> bool {
        let start = {
            let mut state = self.lock();
            let start = state.next_allowed.max(Instant::now());
            state.next_allowed = start + state.interval;
            start
        };
        if start <= Instant::now() {
            return cancel.is_cancelled();
        }
        tokio::select! { _ = tokio::time::sleep_until(start) => false, _ = cancel.cancelled() => true }
    }

    pub fn on_success(&self) {
        let mut state = self.lock();
        if state.interval <= self.base {
            return;
        }
        state.successes += 1;
        if state.successes >= DECAY_AFTER_SUCCESSES {
            state.successes = 0;
            state.interval = (state.interval / 2).max(self.base);
        }
    }

    /// Records a FLOOD_WAIT: doubles the interval (capped) and, when the caller will wait it
    /// out (`will_wait`), holds every request back for that long.
    pub fn on_flood(&self, wait_secs: u64, will_wait: bool) {
        let mut state = self.lock();
        state.interval = (state.interval * 2).min(MAX_ADAPTIVE_INTERVAL.max(self.base));
        state.successes = 0;
        state.last_flood = Some((wait_secs, Utc::now()));
        if will_wait {
            let barrier = Instant::now() + Duration::from_secs(wait_secs);
            state.next_allowed = state.next_allowed.max(barrier);
        }
        tracing::warn!(
            wait_secs,
            interval_ms = state.interval.as_millis() as u64,
            will_wait,
            "Telegram FLOOD_WAIT on history request; slowing down"
        );
    }

    pub fn status(&self) -> RateLimitStatus {
        let state = self.lock();
        RateLimitStatus {
            interval_ms: state.interval.as_millis() as u64,
            base_interval_ms: self.base.as_millis() as u64,
            last_flood_wait_secs: state.last_flood.map(|(secs, _)| secs),
            last_flood_at: state.last_flood.map(|(_, at)| at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn spaces_slots_and_zero_disables() {
        let cancel = CancellationToken::new();
        let pacer = RatePacer::new(Duration::from_millis(1000));
        let t0 = Instant::now();
        for i in 0..3u32 {
            assert!(!pacer.acquire(&cancel).await);
            assert_eq!(Instant::now() - t0, Duration::from_millis(1000) * i);
        }
        let off = RatePacer::disabled();
        let t1 = Instant::now();
        for _ in 0..5 {
            off.acquire(&cancel).await;
        }
        assert_eq!(Instant::now(), t1);
    }

    #[tokio::test(start_paused = true)]
    async fn flood_doubles_to_cap_and_decays_after_successes() {
        let pacer = RatePacer::new(Duration::from_secs(1));
        for _ in 0..5 {
            pacer.on_flood(1, false);
        }
        assert_eq!(pacer.status().interval_ms, 10_000);
        assert_eq!(pacer.status().last_flood_wait_secs, Some(1));
        for _ in 0..DECAY_AFTER_SUCCESSES - 1 {
            pacer.on_success();
        }
        assert_eq!(pacer.status().interval_ms, 10_000);
        pacer.on_success();
        assert_eq!(pacer.status().interval_ms, 5_000);
        for _ in 0..DECAY_AFTER_SUCCESSES * 4 {
            pacer.on_success();
        }
        assert_eq!(pacer.status().interval_ms, 1_000);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_wait_returns_promptly() {
        let cancel = CancellationToken::new();
        let pacer = RatePacer::new(Duration::from_secs(60));
        pacer.acquire(&cancel).await;
        let t0 = Instant::now();
        cancel.cancel();
        assert!(pacer.acquire(&cancel).await);
        assert_eq!(Instant::now(), t0);
    }
}
