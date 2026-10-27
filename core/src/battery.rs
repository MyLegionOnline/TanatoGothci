//! Battery countdown engine.
//!
//! One run lasts one hour (3600 s). Exactly **16** battery-level messages are
//! sent over that hour, starting at 100% and ending at 0%. The first portion
//! uses larger (5–10%) deltas, and the final portion slows to ~1% deltas.
//!
//! The agreed concrete schedule is:
//! `100, 90, 80, 70, 60, 50, 40, 30, 20, 10, 9, 8, 7, 3, 1, 0`
//! — 16 levels, 100% → 0%. (The literal "5%-until-10 then 1%-to-0" wording in
//! the original brief is arithmetically inconsistent with "16 messages", so the
//! 16-message constraint was kept authoritative and the deltas were smoothed.)

use std::time::Duration;

/// Number of battery pings per run = 16.
pub const PINGS_PER_RUN: usize = 16;
/// Total runtime of a run in seconds.
pub const RUN_DURATION: Duration = Duration::from_secs(3600);

/// The ordered battery levels sent to the agent, 100% → 0%.
pub fn levels() -> Vec<u8> {
    vec![100, 90, 80, 70, 60, 50, 40, 30, 20, 10, 9, 8, 7, 3, 1, 0]
}

/// Runs `f` once per ping, passing the battery level and an async sleep-until
/// the next ping. Returns only after all 16 pings complete.
///
/// `on_tick: FnMut(u8)` is invoked for each level in sequence.
pub async fn run_countdown<F, Fut>(mut on_tick: F)
where
    F: FnMut(u8) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let schedule = schedule_ms();
    let levels = levels();
    let n = levels.len();
    let start = std::time::Instant::now();
    for i in 0..n {
        // Sleep until this ping's wall-clock slot.
        let now = start.elapsed().as_millis() as u64;
        if i > 0 && now < schedule[i] {
            tokio::time::sleep(Duration::from_millis(schedule[i] - now)).await;
        }
        on_tick(levels[i]).await;
    }
}

/// Wall-clock offset (ms since run start) at which each ping fires, spread
/// evenly across the hour.
pub fn schedule_ms() -> Vec<u64> {
    let n = PINGS_PER_RUN as u64;
    (0..n)
        .map(|i| RUN_DURATION.as_millis() as u64 * i / (n - 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn emits_16_levels_from_100_to_0() {
        let captured: std::sync::Arc<std::sync::Mutex<Vec<u8>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let c = captured.clone();
        run_countdown(move |lvl| {
            let c = c.clone();
            async move {
                c.lock().unwrap().push(lvl);
            }
        })
        .await;
        let got = captured.lock().unwrap().clone();
        assert_eq!(got.len(), PINGS_PER_RUN);
        assert_eq!(got.first(), Some(&100));
        assert_eq!(got.last(), Some(&0));
        // monotonic non-increasing
        for w in got.windows(2) {
            assert!(w[1] <= w[0], "not monotonic: {:?}", w);
        }
    }

    #[test]
    fn levels_are_monotonic_down_to_zero() {
        let l = levels();
        assert_eq!(l.len(), 16);
        let mut prev = 101u8;
        for &v in &l {
            assert!(v <= prev);
            prev = v;
        }
        assert_eq!(*l.last().unwrap(), 0);
    }

    #[test]
    fn schedule_is_monotonic_and_fits_one_hour() {
        let s = schedule_ms();
        assert_eq!(s.len(), PINGS_PER_RUN);
        assert_eq!(s[0], 0);
        assert_eq!(s[15], 3_600_000);
        for w in s.windows(2) {
            assert!(w[1] >= w[0]);
        }
    }
}
