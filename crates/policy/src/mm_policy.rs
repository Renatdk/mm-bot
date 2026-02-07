use core::types::Ratio;

use structure::bos::BosState;
use structure::pullback::PullbackTracker;

/// Режим MM
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MmMode {
    Disabled,
    Normal,
    Defensive,
}

/// Причина решения (для логов / телеги)
#[derive(Debug, Copy, Clone)]
pub enum MmDecisionReason {
    NoConfirmedBos,
    NoPullback,
    InventoryOutsideSoftBand,
    InventoryOutsideHardBand,
    LtfStructureBroken,
    Ok,
}

/// Параметры policy
#[derive(Debug, Copy, Clone)]
pub struct MmPolicyParams {
    pub soft_min: Ratio,
    pub soft_max: Ratio,
    pub hard_min: Ratio,
    pub hard_max: Ratio,
}

/// Решение policy
#[derive(Debug, Copy, Clone)]
pub struct MmPolicyDecision {
    pub mode: MmMode,
    pub reason: MmDecisionReason,
}

/// Принятие решения: можно ли и как MM-ить
pub fn mm_policy_decision(
    bos_state: BosState,
    pullback: &PullbackTracker,
    base_ratio: Ratio,
    params: MmPolicyParams,
) -> MmPolicyDecision {
    // 1) BOS должен быть подтверждён
    if bos_state != BosState::Confirmed {
        return MmPolicyDecision {
            mode: MmMode::Disabled,
            reason: MmDecisionReason::NoConfirmedBos,
        };
    }

    // 2) должен быть pullback
    if !pullback.triggered {
        return MmPolicyDecision {
            mode: MmMode::Disabled,
            reason: MmDecisionReason::NoPullback,
        };
    }

    let r = base_ratio.0;

    // 3) hard band — MM запрещён
    if r < params.hard_min.0 || r > params.hard_max.0 {
        return MmPolicyDecision {
            mode: MmMode::Disabled,
            reason: MmDecisionReason::InventoryOutsideHardBand,
        };
    }

    // 4) soft band — Defensive
    if r < params.soft_min.0 || r > params.soft_max.0 {
        return MmPolicyDecision {
            mode: MmMode::Defensive,
            reason: MmDecisionReason::InventoryOutsideSoftBand,
        };
    }

    // 5) всё хорошо
    MmPolicyDecision {
        mode: MmMode::Normal,
        reason: MmDecisionReason::Ok,
    }
}
