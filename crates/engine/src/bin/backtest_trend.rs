use anyhow::{Context, Result};
use chrono::{NaiveDate, TimeZone, Utc};
use clap::Parser;

use bybit::rest::{BybitRest, download_range};
use core::types::{Money, Price, Qty};
use engine::feed::CandleFeed;
use execution::sim::ExecutionModel;
use policy::trend_policy::{
    TrendAction, TrendDecisionReason, TrendMode, TrendPolicyInput, TrendPolicyParams,
    trend_policy_decision,
};
use state_machine::trend_cause::TrendCause;
use state_machine::trend_state::TrendState;
use state_machine::trend_transition::trend_transition;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    symbol: String,
    #[arg(long, default_value = "60")]
    interval: String,
    #[arg(long)]
    start: String,
    #[arg(long)]
    end: String,
    #[arg(long, default_value = "data/backtest_trend.csv")]
    cache: String,
    #[arg(long, default_value_t = false)]
    refresh: bool,

    #[arg(long, default_value_t = 20)]
    ema_fast: usize,
    #[arg(long, default_value_t = 100)]
    ema_slow: usize,
    #[arg(long, default_value_t = 2.5)]
    atr_stop_mult: f64,
    #[arg(long, default_value_t = 10.0)]
    fee_bps: f64,
    #[arg(long, default_value_t = 8.0)]
    spread_bps: f64,
    #[arg(long, default_value_t = 2.0)]
    slippage_bps: f64,
    #[arg(long, default_value_t = 1000.0)]
    initial_quote: f64,
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

struct EmaCalc {
    alpha: f64,
    value: Option<f64>,
}

impl EmaCalc {
    fn new(period: usize) -> Self {
        let p = period.max(1) as f64;
        Self {
            alpha: 2.0 / (p + 1.0),
            value: None,
        }
    }

    fn update(&mut self, x: f64) -> f64 {
        match self.value {
            Some(v) => {
                let next = self.alpha * x + (1.0 - self.alpha) * v;
                self.value = Some(next);
                next
            }
            None => {
                self.value = Some(x);
                x
            }
        }
    }
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

fn trend_mode_from_state(state: TrendState) -> TrendMode {
    match state {
        TrendState::Flat => TrendMode::Flat,
        TrendState::Long => TrendMode::Long,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.ema_fast >= args.ema_slow {
        anyhow::bail!("ema_fast must be < ema_slow");
    }
    if args.initial_quote <= 0.0 {
        anyhow::bail!("initial_quote must be > 0");
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

    if candles.len() < args.ema_slow + 5 {
        anyhow::bail!("not enough candles: {}", candles.len());
    }

    let mut feed = CandleFeed::new(args.ema_slow * 5);
    let mut ema_fast = EmaCalc::new(args.ema_fast);
    let mut ema_slow = EmaCalc::new(args.ema_slow);

    let mut trend_state = TrendState::Flat;
    let mut quote = Money(args.initial_quote);
    let mut base = Qty(0.0);
    let mut entry_price: Option<Price> = None;

    let exec = ExecutionModel {
        fee_bps: args.fee_bps,
        spread_bps: args.spread_bps,
        slippage_bps: args.slippage_bps,
    };
    let mut trades = 0usize;
    let mut stop_exits = 0usize;

    let mut max_equity = quote.0;
    let mut max_drawdown = 0.0_f64;

    for c in candles {
        feed.push(c);
        let fast = ema_fast.update(c.close.0);
        let slow = ema_slow.update(c.close.0);

        let Some(atr) = feed.atr() else {
            continue;
        };

        let decision = trend_policy_decision(
            trend_mode_from_state(trend_state),
            TrendPolicyInput {
                close: c.close,
                atr,
                ema_fast: Price(fast),
                ema_slow: Price(slow),
                position_qty: base,
                entry_price,
            },
            TrendPolicyParams {
                atr_stop_mult: args.atr_stop_mult,
            },
        );

        match decision.action {
            TrendAction::EnterLong => {
                if quote.0 > 0.0 {
                    let qty = exec.buy_qty_for_quote(quote.0, c.close);
                    if qty.0 > 0.0 {
                        let cost = exec.buy_cost(qty, c.close);
                        quote = Money((quote.0 - cost).max(0.0));
                        base = Qty(base.0 + qty.0);
                        entry_price = Some(c.close);
                        trades += 1;
                    }
                }

                if let Ok(next) = trend_transition(trend_state, TrendCause::EntrySignal) {
                    trend_state = next;
                }
            }
            TrendAction::ExitLong => {
                if base.0 > 0.0 {
                    let proceeds = exec.sell_proceeds(base, c.close);
                    quote = Money(quote.0 + proceeds);
                    base = Qty(0.0);
                    entry_price = None;
                    trades += 1;
                }

                let cause = match decision.reason {
                    TrendDecisionReason::AtrStopHit => {
                        stop_exits += 1;
                        TrendCause::StopLossHit
                    }
                    TrendDecisionReason::InvalidLongOnlyInvariant => TrendCause::ForceFlat,
                    _ => TrendCause::ExitSignal,
                };

                if let Ok(next) = trend_transition(trend_state, cause) {
                    trend_state = next;
                }
            }
            TrendAction::HoldFlat | TrendAction::HoldLong => {}
        }

        let equity = quote.0 + base.0 * c.close.0;
        max_equity = max_equity.max(equity);
        if max_equity > 0.0 {
            let dd = (max_equity - equity) / max_equity;
            max_drawdown = max_drawdown.max(dd);
        }
    }

    let final_mark = feed.mid().unwrap_or(Price(0.0));
    let final_equity = quote.0 + base.0 * final_mark.0;
    let pnl = final_equity - args.initial_quote;
    let roi_pct = if args.initial_quote > 0.0 {
        100.0 * pnl / args.initial_quote
    } else {
        0.0
    };

    println!("Trend backtest finished");
    println!(
        "cost_model: fee_bps={:.2} spread_bps={:.2} slippage_bps={:.2}",
        args.fee_bps, args.spread_bps, args.slippage_bps
    );
    println!("state={:?} trades={} stop_exits={}", trend_state, trades, stop_exits);
    println!(
        "final_quote={:.4} final_base={:.8} final_equity={:.4}",
        quote.0, base.0, final_equity
    );
    println!("pnl={:.4} roi={:.2}% max_drawdown={:.2}%", pnl, roi_pct, max_drawdown * 100.0);

    Ok(())
}
