//! Monotonic timestamps shared with logind shutdown budgets.

pub fn monotonic_usec() -> Result<u64, std::io::Error> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: time is a valid writable timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(time.tv_sec as u64 * 1_000_000 + time.tv_nsec as u64 / 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_deadlines_use_a_monotonic_clock() {
        let first = monotonic_usec().unwrap();
        assert!(monotonic_usec().unwrap() >= first);
    }
}
