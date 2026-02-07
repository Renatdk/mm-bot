use core::types::{Money, Price, Qty, Ratio};

#[derive(Debug, Copy, Clone)]
pub struct Portfolio {
    /// Кол-во ETH
    pub base: Qty,
    /// Кол-во USDT
    pub quote: Money,
}

#[derive(Debug, Copy, Clone)]
pub struct RebalanceParams {
    /// Целевая доля ETH по стоимости (например 0.50)
    pub target_base_ratio: Ratio,
    /// Допуск (например 0.02 = 2%)
    pub tolerance: Ratio,
    /// Комиссия в долях (например 0.001 = 0.1%)
    pub fee_rate: Ratio,
    /// Минимальная сумма сделки (например 5 USDT)
    pub min_quote_trade: Money,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum RebalanceDecision {
    /// Купить base_qty ETH (за USDT)
    BuyBase(Qty),
    /// Продать base_qty ETH (получим USDT)
    SellBase(Qty),
    /// Уже достаточно близко к цели
    Noop,
}

/// Оценка equity в USDT
pub fn equity(p: Portfolio, mid: Price) -> Money {
    p.quote + (p.base * mid)
}

/// Текущая доля ETH по стоимости: (base*price)/equity
pub fn base_ratio(p: Portfolio, mid: Price) -> Option<Ratio> {
    let e = equity(p, mid).0;
    if e <= 0.0 {
        return None;
    }
    Some(Ratio((p.base.0 * mid.0) / e))
}

/// Решение ребаланса к target_base_ratio (обычно 0.50)
pub fn rebalance_decision(
    p: Portfolio,
    mid: Price,
    params: RebalanceParams,
) -> Option<RebalanceDecision> {
    let e = equity(p, mid).0;
    if e <= 0.0 || mid.0 <= 0.0 {
        return None;
    }

    let target = params.target_base_ratio.0;
    let tol = params.tolerance.0;

    // текущая стоимость base в USDT
    let base_value = p.base.0 * mid.0;
    let current = base_value / e;

    // если уже в допуске — ничего не делаем
    if (current - target).abs() <= tol {
        return Some(RebalanceDecision::Noop);
    }

    // target_base_value = target * equity
    let target_base_value = target * e;

    // delta_value: сколько USDT стоимости base надо докупить/продать
    let delta_value = target_base_value - base_value;

    // учтём комиссию консервативно:
    // покупка: нужно чуть больше USDT
    // продажа: получим чуть меньше USDT
    let fee = params.fee_rate.0;

    if delta_value > 0.0 {
        // BUY
        let quote_needed = delta_value * (1.0 + fee);
        if quote_needed < params.min_quote_trade.0 {
            return Some(RebalanceDecision::Noop);
        }
        if quote_needed > p.quote.0 {
            // недостаточно USDT для ребаланса — лучше не пытаться
            // (в реальном мире можно делать partial, но это усложнение позже)
            return None;
        }
        let qty = Qty(delta_value / mid.0);
        Some(RebalanceDecision::BuyBase(qty))
    } else {
        // SELL
        let sell_value = (-delta_value) * (1.0 + fee);
        if sell_value < params.min_quote_trade.0 {
            return Some(RebalanceDecision::Noop);
        }
        let qty = Qty((-delta_value) / mid.0);
        if qty.0 > p.base.0 {
            return None;
        }
        Some(RebalanceDecision::SellBase(qty))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> RebalanceParams {
        RebalanceParams {
            target_base_ratio: Ratio(0.5),
            tolerance: Ratio(0.02),
            fee_rate: Ratio(0.001),
            min_quote_trade: Money(5.0),
        }
    }

    #[test]
    fn noop_when_already_balanced() {
        let p = Portfolio {
            base: Qty(1.0),
            quote: Money(1000.0),
        };
        let mid = Price(1000.0); // base_value=1000, equity=2000 => 50%
        let d = rebalance_decision(p, mid, params()).unwrap();
        assert_eq!(d, RebalanceDecision::Noop);
    }

    #[test]
    fn buy_when_underweight_base() {
        let p = Portfolio {
            base: Qty(0.2),
            quote: Money(1000.0),
        };
        let mid = Price(1000.0); // base_value=200, equity=1200, target=600 => need +400
        let d = rebalance_decision(p, mid, params()).unwrap();
        match d {
            RebalanceDecision::BuyBase(q) => assert!(q.0 > 0.0),
            _ => panic!("expected buy"),
        }
    }

    #[test]
    fn sell_when_overweight_base() {
        let p = Portfolio {
            base: Qty(2.0),
            quote: Money(100.0),
        };
        let mid = Price(1000.0); // base_value=2000, equity=2100, target=1050 => need sell ~950
        let d = rebalance_decision(p, mid, params()).unwrap();
        match d {
            RebalanceDecision::SellBase(q) => assert!(q.0 > 0.0),
            _ => panic!("expected sell"),
        }
    }
}
