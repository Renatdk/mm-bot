//! Core domain types.
//!
//! Цель:
//! - запретить "голые" f64 в бизнес-логике
//! - зафиксировать единицы измерения
//! - сделать ошибки очевидными на уровне типов

use std::fmt;
use std::ops::{Add, Div, Mul, Sub};

/// Цена актива (например ETH/USDT)
#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct Price(pub f64);

/// Количество актива (ETH)
#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct Qty(pub f64);

/// Денежная сумма (USDT)
#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct Money(pub f64);

/// Базисные пункты (1 bps = 0.01%)
#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct Bps(pub f64);

/// Доля / коэффициент (0.0 .. 1.0)
#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct Ratio(pub f64);

/// Время в миллисекундах (unix epoch)
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimestampMs(pub i64);

/// Эквити (стоимость портфеля)
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Equity {
    pub total: Money,
}

impl Equity {
    pub fn new(total: Money) -> Self {
        Self { total }
    }
}

//
// --- Conversions & helpers --------------------------------------------------
//

impl Bps {
    /// Перевод bps → коэффициент
    pub fn as_ratio(self) -> Ratio {
        Ratio(self.0 / 10_000.0)
    }
}

impl Ratio {
    pub fn clamp_01(self) -> Self {
        Ratio(self.0.clamp(0.0, 1.0))
    }
}

//
// --- Arithmetic (строго минимально) -----------------------------------------
//

impl Add for Money {
    type Output = Money;
    fn add(self, rhs: Money) -> Money {
        Money(self.0 + rhs.0)
    }
}

impl Sub for Money {
    type Output = Money;
    fn sub(self, rhs: Money) -> Money {
        Money(self.0 - rhs.0)
    }
}

impl Mul<Price> for Qty {
    type Output = Money;
    fn mul(self, price: Price) -> Money {
        Money(self.0 * price.0)
    }
}

impl Div<Price> for Money {
    type Output = Qty;
    fn div(self, price: Price) -> Qty {
        Qty(self.0 / price.0)
    }
}

//
// --- Display (для логов / телеги) -------------------------------------------
//

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.4}", self.0)
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.2}", self.0)
    }
}

impl fmt::Display for Bps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.2} bps", self.0)
    }
}
