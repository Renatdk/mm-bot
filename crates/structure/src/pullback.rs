use core::types::Price;

use crate::bos::{BosState, BosTracker};

use crate::candle::Candle;

/// Параметры pullback
#[derive(Debug, Copy, Clone)]
pub struct PullbackParams {
    pub epsilon_frac: f64, // например 0.1 ATR
    pub retrace_frac: f64, // 0.3 .. 0.5
}

/// Детектор pullback (sidecar)
#[derive(Debug, Copy, Clone)]
pub struct PullbackTracker {
    pub max_price_after_bos: Option<Price>,
    pub triggered: bool,
}

impl PullbackTracker {
    pub fn new() -> Self {
        Self {
            max_price_after_bos: None,
            triggered: false,
        }
    }

    /// Обновление на каждой новой закрытой свече
    pub fn on_candle_close(
        &mut self,
        candle: &Candle,
        bos: &BosTracker,
        atr: Price,
        params: PullbackParams,
    ) {
        if bos.state != BosState::Confirmed || self.triggered {
            return;
        }

        let bos_level = match bos.level {
            Some(l) => l,
            None => return,
        };

        // обновляем максимум после BOS
        self.max_price_after_bos = match self.max_price_after_bos {
            Some(max) => Some(Price(max.0.max(candle.high.0))),
            None => Some(candle.high),
        };

        let max_price = self.max_price_after_bos.unwrap();
        let impulse = max_price.0 - bos_level.0;
        if impulse <= 0.0 {
            return;
        }

        // Условие A: возврат к BOS уровню
        let epsilon = atr.0 * params.epsilon_frac;
        if (candle.close.0 - bos_level.0).abs() <= epsilon {
            self.triggered = true;
            return;
        }

        // Условие B: откат импульса
        let retrace = max_price.0 - candle.close.0;
        if retrace >= impulse * params.retrace_frac {
            self.triggered = true;
        }
    }

    pub fn reset(&mut self) {
        self.max_price_after_bos = None;
        self.triggered = false;
    }
}
