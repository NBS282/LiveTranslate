//! Throttle for download-progress callbacks.
//!
//! `download_with_progress` fires its callback once per received network
//! chunk — tens of thousands of times for a multi-hundred-MB file. Forwarding
//! every one of those as a Tauri event floods the webview with DOM updates
//! and can freeze the UI for the rest of the session. Callers wrap their
//! progress closure with a `ProgressThrottle` so only meaningful updates
//! (percent changed, or enough wall time elapsed) reach the frontend.

use std::time::{Duration, Instant};

pub struct ProgressThrottle {
    min_interval: Duration,
    last_emit: Option<(Instant, Option<u8>)>,
}

impl ProgressThrottle {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_emit: None,
        }
    }

    /// True when this progress sample is worth forwarding to the UI.
    /// Takes `now` explicitly so the decision logic is testable without
    /// sleeping; production callers pass `Instant::now()`.
    pub fn should_emit_at(&mut self, downloaded: u64, total: u64, now: Instant) -> bool {
        let pct = (total > 0).then(|| ((downloaded * 100) / total).min(100) as u8);

        let emit = match self.last_emit {
            None => true,
            Some((at, last_pct)) => {
                now.duration_since(at) >= self.min_interval || (pct.is_some() && pct != last_pct)
            }
        };

        if emit {
            self.last_emit = Some((now, pct));
        }
        emit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_millis(200);

    fn throttle() -> ProgressThrottle {
        ProgressThrottle::new(INTERVAL)
    }

    #[test]
    fn emits_the_first_sample() {
        let mut t = throttle();
        assert!(t.should_emit_at(0, 1000, Instant::now()));
    }

    #[test]
    fn suppresses_same_percent_within_interval() {
        let mut t = throttle();
        let start = Instant::now();
        assert!(t.should_emit_at(100, 1000, start));
        assert!(!t.should_emit_at(101, 1000, start + Duration::from_millis(50)));
    }

    #[test]
    fn emits_when_percent_changes_even_within_interval() {
        let mut t = throttle();
        let start = Instant::now();
        assert!(t.should_emit_at(100, 1000, start));
        assert!(t.should_emit_at(110, 1000, start + Duration::from_millis(50)));
    }

    #[test]
    fn emits_after_interval_even_without_percent_change() {
        let mut t = throttle();
        let start = Instant::now();
        assert!(t.should_emit_at(100, 1000, start));
        assert!(t.should_emit_at(101, 1000, start + INTERVAL));
    }

    #[test]
    fn suppressed_samples_do_not_reset_the_interval() {
        let mut t = throttle();
        let start = Instant::now();
        assert!(t.should_emit_at(100, 1000, start));
        assert!(!t.should_emit_at(101, 1000, start + Duration::from_millis(150)));
        // 200ms after the last EMIT (not the last suppressed sample).
        assert!(t.should_emit_at(102, 1000, start + INTERVAL));
    }

    #[test]
    fn unknown_total_throttles_by_interval_only() {
        let mut t = throttle();
        let start = Instant::now();
        assert!(t.should_emit_at(100, 0, start));
        assert!(!t.should_emit_at(200, 0, start + Duration::from_millis(50)));
        assert!(t.should_emit_at(300, 0, start + INTERVAL));
    }
}
