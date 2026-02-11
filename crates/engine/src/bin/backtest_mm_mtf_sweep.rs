use anyhow::{Context, Result};
use chrono::{NaiveDate, TimeZone, Utc};
use clap::Parser;

use bybit::rest::{BybitRest, download_range};
use core::types::{Bps, Money, Price, Qty, Ratio};
use engine::feed::CandleFeed;
use execution::sim::ExecutionModel;
use mm::grid::{GridParams, Inventory, Side, build_grid};
use policy::mm_policy::{MmDecisionReason, MmMode, MmPolicyParams, mm_policy_decision};
use structure::bos::{BosParams, BosState, BosTracker};
use structure::pullback::{PullbackParams, PullbackTracker};
use structure::structure::{StructureParams, detect_structure};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    symbol: String,
    #[arg(long, default_value = "5")]
    htf_interval: String,
    #[arg(long, default_value = "1")]
    ltf_interval: String,
    #[arg(long)]
    start: String,
    #[arg(long)]
    end: String,
    #[arg(long, default_value = "data/backtest_mm_mtf_sweep_htf.csv")]
    htf_cache: String,
    #[arg(long, default_value = "data/backtest_mm_mtf_sweep_ltf.csv")]
    ltf_cache: String,
    #[arg(long, default_value_t = false)]
    refresh: bool,

    #[arg(long, default_value_t = 1000.0)]
    initial_quote: f64,
    #[arg(long, default_value_t = 0.0)]
    initial_base: f64,

    #[arg(long, default_value = "3,5,7")]
    levels_list: String,
    #[arg(long, default_value = "8,12,16")]
    step_bps_list: String,
    #[arg(long, default_value = "15,25,40")]
    base_quote_per_order_list: String,
    #[arg(long, default_value = "1.5,2.0,2.5")]
    max_size_mult_list: String,
    #[arg(long, default_value_t = 0.0001)]
    min_base_qty: f64,

    #[arg(long, default_value = "0.35,0.40,0.45")]
    soft_min_list: String,
    #[arg(long, default_value = "0.55,0.60,0.65")]
    soft_max_list: String,
    #[arg(long, default_value = "0.30,0.35,0.40")]
    hard_min_list: String,
    #[arg(long, default_value = "0.60,0.65,0.70")]
    hard_max_list: String,

    #[arg(long, default_value = "5,10")]
    maker_fee_bps_list: String,
    #[arg(long, default_value = "1.5")]
    defensive_step_mult_list: String,
    #[arg(long, default_value = "0.5")]
    defensive_size_mult_list: String,
    #[arg(long, default_value_t = 10.0)]
    force_close_fee_bps: f64,
    #[arg(long, default_value_t = 8.0)]
    force_close_spread_bps: f64,
    #[arg(long, default_value_t = 2.0)]
    force_close_slippage_bps: f64,
    #[arg(long, default_value_t = true)]
    force_close_at_end: bool,
    #[arg(long, default_value_t = true)]
    bootstrap_rebalance: bool,
    #[arg(long, default_value_t = 0.50)]
    bootstrap_target_ratio: f64,

    #[arg(long, default_value_t = 20)]
    top_n: usize,
    #[arg(long, default_value = "data/mm_mtf_sweep_summary.csv")]
    summary_out: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CandleRow {
    ts: i64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
}

#[derive(serde::Serialize)]
struct SummaryRow {
    rank: usize,
    levels: usize,
    step_bps: f64,
    base_quote_per_order: f64,
    max_size_mult: f64,
    soft_min: f64,
    soft_max: f64,
    hard_min: f64,
    hard_max: f64,
    maker_fee_bps: f64,
    defensive_step_mult: f64,
    defensive_size_mult: f64,
    buy_fills: usize,
    sell_fills: usize,
    bootstrap_trades: usize,
    win_rate_pct: f64,
    avg_win: f64,
    avg_loss: f64,
    profit_factor: f64,
    max_drawdown_pct: f64,
    pnl: f64,
    roi_pct: f64,
}

#[derive(Debug, Copy, Clone)]
struct MmMtfConfig {
    levels: usize,
    step_bps: f64,
    base_quote_per_order: f64,
    max_size_mult: f64,
    soft_min: f64,
    soft_max: f64,
    hard_min: f64,
    hard_max: f64,
    maker_fee_bps: f64,
    defensive_step_mult: f64,
    defensive_size_mult: f64,
}

#[derive(Debug, Copy, Clone)]
struct MmMtfReport {
    buy_fills: usize,
    sell_fills: usize,
    bootstrap_trades: usize,
    win_rate_pct: f64,
    avg_win: f64,
    avg_loss: f64,
    profit_factor: f64,
    max_drawdown_pct: f64,
    pnl: f64,
    roi_pct: f64,
}

fn parse_interval_ms(interval: &str) -> Result<i64> {
    let mins: i64 = interval
        .parse()
        .with_context(|| format!("interval must be numeric minutes, got {}", interval))?;
    if mins <= 0 {
        anyhow::bail!("interval must be > 0");
    }
    Ok(mins * 60 * 1000)
}

fn parse_num_list<T>(s: &str, name: &str) -> Result<Vec<T>>
where
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    let mut out = Vec::new();
    for raw in s.split(',') {
        let v = raw.trim();
        if v.is_empty() {
            continue;
        }
        let parsed = v
            .parse::<T>()
            .map_err(|e| anyhow::anyhow!("bad value in {}: '{}' ({})", name, v, e))?;
        out.push(parsed);
    }
    if out.is_empty() {
        anyhow::bail!("{} cannot be empty", name);
    }
    Ok(out)
}

fn date_to_ms(date: &str) -> Result<i64> {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("bad date: {}", date))?;
    let dt = Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap());
    Ok(dt.timestamp_millis())
}

fn read_cache(path: &str) -> Result<Vec<structure::candle::Candle>> {
    let mut rdr = csv::Reader::from_path(path)?;
    let mut out = Vec::new();
    for r in rdr.deserialize::<CandleRow>() {
        let row = r?;
        out.push(structure::candle::Candle {
            ts: core::types::TimestampMs(row.ts),
            open: Price(row.open),
            high: Price(row.high),
            low: Price(row.low),
            close: Price(row.close),
            volume: Qty(row.volume),
        });
    }
    Ok(out)
}

fn write_cache(path: &str, candles: &[structure::candle::Candle]) -> Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut wtr = csv::Writer::from_path(path)?;
    for c in candles {
        wtr.serialize(CandleRow {
            ts: c.ts.0,
            open: c.open.0,
            high: c.high.0,
            low: c.low.0,
            close: c.close.0,
            volume: c.volume.0,
        })?;
    }
    wtr.flush()?;
    Ok(())
}

fn write_summary(path: &str, rows: &[SummaryRow]) -> Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut wtr = csv::Writer::from_path(path)?;
    for r in rows {
        wtr.serialize(r)?;
    }
    wtr.flush()?;
    Ok(())
}

fn run_mm_mtf(
    htf: &[structure::candle::Candle],
    ltf: &[structure::candle::Candle],
    htf_ms: i64,
    cfg: MmMtfConfig,
    min_base_qty: f64,
    initial_quote: f64,
    initial_base: f64,
    force_close_exec: ExecutionModel,
    force_close_at_end: bool,
    bootstrap_rebalance: bool,
    bootstrap_target_ratio: f64,
) -> MmMtfReport {
    let mut feed = CandleFeed::new(240);
    let mut bos = BosTracker::new();
    let mut pullback = PullbackTracker::new();
    let bos_params = BosParams {
        confirm_candles: 2,
        epsilon_frac: 0.1,
    };
    let pullback_params = PullbackParams {
        epsilon_frac: 0.1,
        retrace_frac: 0.4,
    };
    let structure_params = StructureParams {
        pivot_k: 1,
        min_atr_frac: 0.1,
    };

    let mm_policy = MmPolicyParams {
        soft_min: Ratio(cfg.soft_min),
        soft_max: Ratio(cfg.soft_max),
        hard_min: Ratio(cfg.hard_min),
        hard_max: Ratio(cfg.hard_max),
    };
    let grid_params = GridParams {
        levels: cfg.levels,
        step: Bps(cfg.step_bps),
        base_quote_per_order: Money(cfg.base_quote_per_order),
        max_size_mult: cfg.max_size_mult,
        soft_min: Ratio(cfg.soft_min),
        soft_max: Ratio(cfg.soft_max),
        hard_min: Ratio(cfg.hard_min),
        hard_max: Ratio(cfg.hard_max),
        min_base_qty: Qty(min_base_qty),
    };

    let maker_fee_ratio = cfg.maker_fee_bps.max(0.0) / 10_000.0;
    let mut quote = initial_quote;
    let mut base = initial_base;
    let mut cost_basis_quote = if base > 0.0 { base * htf[0].close.0 } else { 0.0 };

    let mut buy_fills = 0usize;
    let mut sell_fills = 0usize;
    let mut bootstrap_trades = 0usize;
    let mut winning_sells = 0usize;
    let mut losing_sells = 0usize;
    let mut gross_profit = 0.0_f64;
    let mut gross_loss = 0.0_f64;
    let mut max_equity = quote + base * htf[0].close.0;
    let mut max_drawdown = 0.0_f64;

    let mut active_mode = MmMode::Disabled;
    let mut ltf_idx = 0usize;

    for h in htf.iter().copied() {
        let window_start = h.ts.0;
        let window_end = window_start + htf_ms;

        while ltf_idx < ltf.len() && ltf[ltf_idx].ts.0 < window_start {
            ltf_idx += 1;
        }
        while ltf_idx < ltf.len() && ltf[ltf_idx].ts.0 < window_end {
            let lc = ltf[ltf_idx];
            let inv = Inventory {
                base: Qty(base),
                quote: Money(quote),
            };
            if matches!(active_mode, MmMode::Normal | MmMode::Defensive) {
                let mode_grid_params = match active_mode {
                    MmMode::Defensive => GridParams {
                        step: Bps(grid_params.step.0 * cfg.defensive_step_mult.max(1.0)),
                        base_quote_per_order: Money(
                            grid_params.base_quote_per_order.0
                                * cfg.defensive_size_mult.clamp(0.05, 1.0),
                        ),
                        ..grid_params
                    },
                    _ => grid_params,
                };
                if let Some(mut orders) = build_grid(lc.close, lc.close, inv, mode_grid_params) {
                    orders.sort_by(|a, b| match (a.side, b.side) {
                        (Side::Buy, Side::Buy) => b
                            .price
                            .0
                            .partial_cmp(&a.price.0)
                            .unwrap_or(std::cmp::Ordering::Equal),
                        (Side::Sell, Side::Sell) => a
                            .price
                            .0
                            .partial_cmp(&b.price.0)
                            .unwrap_or(std::cmp::Ordering::Equal),
                        (Side::Buy, Side::Sell) => std::cmp::Ordering::Less,
                        (Side::Sell, Side::Buy) => std::cmp::Ordering::Greater,
                    });
                    for o in orders {
                        match o.side {
                            Side::Buy => {
                                if lc.low.0 > o.price.0 {
                                    continue;
                                }
                                let gross = o.qty.0 * o.price.0;
                                let fee = gross * maker_fee_ratio;
                                let total_cost = gross + fee;
                                if total_cost > quote || o.qty.0 <= 0.0 {
                                    continue;
                                }
                                quote -= total_cost;
                                base += o.qty.0;
                                cost_basis_quote += total_cost;
                                buy_fills += 1;
                            }
                            Side::Sell => {
                                if lc.high.0 < o.price.0 || base <= 0.0 {
                                    continue;
                                }
                                let qty = o.qty.0.min(base);
                                if qty <= 0.0 {
                                    continue;
                                }
                                let base_before = base;
                                let avg_cost = if base_before > 0.0 {
                                    cost_basis_quote / base_before
                                } else {
                                    0.0
                                };
                                let gross = qty * o.price.0;
                                let fee = gross * maker_fee_ratio;
                                let proceeds = gross - fee;
                                let removed_cost = avg_cost * qty;
                                let realized = proceeds - removed_cost;
                                quote += proceeds;
                                base -= qty;
                                cost_basis_quote = (cost_basis_quote - removed_cost).max(0.0);
                                if base <= 1e-12 {
                                    base = 0.0;
                                    cost_basis_quote = 0.0;
                                }
                                sell_fills += 1;
                                if realized > 0.0 {
                                    winning_sells += 1;
                                    gross_profit += realized;
                                } else if realized < 0.0 {
                                    losing_sells += 1;
                                    gross_loss += -realized;
                                }
                            }
                        }
                    }
                }
            }

            let equity = quote + base * lc.close.0;
            max_equity = max_equity.max(equity);
            if max_equity > 0.0 {
                let dd = (max_equity - equity) / max_equity;
                max_drawdown = max_drawdown.max(dd);
            }
            ltf_idx += 1;
        }

        feed.push(h);
        let (Some(atr), Some(mid)) = (feed.atr(), feed.mid()) else {
            active_mode = MmMode::Disabled;
            continue;
        };
        let ms = detect_structure(&feed.candles, structure_params);
        bos.on_candle_close(&h, &ms, atr, bos_params);
        if bos.state == BosState::Confirmed {
            pullback.on_candle_close(&h, &bos, atr, pullback_params);
        } else {
            pullback.reset();
        }

        let inv = Inventory {
            base: Qty(base),
            quote: Money(quote),
        };
        if let Some(ratio) = mm::grid::base_ratio(inv, mid) {
            let mut decision = mm_policy_decision(bos.state, &pullback, ratio, mm_policy);
            if bootstrap_rebalance
                && matches!(
                    decision.reason,
                    MmDecisionReason::InventoryOutsideHardBand
                )
                && bos.state == BosState::Confirmed
                && pullback.triggered
            {
                let equity = quote + base * mid.0;
                let target = bootstrap_target_ratio.clamp(0.0, 1.0);
                let target_base_value = target * equity;
                let current_base_value = base * mid.0;
                let delta_value = target_base_value - current_base_value;
                if delta_value > 0.0 && quote > 0.0 {
                    let qty = force_close_exec.buy_qty_for_quote(delta_value.min(quote), mid);
                    if qty.0 > 0.0 {
                        let cost = force_close_exec.buy_cost(qty, mid);
                        if cost <= quote {
                            quote -= cost;
                            base += qty.0;
                            cost_basis_quote += cost;
                            buy_fills += 1;
                            bootstrap_trades += 1;
                        }
                    }
                } else if delta_value < 0.0 && base > 0.0 {
                    let qty = ((-delta_value) / mid.0).min(base);
                    if qty > 0.0 {
                        let proceeds = force_close_exec.sell_proceeds(Qty(qty), mid);
                        let base_before = base;
                        let avg_cost = if base_before > 0.0 {
                            cost_basis_quote / base_before
                        } else {
                            0.0
                        };
                        let removed_cost = avg_cost * qty;
                        let realized = proceeds - removed_cost;
                        quote += proceeds;
                        base -= qty;
                        cost_basis_quote = (cost_basis_quote - removed_cost).max(0.0);
                        if base <= 1e-12 {
                            base = 0.0;
                            cost_basis_quote = 0.0;
                        }
                        sell_fills += 1;
                        bootstrap_trades += 1;
                        if realized > 0.0 {
                            winning_sells += 1;
                            gross_profit += realized;
                        } else if realized < 0.0 {
                            losing_sells += 1;
                            gross_loss += -realized;
                        }
                    }
                }
                let inv2 = Inventory {
                    base: Qty(base),
                    quote: Money(quote),
                };
                if let Some(r2) = mm::grid::base_ratio(inv2, mid) {
                    decision = mm_policy_decision(bos.state, &pullback, r2, mm_policy);
                }
            }
            active_mode = decision.mode;
        } else {
            active_mode = MmMode::Disabled;
        }
    }

    if force_close_at_end && base > 0.0 {
        let final_mark = ltf.last().map(|c| c.close).unwrap_or(Price(0.0));
        let exit_qty = base;
        let proceeds = force_close_exec.sell_proceeds(Qty(exit_qty), final_mark);
        let avg_cost = if exit_qty > 0.0 {
            cost_basis_quote / exit_qty
        } else {
            0.0
        };
        let removed_cost = avg_cost * exit_qty;
        let realized = proceeds - removed_cost;
        quote += proceeds;
        base = 0.0;
        sell_fills += 1;
        if realized > 0.0 {
            winning_sells += 1;
            gross_profit += realized;
        } else if realized < 0.0 {
            losing_sells += 1;
            gross_loss += -realized;
        }
    }

    let final_mark = ltf.last().map(|c| c.close).unwrap_or(Price(0.0));
    let final_equity = quote + base * final_mark.0;
    let initial_equity = initial_quote + initial_base * final_mark.0;
    let pnl = final_equity - initial_equity;
    let roi_pct = if initial_equity > 0.0 {
        100.0 * pnl / initial_equity
    } else {
        0.0
    };
    let win_rate_pct = if sell_fills > 0 {
        100.0 * (winning_sells as f64) / (sell_fills as f64)
    } else {
        0.0
    };
    let avg_win = if winning_sells > 0 {
        gross_profit / (winning_sells as f64)
    } else {
        0.0
    };
    let avg_loss = if losing_sells > 0 {
        gross_loss / (losing_sells as f64)
    } else {
        0.0
    };
    let profit_factor = if gross_loss > 0.0 {
        gross_profit / gross_loss
    } else if gross_profit > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };

    MmMtfReport {
        buy_fills,
        sell_fills,
        bootstrap_trades,
        win_rate_pct,
        avg_win,
        avg_loss,
        profit_factor,
        max_drawdown_pct: max_drawdown * 100.0,
        pnl,
        roi_pct,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.initial_quote < 0.0 || args.initial_base < 0.0 {
        anyhow::bail!("initial balances must be non-negative");
    }

    let htf_ms = parse_interval_ms(&args.htf_interval)?;
    let start_ms = date_to_ms(&args.start)?;
    let end_ms = date_to_ms(&args.end)? + 24 * 60 * 60 * 1000 - 1;

    let levels_list: Vec<usize> = parse_num_list(&args.levels_list, "levels_list")?;
    let step_bps_list: Vec<f64> = parse_num_list(&args.step_bps_list, "step_bps_list")?;
    let base_quote_per_order_list: Vec<f64> =
        parse_num_list(&args.base_quote_per_order_list, "base_quote_per_order_list")?;
    let max_size_mult_list: Vec<f64> =
        parse_num_list(&args.max_size_mult_list, "max_size_mult_list")?;
    let soft_min_list: Vec<f64> = parse_num_list(&args.soft_min_list, "soft_min_list")?;
    let soft_max_list: Vec<f64> = parse_num_list(&args.soft_max_list, "soft_max_list")?;
    let hard_min_list: Vec<f64> = parse_num_list(&args.hard_min_list, "hard_min_list")?;
    let hard_max_list: Vec<f64> = parse_num_list(&args.hard_max_list, "hard_max_list")?;
    let maker_fee_bps_list: Vec<f64> =
        parse_num_list(&args.maker_fee_bps_list, "maker_fee_bps_list")?;
    let defensive_step_mult_list: Vec<f64> =
        parse_num_list(&args.defensive_step_mult_list, "defensive_step_mult_list")?;
    let defensive_size_mult_list: Vec<f64> =
        parse_num_list(&args.defensive_size_mult_list, "defensive_size_mult_list")?;

    let api = BybitRest::new();
    let htf = if !args.refresh && std::path::Path::new(&args.htf_cache).exists() {
        read_cache(&args.htf_cache).context("read htf cache failed")?
    } else {
        let data = download_range(&api, &args.symbol, &args.htf_interval, start_ms, end_ms)
            .await
            .context("download htf failed")?;
        write_cache(&args.htf_cache, &data).context("write htf cache failed")?;
        data
    };
    let ltf = if !args.refresh && std::path::Path::new(&args.ltf_cache).exists() {
        read_cache(&args.ltf_cache).context("read ltf cache failed")?
    } else {
        let data = download_range(&api, &args.symbol, &args.ltf_interval, start_ms, end_ms)
            .await
            .context("download ltf failed")?;
        write_cache(&args.ltf_cache, &data).context("write ltf cache failed")?;
        data
    };
    if htf.len() < 20 || ltf.len() < 20 {
        anyhow::bail!("not enough candles: htf={} ltf={}", htf.len(), ltf.len());
    }

    let force_close_exec = ExecutionModel {
        fee_bps: args.force_close_fee_bps,
        spread_bps: args.force_close_spread_bps,
        slippage_bps: args.force_close_slippage_bps,
    };

    let mut all: Vec<(MmMtfConfig, MmMtfReport)> = Vec::new();
    for &levels in &levels_list {
        for &step_bps in &step_bps_list {
            for &base_quote_per_order in &base_quote_per_order_list {
                for &max_size_mult in &max_size_mult_list {
                    for &soft_min in &soft_min_list {
                        for &soft_max in &soft_max_list {
                            if soft_min >= soft_max {
                                continue;
                            }
                            for &hard_min in &hard_min_list {
                                for &hard_max in &hard_max_list {
                                    if !(hard_min <= soft_min
                                        && soft_max <= hard_max
                                        && hard_min >= 0.0
                                        && hard_max <= 1.0)
                                    {
                                        continue;
                                    }
                                    for &maker_fee_bps in &maker_fee_bps_list {
                                        for &defensive_step_mult in &defensive_step_mult_list {
                                            for &defensive_size_mult in &defensive_size_mult_list {
                                                let cfg = MmMtfConfig {
                                                    levels,
                                                    step_bps,
                                                    base_quote_per_order,
                                                    max_size_mult,
                                                    soft_min,
                                                    soft_max,
                                                    hard_min,
                                                    hard_max,
                                                    maker_fee_bps,
                                                    defensive_step_mult,
                                                    defensive_size_mult,
                                                };
                                                let rep = run_mm_mtf(
                                                    &htf,
                                                    &ltf,
                                                    htf_ms,
                                                    cfg,
                                                    args.min_base_qty,
                                                    args.initial_quote,
                                                    args.initial_base,
                                                    force_close_exec,
                                                    args.force_close_at_end,
                                                    args.bootstrap_rebalance,
                                                    args.bootstrap_target_ratio,
                                                );
                                                all.push((cfg, rep));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    all.sort_by(|a, b| {
        b.1.roi_pct
            .partial_cmp(&a.1.roi_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.1.max_drawdown_pct
                    .partial_cmp(&b.1.max_drawdown_pct)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(
                b.1.profit_factor
                    .partial_cmp(&a.1.profit_factor)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let take_n = args.top_n.min(all.len());
    let mut rows = Vec::with_capacity(take_n);
    for (idx, (cfg, rep)) in all.iter().take(take_n).enumerate() {
        rows.push(SummaryRow {
            rank: idx + 1,
            levels: cfg.levels,
            step_bps: cfg.step_bps,
            base_quote_per_order: cfg.base_quote_per_order,
            max_size_mult: cfg.max_size_mult,
            soft_min: cfg.soft_min,
            soft_max: cfg.soft_max,
            hard_min: cfg.hard_min,
            hard_max: cfg.hard_max,
            maker_fee_bps: cfg.maker_fee_bps,
            defensive_step_mult: cfg.defensive_step_mult,
            defensive_size_mult: cfg.defensive_size_mult,
            buy_fills: rep.buy_fills,
            sell_fills: rep.sell_fills,
            bootstrap_trades: rep.bootstrap_trades,
            win_rate_pct: rep.win_rate_pct,
            avg_win: rep.avg_win,
            avg_loss: rep.avg_loss,
            profit_factor: rep.profit_factor,
            max_drawdown_pct: rep.max_drawdown_pct,
            pnl: rep.pnl,
            roi_pct: rep.roi_pct,
        });
    }
    write_summary(&args.summary_out, &rows).context("write summary failed")?;

    println!(
        "MM MTF sweep done: tested={} top_saved={} summary={}",
        all.len(),
        rows.len(),
        args.summary_out
    );
    if let Some(best) = rows.first() {
        println!(
            "Best: levels={} step_bps={:.2} qpo={:.2} bands=({:.2}-{:.2}|{:.2}-{:.2}) fee={:.2} roi={:.2}% pf={:.4} dd={:.2}%",
            best.levels,
            best.step_bps,
            best.base_quote_per_order,
            best.hard_min,
            best.soft_min,
            best.soft_max,
            best.hard_max,
            best.maker_fee_bps,
            best.roi_pct,
            best.profit_factor,
            best.max_drawdown_pct
        );
    }

    Ok(())
}
