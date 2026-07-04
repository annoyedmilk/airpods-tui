//! Pure ownership state machine for iPhone <-> Linux audio handoff.
//!
//! Every handoff bug so far traced back to invisible boolean state spread
//! across the media controller. This FSM makes the ownership state explicit
//! and returns the side effects as data, so transitions are unit-testable
//! without Bluetooth or PulseAudio.

/// Settle window after a peer's source goes None before reclaiming. Long
/// enough to absorb the AirPods' transient None blip during handoff
/// (observed up to ~1s), short enough that reclaims feel snappy.
pub const RECLAIM_SETTLE_MS: u64 = 1500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ownership {
    /// No report from the device yet.
    #[default]
    Unknown,
    /// We hold (or just claimed) the audio session.
    Linux,
    /// A peer Apple device owns audio. `reclaim_when_silent` is armed when
    /// Linux was producing audio at steal time: once the peer goes quiet we
    /// take the session back.
    Peer { reclaim_when_silent: bool },
    /// The peer went silent; a reclaim fires when the settle window for
    /// `generation` expires, unless a fresher event supersedes it.
    ReclaimPending { generation: u64 },
}

/// Side effects the caller must execute, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Pause playing MPRIS players and remember them for ear-detection resume.
    PauseTracked,
    /// Pause without remembering them (ownership is gone; no auto-resume).
    PauseUntracked,
    /// Send OwnsConnection = 01.
    ClaimOwnership,
    /// Send OwnsConnection = 00.
    ReleaseOwnership,
    /// Start a settle timer that calls `on_settle_expired(generation)`.
    ScheduleReclaim {
        generation: u64,
    },
    /// Suspend/resume the bluez sink to force a fresh AVDTP_START.
    RestartAudioStream,
    ActivateA2dp,
    DeactivateA2dp,
}

#[derive(Debug, Default)]
pub struct HandoffFsm {
    state: Ownership,
    generation: u64,
}

impl HandoffFsm {
    pub fn state(&self) -> Ownership {
        self.state
    }

    fn reclaim_armed(&self) -> bool {
        matches!(
            self.state,
            Ownership::Peer {
                reclaim_when_silent: true
            } | Ownership::ReclaimPending { .. }
        )
    }

    /// AUDIO_SOURCE packet: who the AirPods say is playing.
    /// `linux_has_audio` is whether Linux was producing audio (MPRIS playing
    /// or a non-corked PulseAudio sink input) when the packet arrived.
    pub fn on_audio_source(
        &mut self,
        is_local: bool,
        is_none: bool,
        linux_has_audio: bool,
    ) -> Vec<Action> {
        if is_none {
            // Transient None blips during handoff are routinely followed by a
            // fresh peer/Media within ~1s, so never reclaim immediately:
            // schedule and let a newer event supersede via the generation.
            return if self.reclaim_armed() {
                self.generation += 1;
                self.state = Ownership::ReclaimPending {
                    generation: self.generation,
                };
                vec![Action::ScheduleReclaim {
                    generation: self.generation,
                }]
            } else {
                Vec::new()
            };
        }
        if is_local {
            self.state = Ownership::Linux;
            return Vec::new();
        }
        // A peer took the session. Stay armed if we already were.
        let armed = linux_has_audio || self.reclaim_armed();
        self.state = Ownership::Peer {
            reclaim_when_silent: armed,
        };
        vec![Action::PauseTracked]
    }

    /// Local media started playing (the caller has already verified the buds
    /// are in ear). Claims the session unless we already hold it, which
    /// stops claim/activate storms while a peer contests ownership.
    pub fn on_local_play(&mut self) -> Vec<Action> {
        if self.state == Ownership::Linux {
            return Vec::new();
        }
        self.state = Ownership::Linux;
        vec![Action::ClaimOwnership, Action::ActivateA2dp]
    }

    /// OwnsConnection report from the device (01 = we own, 00 = we lost it).
    pub fn on_owns_report(&mut self, owns: bool) -> Vec<Action> {
        if owns {
            self.state = Ownership::Linux;
            return Vec::new();
        }
        let armed = self.reclaim_armed();
        self.state = Ownership::Peer {
            reclaim_when_silent: armed,
        };
        vec![Action::PauseUntracked]
    }

    /// The settle timer for `generation` expired. Reclaims only when no fresher
    /// event replaced the pending state in the meantime.
    pub fn on_settle_expired(&mut self, generation: u64) -> Vec<Action> {
        if self.state != (Ownership::ReclaimPending { generation }) {
            return Vec::new();
        }
        self.state = Ownership::Linux;
        vec![Action::ClaimOwnership, Action::RestartAudioStream]
    }

    /// Smart-routing SetOwnershipToFalse request: the device asks us to
    /// hand the session over.
    pub fn on_ownership_to_false(&mut self) -> Vec<Action> {
        self.state = Ownership::Peer {
            reclaim_when_silent: false,
        };
        vec![
            Action::ReleaseOwnership,
            Action::PauseUntracked,
            Action::DeactivateA2dp,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer_steal(fsm: &mut HandoffFsm, linux_has_audio: bool) -> Vec<Action> {
        fsm.on_audio_source(false, false, linux_has_audio)
    }

    fn source_none(fsm: &mut HandoffFsm) -> Vec<Action> {
        fsm.on_audio_source(false, true, false)
    }

    #[test]
    fn peer_steal_pauses_and_arms_when_linux_had_audio() {
        let mut fsm = HandoffFsm::default();
        assert_eq!(peer_steal(&mut fsm, true), vec![Action::PauseTracked]);
        assert_eq!(
            fsm.state(),
            Ownership::Peer {
                reclaim_when_silent: true
            }
        );
    }

    #[test]
    fn peer_steal_without_local_audio_does_not_arm() {
        let mut fsm = HandoffFsm::default();
        assert_eq!(peer_steal(&mut fsm, false), vec![Action::PauseTracked]);
        assert_eq!(
            fsm.state(),
            Ownership::Peer {
                reclaim_when_silent: false
            }
        );
        // Peer going quiet must not trigger a reclaim.
        assert!(source_none(&mut fsm).is_empty());
    }

    #[test]
    fn none_after_armed_steal_schedules_reclaim() {
        let mut fsm = HandoffFsm::default();
        peer_steal(&mut fsm, true);
        let actions = source_none(&mut fsm);
        assert_eq!(actions, vec![Action::ScheduleReclaim { generation: 1 }]);
        assert_eq!(fsm.state(), Ownership::ReclaimPending { generation: 1 });
    }

    #[test]
    fn settle_expiry_reclaims_and_restarts_stream() {
        let mut fsm = HandoffFsm::default();
        peer_steal(&mut fsm, true);
        source_none(&mut fsm);
        assert_eq!(
            fsm.on_settle_expired(1),
            vec![Action::ClaimOwnership, Action::RestartAudioStream]
        );
        assert_eq!(fsm.state(), Ownership::Linux);
    }

    #[test]
    fn stale_settle_timer_is_ignored() {
        let mut fsm = HandoffFsm::default();
        peer_steal(&mut fsm, true);
        source_none(&mut fsm);
        source_none(&mut fsm); // reschedules with generation 2
        assert!(fsm.on_settle_expired(1).is_empty());
        assert_eq!(fsm.state(), Ownership::ReclaimPending { generation: 2 });
    }

    #[test]
    fn fresh_peer_media_during_settle_cancels_but_stays_armed() {
        let mut fsm = HandoffFsm::default();
        peer_steal(&mut fsm, true);
        source_none(&mut fsm);
        // The blip resolved into the peer playing again.
        assert_eq!(peer_steal(&mut fsm, false), vec![Action::PauseTracked]);
        assert_eq!(
            fsm.state(),
            Ownership::Peer {
                reclaim_when_silent: true
            }
        );
        // The stale timer must not reclaim.
        assert!(fsm.on_settle_expired(1).is_empty());
        // But the next quiet period arms a fresh reclaim.
        assert_eq!(
            source_none(&mut fsm),
            vec![Action::ScheduleReclaim { generation: 2 }]
        );
    }

    #[test]
    fn local_play_claims_once_then_stays_quiet() {
        let mut fsm = HandoffFsm::default();
        assert_eq!(
            fsm.on_local_play(),
            vec![Action::ClaimOwnership, Action::ActivateA2dp]
        );
        assert_eq!(fsm.state(), Ownership::Linux);
        // The tug-of-war fix: repeated play transitions while we own the
        // session must not spam claims and A2DP re-activations.
        assert!(fsm.on_local_play().is_empty());
        assert!(fsm.on_local_play().is_empty());
    }

    #[test]
    fn local_play_after_peer_steal_reclaims() {
        let mut fsm = HandoffFsm::default();
        fsm.on_local_play();
        peer_steal(&mut fsm, true);
        assert_eq!(
            fsm.on_local_play(),
            vec![Action::ClaimOwnership, Action::ActivateA2dp]
        );
        assert_eq!(fsm.state(), Ownership::Linux);
    }

    #[test]
    fn local_play_supersedes_pending_reclaim() {
        let mut fsm = HandoffFsm::default();
        peer_steal(&mut fsm, true);
        source_none(&mut fsm);
        assert!(!fsm.on_local_play().is_empty());
        // The scheduled timer fires into a superseded state and does nothing.
        assert!(fsm.on_settle_expired(1).is_empty());
        assert_eq!(fsm.state(), Ownership::Linux);
    }

    #[test]
    fn owns_report_false_pauses_untracked() {
        let mut fsm = HandoffFsm::default();
        fsm.on_local_play();
        assert_eq!(fsm.on_owns_report(false), vec![Action::PauseUntracked]);
        assert_eq!(
            fsm.state(),
            Ownership::Peer {
                reclaim_when_silent: false
            }
        );
    }

    #[test]
    fn owns_report_true_confirms_linux_silently() {
        let mut fsm = HandoffFsm::default();
        assert!(fsm.on_owns_report(true).is_empty());
        assert_eq!(fsm.state(), Ownership::Linux);
    }

    #[test]
    fn owns_report_false_keeps_armed_reclaim() {
        let mut fsm = HandoffFsm::default();
        peer_steal(&mut fsm, true);
        fsm.on_owns_report(false);
        assert_eq!(
            source_none(&mut fsm),
            vec![Action::ScheduleReclaim { generation: 1 }]
        );
    }

    #[test]
    fn local_source_report_confirms_linux() {
        let mut fsm = HandoffFsm::default();
        assert!(fsm.on_audio_source(true, false, false).is_empty());
        assert_eq!(fsm.state(), Ownership::Linux);
        // No claim needed on the next play transition.
        assert!(fsm.on_local_play().is_empty());
    }

    #[test]
    fn ownership_to_false_request_releases_and_deactivates() {
        let mut fsm = HandoffFsm::default();
        fsm.on_local_play();
        assert_eq!(
            fsm.on_ownership_to_false(),
            vec![
                Action::ReleaseOwnership,
                Action::PauseUntracked,
                Action::DeactivateA2dp,
            ]
        );
        assert_eq!(
            fsm.state(),
            Ownership::Peer {
                reclaim_when_silent: false
            }
        );
    }
}
