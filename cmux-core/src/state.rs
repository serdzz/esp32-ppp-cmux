//! Per-DLC channel state from the host (initiator) perspective.
//!
//! The multiplexer brings each channel up with a SABM → UA exchange and
//! tears it down with DISC → UA. We do not implement responder semantics
//! (the SIM800L is the responder), so transitions are unidirectional.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DlcState {
    Closed,
    /// SABM sent, waiting for UA from peer.
    Opening,
    Open,
    /// DISC sent, waiting for UA from peer.
    Closing,
}

impl DlcState {
    pub fn on_sabm_sent(self) -> Self {
        match self {
            Self::Closed => Self::Opening,
            other => other,
        }
    }

    pub fn on_disc_sent(self) -> Self {
        match self {
            Self::Open => Self::Closing,
            other => other,
        }
    }

    pub fn on_ua_received(self) -> Self {
        match self {
            Self::Opening => Self::Open,
            Self::Closing => Self::Closed,
            other => other,
        }
    }

    /// DM (Disconnected Mode) from the peer always lands the channel in Closed.
    pub fn on_dm_received(self) -> Self {
        Self::Closed
    }

    pub fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_close_round_trip() {
        let s = DlcState::Closed
            .on_sabm_sent()
            .on_ua_received();
        assert_eq!(s, DlcState::Open);

        let s = s.on_disc_sent().on_ua_received();
        assert_eq!(s, DlcState::Closed);
    }

    #[test]
    fn dm_in_any_state_closes() {
        for s in [DlcState::Closed, DlcState::Opening, DlcState::Open, DlcState::Closing] {
            assert_eq!(s.on_dm_received(), DlcState::Closed);
        }
    }
}
