use crate::candle::Candle;
use core::types::Price;

/// Тип пивота
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PivotKind {
    High,
    Low,
}

/// Подтверждённый pivot
#[derive(Debug, Copy, Clone)]
pub struct Pivot {
    pub index: usize,
    pub price: Price,
    pub kind: PivotKind,
}

/// Проверка: является ли свеча pivot high
pub fn is_pivot_high(candles: &[Candle], i: usize, k: usize) -> bool {
    if i < k || i + k >= candles.len() {
        return false;
    }

    let hi = candles[i].high.0;

    candles[i - k..i].iter().all(|c| c.high.0 < hi)
        && candles[i + 1..=i + k].iter().all(|c| c.high.0 < hi)
}

/// Проверка: является ли свеча pivot low
pub fn is_pivot_low(candles: &[Candle], i: usize, k: usize) -> bool {
    if i < k || i + k >= candles.len() {
        return false;
    }

    let lo = candles[i].low.0;

    candles[i - k..i].iter().all(|c| c.low.0 > lo)
        && candles[i + 1..=i + k].iter().all(|c| c.low.0 > lo)
}
