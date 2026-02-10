use core::types::{Price, TimestampMs};

use crate::candle::Candle;
use crate::structure::MarketStructure;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BosState {
    None,
    Potential,
    Confirmed,
    Failed,
}

#[derive(Debug, Copy, Clone)]
pub struct BosTracker {
    pub state: BosState,
    pub level: Option<Price>,
    pub started_at: Option<TimestampMs>,
    pub confirmed_candles: usize,
}

#[derive(Debug, Copy, Clone)]
pub struct BosParams {
    pub confirm_candles: usize,
    pub epsilon_frac: f64,
}

impl BosTracker {
    pub fn new() -> Self {
        Self {
            state: BosState::None,
            level: None,
            started_at: None,
            confirmed_candles: 0,
        }
    }

    pub fn on_candle_close(
        &mut self,
        candle: &Candle,
        structure: &MarketStructure,
        atr: Price,
        params: BosParams,
    ) {
        let epsilon = atr.0 * params.epsilon_frac;

        match self.state {
            BosState::None => {
                if let Some(high) = structure.last_high {
                    if candle.close.0 > high.0 + epsilon {
                        self.state = BosState::Potential;
                        self.level = Some(high);
                        self.started_at = Some(candle.ts);
                        // считаем пробойную свечу как 1 подтверждение
                        self.confirmed_candles = 1;

                        if self.confirmed_candles >= params.confirm_candles {
                            self.state = BosState::Confirmed;
                        }
                    }
                }
            }

            BosState::Potential => {
                let level = self.level.expect("level must exist");

                // пробой отменился -> сразу возвращаемся в поиск нового BOS
                if candle.close.0 <= level.0 {
                    self.reset();
                    return;
                }

                if candle.close.0 > level.0 + epsilon {
                    self.confirmed_candles += 1;
                }

                if self.confirmed_candles >= params.confirm_candles {
                    self.state = BosState::Confirmed;
                }
            }

            BosState::Confirmed => {
                // опционально: если структура сломалась вниз, начинаем поиск заново
                if let Some(level) = self.level {
                    if candle.close.0 <= level.0 {
                        self.reset();
                    }
                }
            }

            BosState::Failed => {
                // safety net: не залипаем
                self.reset();
            }
        }
    }

    pub fn reset(&mut self) {
        self.state = BosState::None;
        self.level = None;
        self.started_at = None;
        self.confirmed_candles = 0;
    }
}
