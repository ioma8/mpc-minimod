//! Monotonic milliseconds for the status line.
//!
//! The looper shows a short message for a couple of seconds after an action.
//! `CLOCK_MONOTONIC` is the only clock that cannot jump while the sampler runs,
//! so a message never sticks around forever or vanishes instantly.

/// Monotonic milliseconds since an unspecified epoch, or zero if the clock
/// could not be read (which only ever makes a message disappear sooner).
#[must_use]
pub fn now_ms() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `clock_gettime` only writes the `timespec` we hand it.
    let ok = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut ts) };
    if ok != 0 {
        return 0;
    }
    ts.tv_sec
        .saturating_mul(1000)
        .saturating_add(ts.tv_nsec / 1_000_000)
}
