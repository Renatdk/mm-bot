use policy::mm_policy::{MmDecisionReason, MmMode};
use state_machine::cause::TransitionCause;
use state_machine::state::BotState;

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Transition {
        from: BotState,
        cause: TransitionCause,
        to: BotState,
    },
    PolicyDecision {
        mode: MmMode,
        reason: MmDecisionReason,
    },
    Log(String),
}
