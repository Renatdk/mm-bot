use core::types::{Price, Qty};

/// Режим тренд-стратегии (spot, long-only)
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TrendMode {
    Flat,
    Long,
}

/// Действие стратегии на текущем баре
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TrendAction {
    HoldFlat,
    EnterLong,
    HoldLong,
    ExitLong,
}

/// Причина решения (для логов/метрик)
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TrendDecisionReason {
    TrendUpEntry,
    TrendDown,
    AtrStopHit,
    NoSignal,
    InvalidLongOnlyInvariant,
    MissingEntryPrice,
}

/// Параметры trend-policy
#[derive(Debug, Copy, Clone)]
pub struct TrendPolicyParams {
    /// Стоп = entry - atr_stop_mult * ATR
    pub atr_stop_mult: f64,
}

/// Вход для принятия решения
#[derive(Debug, Copy, Clone)]
pub struct TrendPolicyInput {
    pub close: Price,
    pub atr: Price,
    pub ema_fast: Price,
    pub ema_slow: Price,
    pub position_qty: Qty,
    pub entry_price: Option<Price>,
}

/// Результат решения
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TrendPolicyDecision {
    pub next_mode: TrendMode,
    pub action: TrendAction,
    pub reason: TrendDecisionReason,
}

pub fn trend_policy_decision(
    mode: TrendMode,
    input: TrendPolicyInput,
    params: TrendPolicyParams,
) -> TrendPolicyDecision {
    // Long-only invariant: short позиция запрещена.
    if input.position_qty.0 < 0.0 {
        return TrendPolicyDecision {
            next_mode: TrendMode::Flat,
            action: TrendAction::ExitLong,
            reason: TrendDecisionReason::InvalidLongOnlyInvariant,
        };
    }

    let trend_up = input.ema_fast.0 > input.ema_slow.0;
    let trend_down = input.ema_fast.0 < input.ema_slow.0;

    match mode {
        TrendMode::Flat => {
            if input.position_qty.0 > 0.0 {
                // Safety: режим flat с открытой позицией нормализуем к long.
                return TrendPolicyDecision {
                    next_mode: TrendMode::Long,
                    action: TrendAction::HoldLong,
                    reason: TrendDecisionReason::NoSignal,
                };
            }

            if trend_up {
                return TrendPolicyDecision {
                    next_mode: TrendMode::Long,
                    action: TrendAction::EnterLong,
                    reason: TrendDecisionReason::TrendUpEntry,
                };
            }

            TrendPolicyDecision {
                next_mode: TrendMode::Flat,
                action: TrendAction::HoldFlat,
                reason: TrendDecisionReason::NoSignal,
            }
        }
        TrendMode::Long => {
            if input.position_qty.0 == 0.0 {
                return TrendPolicyDecision {
                    next_mode: TrendMode::Flat,
                    action: TrendAction::HoldFlat,
                    reason: TrendDecisionReason::NoSignal,
                };
            }

            let Some(entry) = input.entry_price else {
                return TrendPolicyDecision {
                    next_mode: TrendMode::Flat,
                    action: TrendAction::ExitLong,
                    reason: TrendDecisionReason::MissingEntryPrice,
                };
            };

            if trend_down {
                return TrendPolicyDecision {
                    next_mode: TrendMode::Flat,
                    action: TrendAction::ExitLong,
                    reason: TrendDecisionReason::TrendDown,
                };
            }

            let stop = entry.0 - params.atr_stop_mult.max(0.0) * input.atr.0.max(0.0);
            if input.close.0 <= stop {
                return TrendPolicyDecision {
                    next_mode: TrendMode::Flat,
                    action: TrendAction::ExitLong,
                    reason: TrendDecisionReason::AtrStopHit,
                };
            }

            TrendPolicyDecision {
                next_mode: TrendMode::Long,
                action: TrendAction::HoldLong,
                reason: TrendDecisionReason::NoSignal,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> TrendPolicyParams {
        TrendPolicyParams { atr_stop_mult: 2.5 }
    }

    #[test]
    fn enters_long_on_trend_up_when_flat() {
        let d = trend_policy_decision(
            TrendMode::Flat,
            TrendPolicyInput {
                close: Price(100.0),
                atr: Price(1.0),
                ema_fast: Price(101.0),
                ema_slow: Price(99.0),
                position_qty: Qty(0.0),
                entry_price: None,
            },
            params(),
        );

        assert_eq!(d.next_mode, TrendMode::Long);
        assert_eq!(d.action, TrendAction::EnterLong);
        assert_eq!(d.reason, TrendDecisionReason::TrendUpEntry);
    }

    #[test]
    fn stays_flat_without_entry_signal() {
        let d = trend_policy_decision(
            TrendMode::Flat,
            TrendPolicyInput {
                close: Price(100.0),
                atr: Price(1.0),
                ema_fast: Price(99.0),
                ema_slow: Price(101.0),
                position_qty: Qty(0.0),
                entry_price: None,
            },
            params(),
        );

        assert_eq!(d.next_mode, TrendMode::Flat);
        assert_eq!(d.action, TrendAction::HoldFlat);
    }

    #[test]
    fn exits_long_on_trend_down() {
        let d = trend_policy_decision(
            TrendMode::Long,
            TrendPolicyInput {
                close: Price(100.0),
                atr: Price(1.0),
                ema_fast: Price(99.0),
                ema_slow: Price(101.0),
                position_qty: Qty(1.0),
                entry_price: Some(Price(95.0)),
            },
            params(),
        );

        assert_eq!(d.next_mode, TrendMode::Flat);
        assert_eq!(d.action, TrendAction::ExitLong);
        assert_eq!(d.reason, TrendDecisionReason::TrendDown);
    }

    #[test]
    fn exits_long_on_atr_stop() {
        let d = trend_policy_decision(
            TrendMode::Long,
            TrendPolicyInput {
                close: Price(96.0),
                atr: Price(2.0),
                ema_fast: Price(103.0),
                ema_slow: Price(100.0),
                position_qty: Qty(1.0),
                entry_price: Some(Price(102.0)),
            },
            TrendPolicyParams { atr_stop_mult: 2.5 }, // stop=97
        );

        assert_eq!(d.next_mode, TrendMode::Flat);
        assert_eq!(d.action, TrendAction::ExitLong);
        assert_eq!(d.reason, TrendDecisionReason::AtrStopHit);
    }

    #[test]
    fn rejects_negative_position_for_long_only() {
        let d = trend_policy_decision(
            TrendMode::Long,
            TrendPolicyInput {
                close: Price(100.0),
                atr: Price(1.0),
                ema_fast: Price(101.0),
                ema_slow: Price(99.0),
                position_qty: Qty(-0.1),
                entry_price: Some(Price(100.0)),
            },
            params(),
        );

        assert_eq!(d.next_mode, TrendMode::Flat);
        assert_eq!(d.action, TrendAction::ExitLong);
        assert_eq!(d.reason, TrendDecisionReason::InvalidLongOnlyInvariant);
    }
}
