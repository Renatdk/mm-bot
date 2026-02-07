use core::types::TimestampMs;
use core::types::{Price, Qty};

#[derive(Debug, Copy, Clone)]
pub struct Candle {
    pub ts: TimestampMs,
    pub open: Price,
    pub high: Price,
    pub low: Price,
    pub close: Price,
    pub volume: Qty,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Timeframe {
    Min1,
    Min5,
    Min15,
}

impl Timeframe {
    pub fn as_millis(self) -> i64 {
        match self {
            Timeframe::Min1 => 60_000,
            Timeframe::Min5 => 5 * 60_000,
            Timeframe::Min15 => 15 * 60_000,
        }
    }
}
