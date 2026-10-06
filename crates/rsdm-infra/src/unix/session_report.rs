//! Wire reports for the session-leader handshake.

/// Fixed-size report sent from the session child to the greeter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaderReport {
    AuthFailed,
    UserDenied,
    SessionLaunchFailed,
    SessionSuccess,
    SessionFailed(i32),
    SessionSignaled(i32),
    Lost,
}

pub(super) const AUTHORIZED_TAG: u8 = 7;

impl LeaderReport {
    pub(super) fn encode(self) -> [u8; 5] {
        let (tag, value): (u8, i32) = match self {
            LeaderReport::AuthFailed => (0, 0),
            LeaderReport::UserDenied => (1, 0),
            LeaderReport::SessionLaunchFailed => (2, 0),
            LeaderReport::SessionSuccess => (3, 0),
            LeaderReport::SessionFailed(code) => (4, code),
            LeaderReport::SessionSignaled(signal) => (5, signal),
            LeaderReport::Lost => (6, 0),
        };
        let value = value.to_le_bytes();
        [tag, value[0], value[1], value[2], value[3]]
    }

    pub(super) fn decode(bytes: [u8; 5]) -> LeaderReport {
        let value = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        match bytes[0] {
            0 => LeaderReport::AuthFailed,
            1 => LeaderReport::UserDenied,
            2 => LeaderReport::SessionLaunchFailed,
            3 => LeaderReport::SessionSuccess,
            4 => LeaderReport::SessionFailed(value),
            5 => LeaderReport::SessionSignaled(value),
            _ => LeaderReport::Lost,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AUTHORIZED_TAG, LeaderReport};

    #[test]
    fn report_round_trips_through_the_wire_frame() {
        for report in [
            LeaderReport::AuthFailed,
            LeaderReport::UserDenied,
            LeaderReport::SessionLaunchFailed,
            LeaderReport::SessionSuccess,
            LeaderReport::SessionFailed(0),
            LeaderReport::SessionFailed(37),
            LeaderReport::SessionFailed(-1),
            LeaderReport::SessionSignaled(9),
            LeaderReport::Lost,
        ] {
            assert_eq!(LeaderReport::decode(report.encode()), report);
        }
    }

    #[test]
    fn unknown_tag_decodes_as_lost() {
        assert_eq!(LeaderReport::decode([99, 0, 0, 0, 0]), LeaderReport::Lost);
    }

    #[test]
    fn no_final_report_shares_the_authorized_tag() {
        // The phase-1 marker must never be a valid final report, or a parked
        // child could be mistaken for a finished one.
        for report in [
            LeaderReport::AuthFailed,
            LeaderReport::UserDenied,
            LeaderReport::SessionLaunchFailed,
            LeaderReport::SessionSuccess,
            LeaderReport::SessionFailed(1),
            LeaderReport::SessionSignaled(9),
        ] {
            assert_ne!(report.encode()[0], AUTHORIZED_TAG);
        }
    }
}
