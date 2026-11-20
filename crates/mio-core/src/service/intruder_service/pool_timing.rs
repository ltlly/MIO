use std::time::Duration;

use anyhow::{Context, Result, ensure};
use tokio::time::Instant;

use crate::IntruderPoolDelay;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PoolTiming {
    pub(super) last_send: Option<Instant>,
    pub(super) sent_count: u64,
    pub(super) not_before: Option<Instant>,
}

impl PoolTiming {
    pub(super) fn ready_at(&self, now: Instant) -> Instant {
        self.not_before.map_or(now, |deadline| deadline.max(now))
    }

    pub(super) fn check_policy(self, policy: Option<IntruderPoolDelay>) -> Result<()> {
        ensure!(
            self.last_send.is_some() == (self.sent_count > 0),
            "Intruder 资源池发送计数与时钟不一致"
        );
        if let Some(policy) = policy {
            policy.validate()?;
            if let Some(sent) = self.last_send {
                sent.checked_add(interval_upper_bound(policy, self.sent_count)?)
                    .context("Intruder 资源池下次发送时间溢出；未安排请求")?;
            }
        }
        Ok(())
    }

    pub(super) fn reconfigured(self, policy: Option<IntruderPoolDelay>) -> Result<Self> {
        // Check the full interval bound before sampling. A rejected deadline consumes no draw.
        self.check_policy(policy)?;
        let not_before = match (self.last_send, policy) {
            (Some(sent), Some(policy)) => Some(
                sent.checked_add(interval_after_send(policy, self.sent_count)?)
                    .context("Intruder 资源池下次发送时间溢出；未安排请求")?,
            ),
            _ => None,
        };
        Ok(Self { not_before, ..self })
    }

    pub(super) fn after_send(
        self,
        now: Instant,
        policy: Option<IntruderPoolDelay>,
    ) -> Result<Self> {
        self.next_send(now)?.reconfigured(policy)
    }

    pub(super) fn check_after_send(
        self,
        now: Instant,
        policy: Option<IntruderPoolDelay>,
    ) -> Result<()> {
        self.next_send(now)?.check_policy(policy)
    }

    fn next_send(self, now: Instant) -> Result<Self> {
        ensure!(
            self.last_send.is_some() == (self.sent_count > 0),
            "Intruder 资源池发送计数与时钟不一致"
        );
        ensure!(
            self.last_send.is_none_or(|last| now >= last),
            "Intruder 资源池发送时钟回退"
        );
        Ok(Self {
            last_send: Some(now),
            sent_count: self
                .sent_count
                .checked_add(1)
                .context("Intruder 资源池发送计数溢出；未发送请求")?,
            not_before: None,
        })
    }
}

fn interval_bounds(policy: IntruderPoolDelay, sent_count: u64) -> Result<(u64, u64)> {
    policy.validate()?;
    ensure!(sent_count > 0, "Intruder 资源池间隔要求已有发送记录");
    match policy {
        IntruderPoolDelay::Fixed { milliseconds } => Ok((milliseconds, milliseconds)),
        IntruderPoolDelay::Random { milliseconds } => Ok((
            milliseconds / 2,
            milliseconds
                .checked_mul(3)
                .context("Intruder 随机间隔上界溢出")?
                / 2,
        )),
        IntruderPoolDelay::Increment {
            initial_ms,
            increment_ms,
        } => {
            let milliseconds = sent_count
                .checked_mul(increment_ms)
                .and_then(|increment| initial_ms.checked_add(increment))
                .context("Intruder 资源池递增间隔溢出；未安排请求")?;
            Ok((milliseconds, milliseconds))
        }
    }
}

fn interval_upper_bound(policy: IntruderPoolDelay, sent_count: u64) -> Result<Duration> {
    interval_bounds(policy, sent_count).map(|(_, upper)| Duration::from_millis(upper))
}

fn interval_after_send(policy: IntruderPoolDelay, sent_count: u64) -> Result<Duration> {
    let (lower, upper) = interval_bounds(policy, sent_count)?;
    if lower == upper {
        return Ok(Duration::from_millis(lower));
    }
    let choices = u32::try_from(
        upper
            .checked_sub(lower)
            .and_then(|width| width.checked_add(1))
            .context("Intruder 随机间隔范围溢出")?,
    )?;
    // MIO's integer-millisecond policy is uniform over the inclusive bounds. Finite Ext
    // samples support their approximate range, not proof of its exact random distribution.
    let offset = uniform_u32(choices);
    Ok(Duration::from_millis(lower + u64::from(offset)))
}

fn uniform_u32(choices: u32) -> u32 {
    if choices <= 1 {
        return 0;
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: choices > 1, arc4random_uniform has no pointer preconditions.
        unsafe { libc::arc4random_uniform(choices) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let threshold = choices.wrapping_neg() % choices;
        loop {
            let mut buf = [0u8; 4];
            fill_os_random(&mut buf);
            let candidate = u32::from_ne_bytes(buf);
            if candidate >= threshold {
                return candidate % choices;
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn fill_os_random(buf: &mut [u8]) {
    use std::io::Read as _;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom")
        && f.read_exact(buf).is_ok()
    {
        return;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    buf.copy_from_slice(&nanos.to_le_bytes()[..buf.len()]);
}

#[cfg(windows)]
fn fill_os_random(buf: &mut [u8]) {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            h_algorithm: *mut std::ffi::c_void,
            pb_buffer: *mut u8,
            cb_buffer: u32,
            dw_flags: u32,
        ) -> i32;
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        buf.copy_from_slice(&nanos.to_le_bytes()[..buf.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_and_increment_intervals_follow_the_confirmed_post_send_count() {
        let fixed = IntruderPoolDelay::Fixed { milliseconds: 100 };
        for count in [1, 2, 8, u64::MAX] {
            assert_eq!(
                interval_after_send(fixed, count).unwrap(),
                Duration::from_millis(100)
            );
        }
        let increment = IntruderPoolDelay::Increment {
            initial_ms: 100,
            increment_ms: 100,
        };
        let actual: Vec<_> = (1..=7)
            .map(|count| interval_after_send(increment, count).unwrap().as_millis())
            .collect();
        assert_eq!(actual, [200, 300, 400, 500, 600, 700, 800]);
        assert!(interval_after_send(increment, 0).is_err());
    }

    #[test]
    fn random_interval_bounds_are_inclusive_integer_milliseconds_without_distribution_assertions() {
        for (milliseconds, lower, upper) in [
            (0, 0, 0),
            (1, 0, 1),
            (3, 1, 4),
            (1000, 500, 1500),
            (1001, 500, 1501),
            (300_000, 150_000, 450_000),
        ] {
            let policy = IntruderPoolDelay::Random { milliseconds };
            assert_eq!(interval_bounds(policy, 1).unwrap(), (lower, upper));
            assert_eq!(
                interval_upper_bound(policy, 1).unwrap(),
                Duration::from_millis(upper)
            );
            let sampled = interval_after_send(policy, 1).unwrap();
            assert!(
                sampled >= Duration::from_millis(lower) && sampled <= Duration::from_millis(upper)
            );
            assert_eq!(sampled.subsec_nanos() % 1_000_000, 0);
        }
        let now = Instant::now();
        let zero = PoolTiming::default()
            .after_send(now, Some(IntruderPoolDelay::Random { milliseconds: 0 }))
            .unwrap();
        assert_eq!(zero.not_before, Some(now));
        assert_eq!(zero.sent_count, 1);
        assert!(interval_bounds(IntruderPoolDelay::Random { milliseconds: 100 }, 0).is_err());
        for milliseconds in [300_001, u64::MAX] {
            assert!(interval_upper_bound(IntruderPoolDelay::Random { milliseconds }, 1).is_err());
        }
    }

    #[test]
    fn send_anchor_does_not_add_completion_delay_or_replay_elapsed_intervals() {
        let policy = Some(IntruderPoolDelay::Fixed { milliseconds: 100 });
        let start = Instant::now();
        let fresh = PoolTiming::default().reconfigured(policy).unwrap();
        assert_eq!(fresh.ready_at(start), start);
        let first = fresh.after_send(start, policy).unwrap();
        assert_eq!(first.not_before, Some(start + Duration::from_millis(100)));
        assert_eq!(
            first.ready_at(start + Duration::from_millis(99)),
            start + Duration::from_millis(100)
        );
        let finished = start + Duration::from_millis(500);
        assert_eq!(
            first.ready_at(finished),
            finished,
            "a held response does not add another 100 ms"
        );
        let second = first.after_send(finished, policy).unwrap();
        assert_eq!(
            second.not_before,
            Some(finished + Duration::from_millis(100)),
            "overdue dispatch rebases on its real send, without catching up old intervals"
        );
        assert_eq!(second.sent_count, 2);
    }

    #[test]
    fn policy_edits_preserve_send_history_and_disabled_delay_only_removes_the_deadline() {
        let start = Instant::now();
        let fixed = Some(IntruderPoolDelay::Fixed { milliseconds: 100 });
        let sent = PoolTiming::default().after_send(start, fixed).unwrap();
        assert_eq!(sent.reconfigured(fixed).unwrap(), sent);
        let changed = sent
            .reconfigured(Some(IntruderPoolDelay::Increment {
                initial_ms: 100,
                increment_ms: 100,
            }))
            .unwrap();
        assert_eq!(
            (changed.last_send, changed.sent_count, changed.not_before),
            (Some(start), 1, Some(start + Duration::from_millis(200)))
        );
        let disabled = changed.reconfigured(None).unwrap();
        assert_eq!(
            (disabled.last_send, disabled.sent_count, disabled.not_before),
            (Some(start), 1, None)
        );
        let next = disabled
            .after_send(start + Duration::from_millis(300), None)
            .unwrap();
        let enabled = next
            .reconfigured(Some(IntruderPoolDelay::Increment {
                initial_ms: 100,
                increment_ms: 100,
            }))
            .unwrap();
        assert_eq!(enabled.sent_count, 2);
        assert_eq!(enabled.not_before, Some(start + Duration::from_millis(600)));
    }

    #[test]
    fn arithmetic_and_monotonic_deadline_overflow_are_errors_without_changing_history() {
        for policy in [
            IntruderPoolDelay::Increment {
                initial_ms: 0,
                increment_ms: 2,
            },
            IntruderPoolDelay::Increment {
                initial_ms: 1,
                increment_ms: 1,
            },
        ] {
            assert!(interval_after_send(policy, u64::MAX).is_err());
        }
        let timing = PoolTiming {
            last_send: Some(Instant::now()),
            sent_count: u64::MAX,
            not_before: None,
        };
        assert!(timing.after_send(Instant::now(), None).is_err());
        // Locate this macOS monotonic clock's upper second without assuming its representation.
        let start = Instant::now();
        let (mut low, mut high) = (0, u64::MAX);
        while low < high {
            let middle = low + (high - low) / 2 + 1;
            if start.checked_add(Duration::from_secs(middle)).is_some() {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        let last = start.checked_add(Duration::from_secs(low)).unwrap();
        assert!(last.checked_add(Duration::from_secs(1)).is_none());
        let clock_limit = PoolTiming {
            last_send: Some(last),
            sent_count: 1,
            not_before: None,
        };
        assert!(
            clock_limit
                .reconfigured(Some(IntruderPoolDelay::Fixed { milliseconds: 1000 }))
                .is_err()
        );
        assert!(
            clock_limit
                .check_policy(Some(IntruderPoolDelay::Random { milliseconds: 1000 }))
                .is_err(),
            "the pure upper-bound check must reject an overflowing clock before random sampling"
        );
        assert!(
            clock_limit
                .check_after_send(last, Some(IntruderPoolDelay::Random { milliseconds: 1000 }))
                .is_err()
        );
        assert!(
            timing
                .check_after_send(
                    Instant::now(),
                    Some(IntruderPoolDelay::Random { milliseconds: 100 })
                )
                .is_err()
        );
        assert_eq!(timing.sent_count, u64::MAX);
        assert_eq!(timing.not_before, None);
    }
}
