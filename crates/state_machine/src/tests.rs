use crate::cause::TransitionCause;
use crate::state::BotState;
use crate::transition::transition;

#[test]
fn happy_path_full_cycle() {
    let mut s = BotState::IdleUSDT;

    s = transition(s, TransitionCause::HtfBosUpDetected).unwrap();
    s = transition(s, TransitionCause::BosConfirmed).unwrap();
    s = transition(s, TransitionCause::PullbackDetected).unwrap();
    s = transition(s, TransitionCause::RebalanceDone).unwrap();
    s = transition(s, TransitionCause::LtfBosDown).unwrap();
    s = transition(s, TransitionCause::BreakEvenWithFeesHit).unwrap();
    s = transition(s, TransitionCause::ExitDone).unwrap();

    assert_eq!(s, BotState::IdleUSDT);
}

#[test]
fn illegal_transition_from_idle() {
    assert!(transition(BotState::IdleUSDT, TransitionCause::RebalanceDone).is_err());
}

#[test]
fn cannot_skip_bos_confirmation() {
    assert!(transition(BotState::IdleUSDT, TransitionCause::PullbackDetected).is_err());
}
