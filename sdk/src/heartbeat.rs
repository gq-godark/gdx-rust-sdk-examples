//! Shared heartbeat stale-detection logic for trading and market-data transports.

use std::time::{Duration, Instant};

/// Tracks inbound freshness and consecutive missed heartbeat intervals.
#[derive(Debug, Clone)]
pub struct HeartbeatTracker {
    last_inbound: Instant,
    missed_count: u32,
    inbound_since_last_ping: bool,
}

impl HeartbeatTracker {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            last_inbound: now,
            missed_count: 0,
            inbound_since_last_ping: true,
        }
    }

    /// Record any inbound WebSocket traffic (text, binary, or control).
    pub fn record_inbound(&mut self, now: Instant) {
        self.last_inbound = now;
        self.missed_count = 0;
        self.inbound_since_last_ping = true;
    }

    /// Evaluate one heartbeat tick. Returns `Ok(())` to send a ping, or a stale reason to disconnect.
    pub fn on_tick(
        &mut self,
        now: Instant,
        stale_timeout: Duration,
        missed_heartbeat_limit: u32,
    ) -> Result<(), String> {
        if now.saturating_duration_since(self.last_inbound) >= stale_timeout {
            return Err(format!(
                "stale heartbeat: no inbound message for {}s",
                stale_timeout.as_secs()
            ));
        }

        if !self.inbound_since_last_ping {
            self.missed_count = self.missed_count.saturating_add(1);
            if self.missed_count >= missed_heartbeat_limit {
                return Err(format!(
                    "stale heartbeat: missed {} heartbeat responses (limit {missed_heartbeat_limit})",
                    self.missed_count
                ));
            }
        }

        self.inbound_since_last_ping = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_missed_interval_does_not_disconnect() {
        let t0 = Instant::now();
        let mut tracker = HeartbeatTracker::new(t0);
        tracker.on_tick(t0, Duration::from_secs(120), 2).expect("first tick");
        let t1 = t0 + Duration::from_secs(30);
        tracker.on_tick(t1, Duration::from_secs(120), 2).expect("one miss");
        assert_eq!(tracker.missed_count, 1);
    }

    #[test]
    fn test_two_missed_intervals_disconnect() {
        let t0 = Instant::now();
        let mut tracker = HeartbeatTracker::new(t0);
        tracker.on_tick(t0, Duration::from_secs(120), 2).expect("first tick");
        let t1 = t0 + Duration::from_secs(30);
        tracker.on_tick(t1, Duration::from_secs(120), 2).expect("one miss");
        let t2 = t0 + Duration::from_secs(60);
        let err = tracker
            .on_tick(t2, Duration::from_secs(120), 2)
            .expect_err("second miss");
        assert!(err.contains("missed 2 heartbeat responses"));
    }

    #[test]
    fn test_inbound_resets_missed_counter() {
        let t0 = Instant::now();
        let mut tracker = HeartbeatTracker::new(t0);
        tracker.on_tick(t0, Duration::from_secs(120), 2).expect("first tick");
        let t1 = t0 + Duration::from_secs(30);
        tracker.on_tick(t1, Duration::from_secs(120), 2).expect("one miss");
        tracker.record_inbound(t1 + Duration::from_secs(5));
        let t2 = t0 + Duration::from_secs(60);
        tracker.on_tick(t2, Duration::from_secs(120), 2).expect("reset");
        assert_eq!(tracker.missed_count, 0);
    }

    #[test]
    fn test_absolute_stale_timeout_wins() {
        let t0 = Instant::now();
        let mut tracker = HeartbeatTracker::new(t0);
        tracker.on_tick(t0, Duration::from_secs(120), 2).expect("first tick");
        let stale = t0 + Duration::from_secs(121);
        let err = tracker
            .on_tick(stale, Duration::from_secs(120), 2)
            .expect_err("absolute stale");
        assert!(err.contains("no inbound message for 120s"));
    }
}
