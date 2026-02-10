use core::types::Price;

use state_machine::cause::TransitionCause;
use state_machine::state::BotState;
use state_machine::transition::{TransitionError, transition};

use structure::bos::BosTracker;
use structure::pullback::PullbackTracker;

use mm::grid::{Inventory, base_ratio};

use policy::mm_policy::{MmMode, MmPolicyParams, mm_policy_decision};

/// Решение MM policy -> вызывает изменения state machine.
/// Здесь мы НЕ выставляем ордера. Только режим.
pub fn drive_once(
    state: BotState,
    bos: &BosTracker,
    pullback: &PullbackTracker,
    inv: Inventory,
    mid: Price,
    mm_policy: MmPolicyParams,
) -> Result<BotState, TransitionError> {
    let r = match base_ratio(inv, mid) {
        Some(x) => x,
        None => return Ok(state),
    };

    let decision = mm_policy_decision(bos.state, pullback, r, mm_policy);

    match (state, decision.mode) {
        (BotState::MMNormal | BotState::MMDefensive, MmMode::Disabled) => {
            transition(state, TransitionCause::HtfBosDown)
        }
        (BotState::Rebalancing, MmMode::Normal | MmMode::Defensive) => {
            transition(state, TransitionCause::RebalanceDone)
        }
        _ => Ok(state),
    }
}
