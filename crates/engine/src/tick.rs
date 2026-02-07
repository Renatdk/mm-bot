use core::types::Price;

use state_machine::cause::TransitionCause;
use state_machine::state::BotState;
use state_machine::transition::transition;

use structure::bos::{BosParams, BosTracker};
use structure::pullback::{PullbackParams, PullbackTracker};

use mm::grid::GridParams;
use mm::grid::{Inventory, base_ratio, build_grid};

use policy::mm_policy::{MmMode, MmPolicyParams, mm_policy_decision};

use crate::event::EngineEvent;

/// Engine runtime context (живёт между тиками)
pub struct EngineCtx {
    pub state: BotState,

    // structure sidecars
    pub bos: BosTracker,
    pub pullback: PullbackTracker,

    // config
    pub mm_policy: MmPolicyParams,
    pub grid: GridParams,
    pub bos_params: BosParams,
    pub pullback_params: PullbackParams,
}

impl EngineCtx {
    pub fn new(
        state: BotState,
        mm_policy: MmPolicyParams,
        grid: GridParams,
        bos_params: BosParams,
        pullback_params: PullbackParams,
    ) -> Self {
        Self {
            state,
            bos: BosTracker::new(),
            pullback: PullbackTracker::new(),
            mm_policy,
            grid,
            bos_params,
            pullback_params,
        }
    }
}

/// Вход тик-данных (пока мок)
#[derive(Debug, Copy, Clone)]
pub struct TickInput {
    pub mid: Price,
    pub atr: Price,
    pub inv: Inventory,
    pub ltf_broken_down: bool,
    pub ltf_recovered: bool,
}

/// Один тик мышления.
/// Возвращает события (для логов/телеги/хранилища).
pub fn tick(ctx: &mut EngineCtx, input: TickInput) -> Vec<EngineEvent> {
    let _ = ctx.bos_params;
    let _ = ctx.pullback_params;
    let _ = input.atr;

    let mut events = Vec::new();

    // --- 2) policy decision ---
    let r = match base_ratio(input.inv, input.mid) {
        Some(x) => x,
        None => {
            events.push(EngineEvent::Log("base_ratio unavailable".into()));
            return events;
        }
    };

    let decision = mm_policy_decision(ctx.bos.state, &ctx.pullback, r, ctx.mm_policy);

    events.push(EngineEvent::PolicyDecision {
        mode: decision.mode,
        reason: decision.reason,
    });

    // --- 3) state machine causes (минимальный набор) ---
    // Pullback -> разрешение ребаланса
    if ctx.pullback.triggered {
        if let Ok(next) = transition(ctx.state, TransitionCause::PullbackDetected) {
            events.push(EngineEvent::Transition {
                from: ctx.state,
                cause: TransitionCause::PullbackDetected,
                to: next,
            });
            ctx.state = next;
        }
    }

    // LTF signals
    if input.ltf_broken_down {
        if let Ok(next) = transition(ctx.state, TransitionCause::LtfBosDown) {
            events.push(EngineEvent::Transition {
                from: ctx.state,
                cause: TransitionCause::LtfBosDown,
                to: next,
            });
            ctx.state = next;
        }
    }

    if input.ltf_recovered {
        if let Ok(next) = transition(ctx.state, TransitionCause::LtfStructureRecovered) {
            events.push(EngineEvent::Transition {
                from: ctx.state,
                cause: TransitionCause::LtfStructureRecovered,
                to: next,
            });
            ctx.state = next;
        }
    }

    // Policy disabled while in MM -> exit intent
    if matches!(ctx.state, BotState::MMNormal | BotState::MMDefensive)
        && decision.mode == MmMode::Disabled
    {
        if let Ok(next) = transition(ctx.state, TransitionCause::HtfBosDown) {
            events.push(EngineEvent::Transition {
                from: ctx.state,
                cause: TransitionCause::HtfBosDown,
                to: next,
            });
            ctx.state = next;
        }
    }

    // --- 4) build desired grid when MM is allowed ---
    if matches!(decision.mode, MmMode::Normal | MmMode::Defensive) {
        // anchor пока = mid (позже будет BOS level / last fill / VWAP)
        let anchor = input.mid;

        if let Some(orders) = build_grid(anchor, input.mid, input.inv, ctx.grid) {
            events.push(EngineEvent::Log(format!(
                "desired_orders: {}",
                orders.len()
            )));
        } else {
            events.push(EngineEvent::Log(
                "grid disabled by hard band or invalid inputs".into(),
            ));
        }
    }

    events
}
