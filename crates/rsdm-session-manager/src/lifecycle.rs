//! Admission rules for launches, readiness and session shutdown.

use std::collections::HashSet;

use rsdm_core::domain::SessionPhase;

#[derive(Debug)]
pub(super) struct Lifecycle {
    pub phase: SessionPhase,
    pending: HashSet<String>,
}

impl Lifecycle {
    pub fn new() -> Self {
        Self { phase: SessionPhase::Starting, pending: HashSet::new() }
    }

    pub fn accepts_launch(&self) -> bool {
        matches!(self.phase, SessionPhase::Starting | SessionPhase::Running)
    }

    pub fn accepts_finalize(&self) -> bool {
        self.accepts_launch()
    }

    pub fn register_launch(&mut self, unit: String) -> bool {
        self.accepts_launch() && self.pending.insert(unit)
    }

    pub fn complete_launch(&mut self, unit: &str) {
        self.pending.remove(unit);
    }

    pub fn pending_launches(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn ready(&mut self) {
        if self.phase == SessionPhase::Starting {
            self.phase = SessionPhase::Running;
        }
    }

    pub fn prepare(&mut self) {
        self.phase = SessionPhase::Preparing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_closes_admission_but_drains_previously_accepted_launches() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.ready();
        assert!(lifecycle.register_launch("accepted.service".into()));
        lifecycle.prepare();
        assert!(!lifecycle.register_launch("late.service".into()));
        assert!(!lifecycle.accepts_finalize());
        assert!(lifecycle.pending_launches());
        lifecycle.complete_launch("accepted.service");
        assert!(!lifecycle.pending_launches());
    }

    #[test]
    fn late_readiness_cannot_reopen_a_shutdown_session() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.prepare();
        lifecycle.ready();
        assert_eq!(lifecycle.phase, SessionPhase::Preparing);
    }
}
