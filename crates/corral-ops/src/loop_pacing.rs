//! Shared pacing for the workspace's single synchronization loop.
use std::time::Duration;

pub const SYNC_EVERY: Duration = Duration::from_secs(2);

/// The longest wait between periodic syncs; it bounds how late autopilot reacts on its own.
pub const SYNC_MAX_WAIT: Duration = Duration::from_secs(60);

/// Back off costly idle passes to about 5% of a core (before the one-minute latency cap).
/// Explicit user triggers bypass the wait and are coalesced, never replayed as a pass per key.
pub fn periodic_wait(pass_cpu: Duration) -> Duration {
    pass_cpu.saturating_mul(19).clamp(SYNC_EVERY, SYNC_MAX_WAIT)
}

/// CPU time used so far by this whole process (every thread), where /proc provides it.
pub fn process_cpu() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // utime and stime are fields 14 and 15; the first field after the parenthesized name is 3.
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    // Linux reports these in USER_HZ, which is 100 on every architecture.
    Some(Duration::from_millis(
        utime.saturating_add(stime).saturating_mul(10),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_sync_waits_nineteen_times_pass_cpu_between_two_and_sixty_seconds() {
        let ms = Duration::from_millis;
        assert_eq!(periodic_wait(Duration::ZERO), SYNC_EVERY);
        assert_eq!(periodic_wait(ms(100)), SYNC_EVERY);
        assert_eq!(periodic_wait(ms(500)), ms(9_500));
        assert_eq!(periodic_wait(ms(1_500)), ms(28_500));
        assert_eq!(periodic_wait(ms(2_000)), ms(38_000));
        assert_eq!(periodic_wait(ms(4_000)), SYNC_MAX_WAIT);
        assert_eq!(periodic_wait(Duration::MAX), SYNC_MAX_WAIT);
    }

    #[test]
    fn process_cpu_is_read_and_never_runs_backwards() {
        let before = process_cpu();
        if cfg!(target_os = "linux") {
            assert!(before.is_some());
        }
        assert!(process_cpu() >= before);
    }
}
