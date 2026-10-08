//! Small environment lookups shared by the locker. The banner/clock/hostname
//! helpers moved to `rsdm_ui::banner`; only the seated-user lookup is locker
//! specific.

/// Resolve the process owner's login name from the effective uid.
///
/// Environment variables such as `USER` are deliberately ignored: they are
/// caller-controlled and must never select the PAM identity used by a locker.
pub fn current_username() -> Option<String> {
    rsdm_infra::unix::current_username()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_effective_uid() {
        assert!(current_username().is_some());
    }
}
