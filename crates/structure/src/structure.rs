use core::types::Price;

use crate::atr::atr;
use crate::candle::Candle;
use crate::pivot::{is_pivot_high, is_pivot_low};

/// Параметры структуры
#[derive(Debug, Copy, Clone)]
pub struct StructureParams {
    pub pivot_k: usize,    // например 2
    pub min_atr_frac: f64, // например 0.3 (30% ATR)
}

/// Последняя подтверждённая структура
#[derive(Debug, Copy, Clone)]
pub struct MarketStructure {
    pub last_high: Option<Price>,
    pub last_low: Option<Price>,
}

/// Обновить структуру на новых данных
pub fn detect_structure(candles: &[Candle], params: StructureParams) -> MarketStructure {
    let atr_val = match atr(candles) {
        Some(v) => v,
        None => {
            return MarketStructure {
                last_high: None,
                last_low: None,
            };
        }
    };

    let min_move = atr_val.0 * params.min_atr_frac;

    let mut last_high = None;
    let mut last_low = None;

    for i in 0..candles.len() {
        if is_pivot_high(candles, i, params.pivot_k) {
            // проверяем, что после pivot был откат вниз >= min_move
            let hi = candles[i].high.0;

            let retraced = candles[i + 1..].iter().any(|c| hi - c.low.0 >= min_move);

            if retraced {
                last_high = Some(Price(hi));
            }
        }

        if is_pivot_low(candles, i, params.pivot_k) {
            let lo = candles[i].low.0;

            let retraced = candles[i + 1..].iter().any(|c| c.high.0 - lo >= min_move);

            if retraced {
                last_low = Some(Price(lo));
            }
        }
    }

    MarketStructure {
        last_high,
        last_low,
    }
}
