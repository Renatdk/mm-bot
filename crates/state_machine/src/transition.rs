use crate::cause::TransitionCause;
use crate::state::BotState;

#[derive(Debug, PartialEq, Eq)]
pub enum TransitionError {
    IllegalTransition {
        from: BotState,
        cause: TransitionCause,
    },
}

pub fn transition(state: BotState, cause: TransitionCause) -> Result<BotState, TransitionError> {
    let next = match (state, cause) {
        // --- Idle -----------------------------------------------------------
        (BotState::IdleUSDT, TransitionCause::HtfBosUpDetected) => BotState::BosPotential,

        // --- BOS potential --------------------------------------------------
        (BotState::BosPotential, TransitionCause::BosConfirmed) => BotState::BosConfirmed,
        (BotState::BosPotential, TransitionCause::BosFailed) => BotState::IdleUSDT,
        (BotState::BosPotential, TransitionCause::HtfBosDown) => BotState::IdleUSDT,

        // --- BOS confirmed --------------------------------------------------
        (BotState::BosConfirmed, TransitionCause::PullbackDetected) => BotState::Rebalancing,
        (BotState::BosConfirmed, TransitionCause::HtfBosDown) => BotState::IdleUSDT,

        // --- Rebalancing ----------------------------------------------------
        (BotState::Rebalancing, TransitionCause::RebalanceDone) => BotState::MMNormal,
        (BotState::Rebalancing, TransitionCause::RebalanceFailed) => BotState::Exiting,
        (BotState::Rebalancing, TransitionCause::HtfBosDown) => BotState::Exiting,

        // --- MM normal ------------------------------------------------------
        (BotState::MMNormal, TransitionCause::LtfBosDown) => BotState::MMDefensive,
        (BotState::MMNormal, TransitionCause::HtfBosDown) => BotState::Exiting,
        (BotState::MMNormal, TransitionCause::BreakEvenHit) => BotState::Exiting,
        (BotState::MMNormal, TransitionCause::BreakEvenWithFeesHit) => BotState::Exiting,

        // --- MM defensive ---------------------------------------------------
        (BotState::MMDefensive, TransitionCause::LtfStructureRecovered) => BotState::MMNormal,
        (BotState::MMDefensive, TransitionCause::HtfBosDown) => BotState::Exiting,
        (BotState::MMDefensive, TransitionCause::BreakEvenHit) => BotState::Exiting,
        (BotState::MMDefensive, TransitionCause::BreakEvenWithFeesHit) => BotState::Exiting,

        // --- Exiting --------------------------------------------------------
        (BotState::Exiting, TransitionCause::ExitDone) => BotState::IdleUSDT,

        // --- Illegal --------------------------------------------------------
        _ => return Err(TransitionError::IllegalTransition { from: state, cause }),
    };

    Ok(next)
}
