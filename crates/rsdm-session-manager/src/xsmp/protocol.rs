//! Save barriers apply to every participating XSMP connection.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Registering,
    InitialSave,
    InitialDone,
    Idle,
    Saving,
    WaitingForPhase2,
    SavingPhase2,
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
        matches!(self, Self::Saving | Self::WaitingForPhase2 | Self::SavingPhase2)
    }

    pub fn save_done(&mut self, success: bool) {
        *self = match *self {
            Self::InitialSave => Self::InitialDone,
            Self::Cancelling => Self::CancelledDone,
            Self::Saving | Self::SavingPhase2 if success => Self::Saved,
            Self::Saving | Self::SavingPhase2 => Self::Failed,
            other => other,
        };
    }
}

pub(super) fn phase2_ready(phases: &[Phase]) -> bool {
    phases.contains(&Phase::WaitingForPhase2) && phases.iter().all(|phase|
        matches!(phase, Phase::WaitingForPhase2 | Phase::Saved | Phase::Dying | Phase::Closed | Phase::TimedOut))
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
        assert!(!phase2_ready(&[Phase::WaitingForPhase2, Phase::Saving]));
        assert!(phase2_ready(&[Phase::WaitingForPhase2, Phase::Saved]));
        assert!(phase2_ready(&[Phase::WaitingForPhase2, Phase::Closed]));
    }

    #[test]
    fn save_done_does_not_allow_die_while_another_client_saves() {
        assert!(!die_ready(&[Phase::Saved, Phase::SavingPhase2]));
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
