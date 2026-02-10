use crate::trend_cause::TrendCause;
use crate::trend_state::TrendState;
use crate::trend_transition::trend_transition;

#[test]
fn trend_happy_path_long_then_flat() {
    let mut s = TrendState::Flat;
    s = trend_transition(s, TrendCause::EntrySignal).unwrap();
    s = trend_transition(s, TrendCause::ExitSignal).unwrap();
    assert_eq!(s, TrendState::Flat);
}

#[test]
fn stop_loss_transitions_to_flat() {
    let mut s = TrendState::Flat;
    s = trend_transition(s, TrendCause::EntrySignal).unwrap();
    s = trend_transition(s, TrendCause::StopLossHit).unwrap();
    assert_eq!(s, TrendState::Flat);
}

#[test]
fn illegal_flat_to_exit_is_rejected() {
    assert!(trend_transition(TrendState::Flat, TrendCause::ExitSignal).is_err());
}
