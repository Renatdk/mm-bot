use crate::trend_cause::TrendCause;
use crate::trend_state::TrendState;

#[derive(Debug, PartialEq, Eq)]
pub enum TrendTransitionError {
    IllegalTransition {
        from: TrendState,
        cause: TrendCause,
    },
}

pub fn trend_transition(
    state: TrendState,
    cause: TrendCause,
) -> Result<TrendState, TrendTransitionError> {
    let next = match (state, cause) {
        (TrendState::Flat, TrendCause::EntrySignal) => TrendState::Long,

        (TrendState::Long, TrendCause::ExitSignal) => TrendState::Flat,
        (TrendState::Long, TrendCause::StopLossHit) => TrendState::Flat,
        (TrendState::Long, TrendCause::ForceFlat) => TrendState::Flat,

        _ => {
            return Err(TrendTransitionError::IllegalTransition {
                from: state,
                cause,
            });
        }
    };

    Ok(next)
}
