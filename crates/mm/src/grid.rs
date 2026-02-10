use core::types::{Bps, Money, Price, Qty, Ratio};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Copy, Clone)]
pub struct DesiredOrder {
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
}

/// Параметры “сетки, которая держит форму”
#[derive(Debug, Copy, Clone)]
pub struct GridParams {
    /// Сколько уровней на сторону (например 6)
    pub levels: usize,

    /// Шаг сетки в bps (например 12 bps = 0.12%)
    pub step: Bps,

    /// Базовый размер заявки в USDT (например 25 USDT)
    pub base_quote_per_order: Money,

    /// max усиливаем размер от дисбаланса инвентаря
    pub max_size_mult: f64, // например 2.0

    /// Инвентарь: soft band (например 0.40..0.60)
    pub soft_min: Ratio,
    pub soft_max: Ratio,

    /// Инвентарь: hard band (например 0.35..0.65)
    pub hard_min: Ratio,
    pub hard_max: Ratio,

    /// Минимальный размер в базовой валюте (exchange limits)
    pub min_base_qty: Qty,
}

/// Контекст сетки: что сейчас у нас в портфеле
#[derive(Debug, Copy, Clone)]
pub struct Inventory {
    pub base: Qty,
    pub quote: Money,
}

/// Equity в USDT
pub fn equity(inv: Inventory, mid: Price) -> Money {
    Money(inv.quote.0 + inv.base.0 * mid.0)
}

/// Доля base по стоимости (0..1)
pub fn base_ratio(inv: Inventory, mid: Price) -> Option<Ratio> {
    let e = equity(inv, mid).0;
    if e <= 0.0 {
        return None;
    }
    Some(Ratio((inv.base.0 * mid.0) / e))
}

/// bps → множитель цены
fn bps_factor(bps: Bps) -> f64 {
    1.0 + (bps.0 / 10_000.0)
}

/// Формирует сетку лимиток вокруг anchor.
/// - buy ниже anchor, sell выше anchor
/// - размеры адаптивны к inventory ratio (подталкивают к 50/50)
pub fn build_grid(
    anchor: Price,
    mid: Price,
    inv: Inventory,
    params: GridParams,
) -> Option<Vec<DesiredOrder>> {
    if params.levels == 0 || mid.0 <= 0.0 || anchor.0 <= 0.0 {
        return None;
    }

    // Spot long-only invariants:
    // - no negative holdings
    // - no synthetic leverage by overspending quote
    if inv.base.0 < 0.0 || inv.quote.0 < 0.0 {
        return None;
    }

    let r = base_ratio(inv, mid)?.0;

    // Если вышли за hard band — сетку строить нельзя (пусть policy/engine выведет)
    if r < params.hard_min.0 || r > params.hard_max.0 {
        return None;
    }

    // В soft band — норм.
    // Вне soft band, но внутри hard — усиливаем “нужную” сторону.
    let target = 0.5;
    let dist = (r - target).abs();

    // dist=0 -> mult=1
    // dist растёт -> mult до max_size_mult
    let mult = 1.0 + (params.max_size_mult - 1.0) * (dist / 0.5).min(1.0);

    let mut out: Vec<DesiredOrder> = Vec::with_capacity(params.levels * 2);
    let mut remaining_base = inv.base.0;
    let mut remaining_quote = inv.quote.0;

    for level in 1..=params.levels {
        let step_bps = Bps(params.step.0 * level as f64);

        // цены уровней
        let buy_price = Price(anchor.0 / bps_factor(step_bps)); // ниже
        let sell_price = Price(anchor.0 * bps_factor(step_bps)); // выше

        // базовый qty = base_quote_per_order / price
        let base_qty_buy = Qty(params.base_quote_per_order.0 / buy_price.0);
        let base_qty_sell = Qty(params.base_quote_per_order.0 / sell_price.0);

        // адаптация размеров:
        // - если base слишком много (r > 0.5): уменьшаем BUY и увеличиваем SELL
        // - если base мало (r < 0.5): увеличиваем BUY и уменьшаем SELL
        let (buy_mult, sell_mult) = if r > target {
            (1.0 / mult, mult)
        } else if r < target {
            (mult, 1.0 / mult)
        } else {
            (1.0, 1.0)
        };

        let desired_buy_qty = base_qty_buy.0 * buy_mult;
        let desired_sell_qty = base_qty_sell.0 * sell_mult;

        // Reserve quote/base so desired orders are executable in spot long-only mode.
        let max_buy_qty_by_quote = if buy_price.0 > 0.0 {
            remaining_quote / buy_price.0
        } else {
            0.0
        };
        let buy_qty = Qty(desired_buy_qty.min(max_buy_qty_by_quote).max(0.0));
        let sell_qty = Qty(desired_sell_qty.min(remaining_base).max(0.0));

        // фильтр минимального количества (биржевые лимиты)
        if buy_qty.0 >= params.min_base_qty.0 {
            remaining_quote -= buy_qty.0 * buy_price.0;
            out.push(DesiredOrder {
                side: Side::Buy,
                price: buy_price,
                qty: buy_qty,
            });
        }

        if sell_qty.0 >= params.min_base_qty.0 {
            remaining_base -= sell_qty.0;
            out.push(DesiredOrder {
                side: Side::Sell,
                price: sell_price,
                qty: sell_qty,
            });
        }
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> GridParams {
        GridParams {
            levels: 3,
            step: Bps(10.0), // 0.10%
            base_quote_per_order: Money(50.0),
            max_size_mult: 2.0,
            soft_min: Ratio(0.40),
            soft_max: Ratio(0.60),
            hard_min: Ratio(0.35),
            hard_max: Ratio(0.65),
            min_base_qty: Qty(0.0001),
        }
    }

    #[test]
    fn builds_orders() {
        let inv = Inventory {
            base: Qty(1.0),
            quote: Money(1000.0),
        };
        let mid = Price(1000.0);
        let anchor = Price(1000.0);
        let orders = build_grid(anchor, mid, inv, params()).unwrap();
        assert!(!orders.is_empty());
    }

    #[test]
    fn returns_none_if_outside_hard_band() {
        let inv = Inventory {
            base: Qty(10.0),
            quote: Money(10.0),
        }; // почти всё в base
        let mid = Price(1000.0);
        let anchor = Price(1000.0);
        let orders = build_grid(anchor, mid, inv, params());
        assert!(orders.is_none());
    }

    #[test]
    fn caps_total_sell_qty_to_available_base() {
        let inv = Inventory {
            base: Qty(0.02),
            quote: Money(20.0),
        };
        let mid = Price(1000.0);
        let anchor = Price(1000.0);

        let orders = build_grid(anchor, mid, inv, params()).unwrap();
        let total_sell_qty: f64 = orders
            .iter()
            .filter(|o| o.side == Side::Sell)
            .map(|o| o.qty.0)
            .sum();

        assert!(total_sell_qty <= inv.base.0 + 1e-9);
    }

    #[test]
    fn caps_total_buy_notional_to_available_quote() {
        let inv = Inventory {
            base: Qty(0.02),
            quote: Money(20.0),
        };
        let mid = Price(1000.0);
        let anchor = Price(1000.0);

        let orders = build_grid(anchor, mid, inv, params()).unwrap();
        let total_buy_notional: f64 = orders
            .iter()
            .filter(|o| o.side == Side::Buy)
            .map(|o| o.qty.0 * o.price.0)
            .sum();

        assert!(total_buy_notional <= inv.quote.0 + 1e-9);
    }

    #[test]
    fn over_target_base_biases_toward_sells() {
        let inv = Inventory {
            base: Qty(6.0),
            quote: Money(4000.0),
        }; // r = 0.6 at mid=1000
        let mid = Price(1000.0);
        let anchor = Price(1000.0);

        let orders = build_grid(anchor, mid, inv, params()).unwrap();
        let total_buy_qty: f64 = orders
            .iter()
            .filter(|o| o.side == Side::Buy)
            .map(|o| o.qty.0)
            .sum();
        let total_sell_qty: f64 = orders
            .iter()
            .filter(|o| o.side == Side::Sell)
            .map(|o| o.qty.0)
            .sum();

        assert!(total_sell_qty > total_buy_qty);
    }

    #[test]
    fn under_target_base_biases_toward_buys() {
        let inv = Inventory {
            base: Qty(4.0),
            quote: Money(6000.0),
        }; // r = 0.4 at mid=1000
        let mid = Price(1000.0);
        let anchor = Price(1000.0);

        let orders = build_grid(anchor, mid, inv, params()).unwrap();
        let total_buy_qty: f64 = orders
            .iter()
            .filter(|o| o.side == Side::Buy)
            .map(|o| o.qty.0)
            .sum();
        let total_sell_qty: f64 = orders
            .iter()
            .filter(|o| o.side == Side::Sell)
            .map(|o| o.qty.0)
            .sum();

        assert!(total_buy_qty > total_sell_qty);
    }
}
