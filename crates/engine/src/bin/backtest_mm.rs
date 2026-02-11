use anyhow::{Context, Result};
use chrono::{NaiveDate, TimeZone, Utc};
use clap::Parser;

use bybit::rest::{BybitRest, download_range};
use core::types::{Bps, Money, Price, Qty, Ratio};
use engine::feed::CandleFeed;
use execution::sim::ExecutionModel;
use mm::grid::{GridParams, Inventory, Side, build_grid};
use policy::mm_policy::{MmMode, MmPolicyParams, mm_policy_decision};
use structure::bos::{BosParams, BosState, BosTracker};
use structure::pullback::{PullbackParams, PullbackTracker};
use structure::structure::{StructureParams, detect_structure};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    symbol: String,
    #[arg(long, default_value = "5")]
    interval: String,
    #[arg(long)]
    start: String,
    #[arg(long)]
    end: String,
    #[arg(long, default_value = "data/backtest_mm.csv")]
    cache: String,
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

    #[arg(long, default_value = "data/backtest_mm_equity.csv")]
    equity_out: String,
    #[arg(long, default_value = "data/backtest_mm_fills.csv")]
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

    let start_ms = date_to_ms(&args.start)?;
    let end_ms = date_to_ms(&args.end)? + 24 * 60 * 60 * 1000 - 1;

    let candles = if !args.refresh && std::path::Path::new(&args.cache).exists() {
        read_cache(&args.cache).context("read cache failed")?
    } else {
        let api = BybitRest::new();
        let data = download_range(&api, &args.symbol, &args.interval, start_ms, end_ms)
            .await
            .context("download range failed")?;
        write_cache(&args.cache, &data).context("write cache failed")?;
        data
    };

    if candles.len() < 20 {
        anyhow::bail!("not enough candles: {}", candles.len());
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
    let mut cost_basis_quote = if base > 0.0 {
        base * candles[0].close.0
    } else {
        0.0
    };

    let mut fill_rows: Vec<FillRow> = Vec::new();
    let mut equity_rows: Vec<EquityRow> = Vec::new();

    let mut buy_fills = 0usize;
    let mut sell_fills = 0usize;
    let mut winning_sells = 0usize;
    let mut losing_sells = 0usize;
    let mut gross_profit = 0.0_f64;
    let mut gross_loss = 0.0_f64;
    let mut stop_like_disables = 0usize;
    let mut max_equity = quote + base * candles[0].close.0;
    let mut max_drawdown = 0.0_f64;
    let mut last_ts = candles[0].ts.0;

    for c in candles {
        last_ts = c.ts.0;
        feed.push(c);
        let (Some(atr), Some(mid)) = (feed.atr(), feed.mid()) else {
            continue;
        };

        let ms = detect_structure(&feed.candles, structure_params);
        bos.on_candle_close(&c, &ms, atr, bos_params);
        if bos.state == BosState::Confirmed {
            pullback.on_candle_close(&c, &bos, atr, pullback_params);
        } else {
            pullback.reset();
        }

        let inv = Inventory {
            base: Qty(base),
            quote: Money(quote),
        };
        let Some(ratio) = mm::grid::base_ratio(inv, mid) else {
            continue;
        };
        let policy = mm_policy_decision(bos.state, &pullback, ratio, mm_policy);
        if policy.mode == MmMode::Disabled {
            stop_like_disables += 1;
        }

        if matches!(policy.mode, MmMode::Normal | MmMode::Defensive) {
            if let Some(mut orders) = build_grid(mid, mid, inv, grid_params) {
                // Approx intrabar fill sequence: higher-priority limits first.
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
                            if c.low.0 > o.price.0 {
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
                                ts: c.ts.0,
                                side: "BUY".to_string(),
                                mode: format!("{:?}", policy.mode),
                                qty: o.qty.0,
                                price: o.price.0,
                                fee_quote: fee,
                                quote_delta: -total_cost,
                                realized_pnl: None,
                            });
                        }
                        Side::Sell => {
                            if c.high.0 < o.price.0 || base <= 0.0 {
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
                                ts: c.ts.0,
                                side: "SELL".to_string(),
                                mode: format!("{:?}", policy.mode),
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

        let equity = quote + base * c.close.0;
        max_equity = max_equity.max(equity);
        if max_equity > 0.0 {
            let dd = (max_equity - equity) / max_equity;
            max_drawdown = max_drawdown.max(dd);
            equity_rows.push(EquityRow {
                ts: c.ts.0,
                close: c.close.0,
                mode: format!("{:?}", policy.mode),
                quote,
                base,
                cost_basis_quote,
                equity,
                drawdown_pct: dd * 100.0,
            });
        }
    }

    if args.force_close_at_end && base > 0.0 {
        let final_mark = feed.mid().unwrap_or(Price(0.0));
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

    let final_mark = feed.mid().unwrap_or(Price(0.0));
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

    println!("MM backtest finished");
    println!(
        "cost_model: maker_fee_bps={:.2} force_close_fee_bps={:.2} force_close_spread_bps={:.2} force_close_slippage_bps={:.2}",
        args.maker_fee_bps, args.force_close_fee_bps, args.force_close_spread_bps, args.force_close_slippage_bps
    );
    println!(
        "state: buy_fills={} sell_fills={} stop_like_disables={}",
        buy_fills, sell_fills, stop_like_disables
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
