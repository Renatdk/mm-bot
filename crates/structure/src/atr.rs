use core::types::Price;

use crate::candle::Candle;

/// True Range для одной свечи
pub fn true_range(prev_close: Price, candle: &Candle) -> Price {
    let hl = candle.high.0 - candle.low.0;
    let hc = (candle.high.0 - prev_close.0).abs();
    let lc = (candle.low.0 - prev_close.0).abs();

    Price(hl.max(hc).max(lc))
}

/// Простая ATR (SMA), без EMA и оптимизаций
pub fn atr(candles: &[Candle]) -> Option<Price> {
    if candles.len() < 2 {
        return None;
    }

    let mut sum = 0.0;

    for i in 1..candles.len() {
        let tr = true_range(candles[i - 1].close, &candles[i]);
        sum += tr.0;
    }

    Some(Price(sum / (candles.len() as f64 - 1.0)))
}
