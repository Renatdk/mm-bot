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
    #[arg(long, default_value = "data/backtest_mm_mtf_htf.csv")]
    htf_cache: String,
    #[arg(long, default_value = "data/backtest_mm_mtf_ltf.csv")]
    ltf_cache: String,
    #[arg(long, default_value_t = false)]
    refresh: bool,

    #[arg(long, default_value_t = 1000.0)]
    initial_quote: f64,
    #[arg(long, default_value_t = 0.0)]
    initial_base: f64,

    #[arg(long, default_value_t = 5)]
    levels: usize,
    #[arg(long, default_value_t = 12.0)]
    step_bps: f64,
    #[arg(long, default_value_t = 25.0)]
    base_quote_per_order: f64,
    #[arg(long, default_value_t = 2.0)]
    max_size_mult: f64,
    #[arg(long, default_value_t = 0.0001)]
    min_base_qty: f64,

    #[arg(long, default_value_t = 0.40)]
    soft_min: f64,
    #[arg(long, default_value_t = 0.60)]
    soft_max: f64,
    #[arg(long, default_value_t = 0.35)]
    hard_min: f64,
    #[arg(long, default_value_t = 0.65)]
    hard_max: f64,

    #[arg(long, default_value_t = 10.0)]
    maker_fee_bps: f64,
    #[arg(long, default_value_t = 10.0)]
    force_close_fee_bps: f64,
    #[arg(long, default_value_t = 8.0)]
    force_close_spread_bps: f64,
    #[arg(long, default_value_t = 2.0)]
    force_close_slippage_bps: f64,
    #[arg(long, default_value_t = true)]
    force_close_at_end: bool,
    #[arg(long, default_value_t = 1.5)]
    defensive_step_mult: f64,
    #[arg(long, default_value_t = 0.5)]
    defensive_size_mult: f64,
    #[arg(long, default_value_t = true)]
    bootstrap_rebalance: bool,
    #[arg(long, default_value_t = 0.50)]
    bootstrap_target_ratio: f64,

    #[arg(long, default_value = "data/backtest_mm_mtf_equity.csv")]
    equity_out: String,
    #[arg(long, default_value = "data/backtest_mm_mtf_fills.csv")]
    fills_out: String,
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
struct EquityRow {
    ts: i64,
    close: f64,
    mode: String,
    quote: f64,
    base: f64,
    cost_basis_quote: f64,
    equity: f64,
    drawdown_pct: f64,
}

#[derive(serde::Serialize)]
struct FillRow {
    ts: i64,
    side: String,
    mode: String,
    qty: f64,
    price: f64,
    fee_quote: f64,
    quote_delta: f64,
    realized_pnl: Option<f64>,
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

fn write_equity_csv(path: &str, rows: &[EquityRow]) -> Result<()> {
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

fn write_fills_csv(path: &str, rows: &[FillRow]) -> Result<()> {
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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.initial_quote < 0.0 || args.initial_base < 0.0 {
        anyhow::bail!("initial balances must be non-negative");
    }
    if !(0.0 <= args.hard_min
        && args.hard_min <= args.soft_min
        && args.soft_min <= args.soft_max
        && args.soft_max <= args.hard_max
        && args.hard_max <= 1.0)
    {
        anyhow::bail!("invalid bands: expected hard_min <= soft_min <= soft_max <= hard_max");
    }

    let htf_ms = parse_interval_ms(&args.htf_interval)?;

    let start_ms = date_to_ms(&args.start)?;
    let end_ms = date_to_ms(&args.end)? + 24 * 60 * 60 * 1000 - 1;

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
        soft_min: Ratio(args.soft_min),
        soft_max: Ratio(args.soft_max),
        hard_min: Ratio(args.hard_min),
        hard_max: Ratio(args.hard_max),
    };
    let grid_params = GridParams {
        levels: args.levels,
        step: Bps(args.step_bps),
        base_quote_per_order: Money(args.base_quote_per_order),
        max_size_mult: args.max_size_mult,
        soft_min: Ratio(args.soft_min),
        soft_max: Ratio(args.soft_max),
        hard_min: Ratio(args.hard_min),
        hard_max: Ratio(args.hard_max),
        min_base_qty: Qty(args.min_base_qty),
    };
    let force_close_exec = ExecutionModel {
        fee_bps: args.force_close_fee_bps,
        spread_bps: args.force_close_spread_bps,
        slippage_bps: args.force_close_slippage_bps,
    };
    let maker_fee_ratio = args.maker_fee_bps.max(0.0) / 10_000.0;

    let mut quote = args.initial_quote;
    let mut base = args.initial_base;
    let mut cost_basis_quote = if base > 0.0 { base * htf[0].close.0 } else { 0.0 };

    let mut fill_rows = Vec::new();
    let mut equity_rows = Vec::new();

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
    let mut last_ts = htf[0].ts.0;

    for h in htf {
        let window_start = h.ts.0;
        let window_end = window_start + htf_ms;

        while ltf_idx < ltf.len() && ltf[ltf_idx].ts.0 < window_start {
            ltf_idx += 1;
        }

        while ltf_idx < ltf.len() && ltf[ltf_idx].ts.0 < window_end {
            let lc = ltf[ltf_idx];
            last_ts = lc.ts.0;
            let inv = Inventory {
                base: Qty(base),
                quote: Money(quote),
            };
            if matches!(active_mode, MmMode::Normal | MmMode::Defensive) {
                let mode_grid_params = match active_mode {
                    MmMode::Defensive => GridParams {
                        step: Bps(grid_params.step.0 * args.defensive_step_mult.max(1.0)),
                        base_quote_per_order: Money(
                            grid_params.base_quote_per_order.0
                                * args.defensive_size_mult.clamp(0.05, 1.0),
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
                                fill_rows.push(FillRow {
                                    ts: lc.ts.0,
                                    side: "BUY".to_string(),
                                    mode: format!("{:?}", active_mode),
                                    qty: o.qty.0,
                                    price: o.price.0,
                                    fee_quote: fee,
                                    quote_delta: -total_cost,
                                    realized_pnl: None,
                                });
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
                                fill_rows.push(FillRow {
                                    ts: lc.ts.0,
                                    side: "SELL".to_string(),
                                    mode: format!("{:?}", active_mode),
                                    qty,
                                    price: o.price.0,
                                    fee_quote: fee,
                                    quote_delta: proceeds,
                                    realized_pnl: Some(realized),
                                });
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
                equity_rows.push(EquityRow {
                    ts: lc.ts.0,
                    close: lc.close.0,
                    mode: format!("{:?}", active_mode),
                    quote,
                    base,
                    cost_basis_quote,
                    equity,
                    drawdown_pct: dd * 100.0,
                });
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

            if args.bootstrap_rebalance
                && matches!(
                    decision.reason,
                    MmDecisionReason::InventoryOutsideHardBand
                )
                && bos.state == BosState::Confirmed
                && pullback.triggered
            {
                let equity = quote + base * mid.0;
                let target = args.bootstrap_target_ratio.clamp(0.0, 1.0);
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
                            fill_rows.push(FillRow {
                                ts: h.ts.0,
                                side: "BUY".to_string(),
                                mode: "Bootstrap".to_string(),
                                qty: qty.0,
                                price: force_close_exec.buy_fill_price(mid).0,
                                fee_quote: cost - (qty.0 * force_close_exec.buy_fill_price(mid).0),
                                quote_delta: -cost,
                                realized_pnl: None,
                            });
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
                        fill_rows.push(FillRow {
                            ts: h.ts.0,
                            side: "SELL".to_string(),
                            mode: "Bootstrap".to_string(),
                            qty,
                            price: force_close_exec.sell_fill_price(mid).0,
                            fee_quote: (qty * force_close_exec.sell_fill_price(mid).0) - proceeds,
                            quote_delta: proceeds,
                            realized_pnl: Some(realized),
                        });
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

    if args.force_close_at_end && base > 0.0 {
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
        let gross = exit_qty * final_mark.0;
        let fee = gross - proceeds;
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
        fill_rows.push(FillRow {
            ts: last_ts,
            side: "SELL".to_string(),
            mode: "ForceClose".to_string(),
            qty: exit_qty,
            price: final_mark.0,
            fee_quote: fee.max(0.0),
            quote_delta: proceeds,
            realized_pnl: Some(realized),
        });
    }

    let final_mark = ltf.last().map(|c| c.close).unwrap_or(Price(0.0));
    let final_equity = quote + base * final_mark.0;
    let initial_equity = args.initial_quote + args.initial_base * final_mark.0;
    let pnl = final_equity - initial_equity;
    let roi_pct = if initial_equity > 0.0 {
        100.0 * pnl / initial_equity
    } else {
        0.0
    };
    let closed_trades = sell_fills;
    let win_rate_pct = if closed_trades > 0 {
        100.0 * (winning_sells as f64) / (closed_trades as f64)
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

    write_equity_csv(&args.equity_out, &equity_rows).context("write equity csv failed")?;
    write_fills_csv(&args.fills_out, &fill_rows).context("write fills csv failed")?;

    println!("MM MTF backtest finished");
    println!(
        "tf: htf={}m ltf={}m",
        args.htf_interval, args.ltf_interval
    );
    println!(
        "cost_model: maker_fee_bps={:.2} force_close_fee_bps={:.2} force_close_spread_bps={:.2} force_close_slippage_bps={:.2}",
        args.maker_fee_bps, args.force_close_fee_bps, args.force_close_spread_bps, args.force_close_slippage_bps
    );
    println!(
        "defensive_profile: step_mult={:.2} size_mult={:.2}",
        args.defensive_step_mult, args.defensive_size_mult
    );
    println!(
        "fills: buy={} sell={} bootstrap={}",
        buy_fills, sell_fills, bootstrap_trades
    );
    println!(
        "final_quote={:.4} final_base={:.8} final_equity={:.4}",
        quote, base, final_equity
    );
    println!("pnl={:.4} roi={:.2}% max_drawdown={:.2}%", pnl, roi_pct, max_drawdown * 100.0);
    if gross_loss > 0.0 {
        println!(
            "closed_trades={} win_rate={:.2}% avg_win={:.4} avg_loss={:.4} profit_factor={:.4}",
            closed_trades,
            win_rate_pct,
            avg_win,
            avg_loss,
            gross_profit / gross_loss
        );
    } else {
        println!(
            "closed_trades={} win_rate={:.2}% avg_win={:.4} avg_loss={:.4} profit_factor=INF",
            closed_trades, win_rate_pct, avg_win, avg_loss
        );
    }
    println!(
        "artifacts: equity_csv={} fills_csv={}",
        args.equity_out, args.fills_out
    );

    Ok(())
}
