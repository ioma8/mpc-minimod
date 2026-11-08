//! The monotonic clock the glue schedules on.
//!
//! Every deadline in the mod (toast expiry, spectral marker timeouts, settings
//! throttling) is a `CLOCK_MONOTONIC` millisecond count, so it never jumps when
//! the wall clock is adjusted.

/// Milliseconds since boot.
#[must_use]
pub fn now_ms() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid out-parameter for clock_gettime.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, core::ptr::addr_of_mut!(ts)) } != 0 {
        return 0;
    }
    ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000
}

/// Milliseconds since boot, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_now_ms() -> i64 {
    now_ms()
}

#[cfg(test)]
mod tests {
    use super::now_ms;

    #[test]
    fn the_clock_moves_forward_in_real_time() {
        let first = now_ms();
        let started = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = now_ms();
        let elapsed = started.elapsed().as_millis();
        assert!(second >= first, "{first} -> {second}");
        let advanced = u128::try_from(second - first).unwrap_or(0);
        // The sleep is 20 ms; allow scheduling slack but require that the clock
        // saw the same real time pass (and not a multiple of it).
        assert!(advanced <= elapsed + 5, "{advanced} vs {elapsed}");
        assert!(advanced + 5 >= elapsed, "{advanced} vs {elapsed}");
    }
}
