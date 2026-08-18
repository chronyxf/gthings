//! Time helpers.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix time in milliseconds since the epoch.
///
/// Uses [`SystemTime`], which is sufficient for pacing/cooldown bookkeeping
/// and requires no external dependencies. Returns `0` if the system clock is
/// before the Unix epoch (practically impossible).
#[must_use]
pub fn unix_now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

#[cfg(test)]
mod tests {
    use super::unix_now_ms;

    #[test]
    fn unix_now_ms_is_positive_and_advances() {
        let a = unix_now_ms();
        assert!(a > 0, "epoch ms should be positive");
        let b = unix_now_ms();
        assert!(b >= a, "clock should not go backwards");
    }
}
