use core::types::Price;

use structure::atr::atr;
use structure::candle::Candle;

pub struct CandleFeed {
    pub window: usize,
    pub candles: Vec<Candle>,
}

impl CandleFeed {
    pub fn new(window: usize) -> Self {
        Self {
            window,
            candles: Vec::with_capacity(window + 8),
        }
    }

    pub fn push(&mut self, c: Candle) {
        self.candles.push(c);

        // держим последний window
        if self.candles.len() > self.window {
            let excess = self.candles.len() - self.window;
            self.candles.drain(0..excess);
        }
    }

    pub fn atr(&self) -> Option<Price> {
        atr(&self.candles)
    }

    /// mid price = close последней свечи
    pub fn mid(&self) -> Option<Price> {
        self.candles.last().map(|c| c.close)
    }
}
