use core::types::{Price, Qty};

#[derive(Debug, Copy, Clone)]
pub struct ExecutionModel {
    pub fee_bps: f64,
    pub spread_bps: f64,
    pub slippage_bps: f64,
}

impl ExecutionModel {
    fn bps_to_ratio(bps: f64) -> f64 {
        (bps.max(0.0)) / 10_000.0
    }

    pub fn buy_fill_price(self, mid: Price) -> Price {
        let half_spread = Self::bps_to_ratio(self.spread_bps) / 2.0;
        let slippage = Self::bps_to_ratio(self.slippage_bps);
        Price(mid.0 * (1.0 + half_spread + slippage))
    }

    pub fn sell_fill_price(self, mid: Price) -> Price {
        let half_spread = Self::bps_to_ratio(self.spread_bps) / 2.0;
        let slippage = Self::bps_to_ratio(self.slippage_bps);
        Price(mid.0 * (1.0 - half_spread - slippage))
    }

    pub fn buy_qty_for_quote(self, quote_budget: f64, mid: Price) -> Qty {
        if quote_budget <= 0.0 || mid.0 <= 0.0 {
            return Qty(0.0);
        }
        let fee = Self::bps_to_ratio(self.fee_bps);
        let fill = self.buy_fill_price(mid).0;
        if fill <= 0.0 {
            return Qty(0.0);
        }
        Qty(quote_budget / (fill * (1.0 + fee)))
    }

    pub fn buy_cost(self, qty: Qty, mid: Price) -> f64 {
        if qty.0 <= 0.0 || mid.0 <= 0.0 {
            return 0.0;
        }
        let fee = Self::bps_to_ratio(self.fee_bps);
        qty.0 * self.buy_fill_price(mid).0 * (1.0 + fee)
    }

    pub fn sell_proceeds(self, qty: Qty, mid: Price) -> f64 {
        if qty.0 <= 0.0 || mid.0 <= 0.0 {
            return 0.0;
        }
        let fee = Self::bps_to_ratio(self.fee_bps);
        qty.0 * self.sell_fill_price(mid).0 * (1.0 - fee)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buy_fill_is_above_mid_sell_fill_below_mid() {
        let m = ExecutionModel {
            fee_bps: 10.0,
            spread_bps: 8.0,
            slippage_bps: 2.0,
        };
        let mid = Price(100.0);

        assert!(m.buy_fill_price(mid).0 > mid.0);
        assert!(m.sell_fill_price(mid).0 < mid.0);
    }

    #[test]
    fn buy_cost_does_not_exceed_budget_when_sized_by_budget() {
        let m = ExecutionModel {
            fee_bps: 10.0,
            spread_bps: 8.0,
            slippage_bps: 2.0,
        };
        let budget = 1000.0;
        let mid = Price(200.0);
        let qty = m.buy_qty_for_quote(budget, mid);
        let cost = m.buy_cost(qty, mid);

        assert!(cost <= budget + 1e-9);
    }

    #[test]
    fn round_trip_loses_money_with_costs() {
        let m = ExecutionModel {
            fee_bps: 10.0,
            spread_bps: 10.0,
            slippage_bps: 5.0,
        };
        let mid = Price(100.0);
        let quote = 1000.0;
        let qty = m.buy_qty_for_quote(quote, mid);
        let proceeds = m.sell_proceeds(qty, mid);

        assert!(proceeds < quote);
    }
}
