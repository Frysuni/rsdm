//! Save barriers apply to every participating XSMP connection.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Registering,
    InitialSave,
    InitialDone,
    Idle,
    Saving,
    Phase2Waiting,
    Phase2Saving,
    Saved,
    Dying,
    Cancelling,
    CancelledDone,
    TimedOut,
    Failed,
    Closed,
}

impl Phase {
    pub fn saving(self) -> bool {
        matches!(self, Self::Saving | Self::Phase2Waiting | Self::Phase2Saving)
    }

    pub fn save_done(&mut self, success: bool) {
        *self = match *self {
            Self::InitialSave => Self::InitialDone,
            Self::Cancelling => Self::CancelledDone,
            Self::Saving | Self::Phase2Saving if success => Self::Saved,
            Self::Saving | Self::Phase2Saving => Self::Failed,
            other => other,
        };
    }
}

pub(super) fn phase2_ready(phases: &[Phase]) -> bool {
    phases.contains(&Phase::Phase2Waiting) && phases.iter().all(|phase|
        matches!(phase, Phase::Phase2Waiting | Phase::Saved | Phase::Dying | Phase::Closed | Phase::TimedOut))
}

pub(super) fn die_ready(phases: &[Phase]) -> bool {
    !phases.is_empty() && phases.iter().all(|phase|
        matches!(phase, Phase::Saved | Phase::Dying | Phase::Closed | Phase::TimedOut))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase2_waits_for_every_participants_first_phase() {
        assert!(!phase2_ready(&[Phase::Phase2Waiting, Phase::Saving]));
        assert!(phase2_ready(&[Phase::Phase2Waiting, Phase::Saved]));
        assert!(phase2_ready(&[Phase::Phase2Waiting, Phase::Closed]));
    }

    #[test]
    fn save_done_does_not_allow_die_while_another_client_saves() {
        assert!(!die_ready(&[Phase::Saved, Phase::Phase2Saving]));
        assert!(!die_ready(&[Phase::Saved, Phase::Failed]));
        assert!(die_ready(&[Phase::Saved, Phase::Saved]));
    }

    #[test]
    fn late_save_done_cannot_restart_a_timed_out_or_cancelled_save() {
        let mut phase = Phase::TimedOut;
        phase.save_done(true);
        assert_eq!(phase, Phase::TimedOut);
        let mut phase = Phase::Idle;
        phase.save_done(true);
        assert_eq!(phase, Phase::Idle);
    }
}
