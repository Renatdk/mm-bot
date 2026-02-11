use anyhow::{Context, Result};
use chrono::{NaiveDate, TimeZone, Utc};
use clap::{Parser, ValueEnum};

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
use structure::bos::{BosParams, BosState, BosTracker};
use structure::pullback::{PullbackParams, PullbackTracker};
use structure::structure::{StructureParams, detect_structure};

#[derive(Debug, Copy, Clone, ValueEnum)]
enum EntryGate {
    Trend,
    TrendBos,
    TrendBosPullback,
}

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

    #[arg(long, default_value = "20")]
    ema_fast_list: String,
    #[arg(long, default_value = "100")]
    ema_slow_list: String,
    #[arg(long, default_value = "trend,trend-bos,trend-bos-pullback")]
    entry_gate_list: String,
    #[arg(long, default_value = "0,20,35")]
    min_trend_gap_bps_list: String,
    #[arg(long, default_value = "0,6,12")]
    cooldown_bars_list: String,
    #[arg(long, default_value = "100,2.5,2.0")]
    max_atr_pct_list: String,

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
    #[arg(long, default_value_t = true)]
    force_close_at_end: bool,

    #[arg(long, default_value_t = 10)]
    top_n: usize,
    #[arg(long, default_value = "data/backtest_trend_sweep_summary.csv")]
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
    ema_fast: usize,
    ema_slow: usize,
    entry_gate: String,
    min_trend_gap_bps: f64,
    cooldown_bars: usize,
    max_atr_pct: f64,
    trades: usize,
    closed_trades: usize,
    stop_exits: usize,
    win_rate_pct: f64,
    profit_factor: f64,
    max_drawdown_pct: f64,
    pnl: f64,
    roi_pct: f64,
}

#[derive(Debug, Copy, Clone)]
struct SweepConfig {
    ema_fast: usize,
    ema_slow: usize,
    entry_gate: EntryGate,
    min_trend_gap_bps: f64,
    cooldown_bars: usize,
    max_atr_pct: f64,
}

#[derive(Debug, Copy, Clone)]
struct BacktestReport {
    trades: usize,
    closed_trades: usize,
    stop_exits: usize,
    win_rate_pct: f64,
    profit_factor: f64,
    max_drawdown_pct: f64,
    pnl: f64,
    roi_pct: f64,
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

fn parse_gate_list(s: &str) -> Result<Vec<EntryGate>> {
    let mut out = Vec::new();
    for raw in s.split(',') {
        let v = raw.trim().to_ascii_lowercase();
        let gate = match v.as_str() {
            "trend" => EntryGate::Trend,
            "trend-bos" => EntryGate::TrendBos,
            "trend-bos-pullback" => EntryGate::TrendBosPullback,
            _ => anyhow::bail!("bad entry gate: {}", raw.trim()),
        };
        out.push(gate);
    }
    if out.is_empty() {
        anyhow::bail!("entry_gate_list cannot be empty");
    }
    Ok(out)
}

fn trend_mode_from_state(state: TrendState) -> TrendMode {
    match state {
        TrendState::Flat => TrendMode::Flat,
        TrendState::Long => TrendMode::Long,
    }
}

fn run_backtest(
    candles: &[structure::candle::Candle],
    cfg: SweepConfig,
    atr_stop_mult: f64,
    exec: ExecutionModel,
    initial_quote: f64,
    force_close_at_end: bool,
) -> BacktestReport {
    let mut feed = CandleFeed::new(cfg.ema_slow * 5);
    let mut ema_fast = EmaCalc::new(cfg.ema_fast);
    let mut ema_slow = EmaCalc::new(cfg.ema_slow);

    let mut trend_state = TrendState::Flat;
    let mut quote = Money(initial_quote);
    let mut base = Qty(0.0);
    let mut entry_price: Option<Price> = None;
    let mut entry_cost_quote: Option<f64> = None;

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

    let mut trades = 0usize;
    let mut stop_exits = 0usize;
    let mut closed_trades = 0usize;
    let mut winning_trades = 0usize;
    let mut gross_profit = 0.0_f64;
    let mut gross_loss = 0.0_f64;
    let mut max_equity = quote.0;
    let mut max_drawdown = 0.0_f64;
    let mut bars_since_exit: usize = usize::MAX / 2;

    for c in candles.iter().copied() {
        bars_since_exit = bars_since_exit.saturating_add(1);
        feed.push(c);
        let fast = ema_fast.update(c.close.0);
        let slow = ema_slow.update(c.close.0);

        let Some(atr) = feed.atr() else {
            continue;
        };

        let ms = detect_structure(&feed.candles, structure_params);
        bos.on_candle_close(&c, &ms, atr, bos_params);
        if bos.state == BosState::Confirmed {
            pullback.on_candle_close(&c, &bos, atr, pullback_params);
        } else {
            pullback.reset();
        }

        let mut decision = trend_policy_decision(
            trend_mode_from_state(trend_state),
            TrendPolicyInput {
                close: c.close,
                atr,
                ema_fast: Price(fast),
                ema_slow: Price(slow),
                position_qty: base,
                entry_price,
            },
            TrendPolicyParams { atr_stop_mult },
        );

        if decision.action == TrendAction::EnterLong {
            let bos_gate_ok = match cfg.entry_gate {
                EntryGate::Trend => true,
                EntryGate::TrendBos => bos.state == BosState::Confirmed,
                EntryGate::TrendBosPullback => bos.state == BosState::Confirmed && pullback.triggered,
            };
            let trend_gap_bps = if c.close.0 > 0.0 {
                ((fast - slow) / c.close.0) * 10_000.0
            } else {
                0.0
            };
            let trend_gap_ok = trend_gap_bps >= cfg.min_trend_gap_bps.max(0.0);
            let cooldown_ok = bars_since_exit >= cfg.cooldown_bars;
            let atr_pct = if c.close.0 > 0.0 {
                100.0 * atr.0 / c.close.0
            } else {
                0.0
            };
            let atr_ok = atr_pct <= cfg.max_atr_pct.max(0.0);
            let gate_ok = bos_gate_ok && trend_gap_ok && cooldown_ok && atr_ok;

            if !gate_ok {
                decision = match trend_mode_from_state(trend_state) {
                    TrendMode::Flat => policy::trend_policy::TrendPolicyDecision {
                        next_mode: TrendMode::Flat,
                        action: TrendAction::HoldFlat,
                        reason: TrendDecisionReason::NoSignal,
                    },
                    TrendMode::Long => policy::trend_policy::TrendPolicyDecision {
                        next_mode: TrendMode::Long,
                        action: TrendAction::HoldLong,
                        reason: TrendDecisionReason::NoSignal,
                    },
                };
            }
        }

        match decision.action {
            TrendAction::EnterLong => {
                if quote.0 > 0.0 {
                    let qty = exec.buy_qty_for_quote(quote.0, c.close);
                    if qty.0 > 0.0 {
                        let cost = exec.buy_cost(qty, c.close);
                        quote = Money((quote.0 - cost).max(0.0));
                        base = Qty(base.0 + qty.0);
                        entry_price = Some(c.close);
                        entry_cost_quote = Some(cost);
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
                    if let Some(cost) = entry_cost_quote {
                        let trade_pnl = proceeds - cost;
                        closed_trades += 1;
                        if trade_pnl > 0.0 {
                            winning_trades += 1;
                            gross_profit += trade_pnl;
                        } else if trade_pnl < 0.0 {
                            gross_loss += -trade_pnl;
                        }
                    }

                    quote = Money(quote.0 + proceeds);
                    base = Qty(0.0);
                    entry_price = None;
                    entry_cost_quote = None;
                    bars_since_exit = 0;
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

    if force_close_at_end && base.0 > 0.0 {
        let final_mark = feed.mid().unwrap_or(Price(0.0));
        let proceeds = exec.sell_proceeds(base, final_mark);
        if let Some(cost) = entry_cost_quote {
            let trade_pnl = proceeds - cost;
            closed_trades += 1;
            if trade_pnl > 0.0 {
                winning_trades += 1;
                gross_profit += trade_pnl;
            } else if trade_pnl < 0.0 {
                gross_loss += -trade_pnl;
            }
        }
        quote = Money(quote.0 + proceeds);
        base = Qty(0.0);
        trades += 1;
        let _ = trend_transition(trend_state, TrendCause::ForceFlat);
    }

    let final_mark = feed.mid().unwrap_or(Price(0.0));
    let final_equity = quote.0 + base.0 * final_mark.0;
    let pnl = final_equity - initial_quote;
    let roi_pct = if initial_quote > 0.0 {
        100.0 * pnl / initial_quote
    } else {
        0.0
    };
    let win_rate_pct = if closed_trades > 0 {
        100.0 * (winning_trades as f64) / (closed_trades as f64)
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

    BacktestReport {
        trades,
        closed_trades,
        stop_exits,
        win_rate_pct,
        profit_factor,
        max_drawdown_pct: max_drawdown * 100.0,
        pnl,
        roi_pct,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.initial_quote <= 0.0 {
        anyhow::bail!("initial_quote must be > 0");
    }

    let ema_fast_list: Vec<usize> = parse_num_list(&args.ema_fast_list, "ema_fast_list")?;
    let ema_slow_list: Vec<usize> = parse_num_list(&args.ema_slow_list, "ema_slow_list")?;
    let entry_gate_list = parse_gate_list(&args.entry_gate_list)?;
    let min_trend_gap_bps_list: Vec<f64> =
        parse_num_list(&args.min_trend_gap_bps_list, "min_trend_gap_bps_list")?;
    let cooldown_bars_list: Vec<usize> =
        parse_num_list(&args.cooldown_bars_list, "cooldown_bars_list")?;
    let max_atr_pct_list: Vec<f64> = parse_num_list(&args.max_atr_pct_list, "max_atr_pct_list")?;

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

    if candles.len() < 120 {
        anyhow::bail!("not enough candles: {}", candles.len());
    }

    let exec = ExecutionModel {
        fee_bps: args.fee_bps,
        spread_bps: args.spread_bps,
        slippage_bps: args.slippage_bps,
    };

    let mut results: Vec<(SweepConfig, BacktestReport)> = Vec::new();
    for &ema_fast in &ema_fast_list {
        for &ema_slow in &ema_slow_list {
            if ema_fast >= ema_slow {
                continue;
            }
            for &entry_gate in &entry_gate_list {
                for &min_trend_gap_bps in &min_trend_gap_bps_list {
                    for &cooldown_bars in &cooldown_bars_list {
                        for &max_atr_pct in &max_atr_pct_list {
                            let cfg = SweepConfig {
                                ema_fast,
                                ema_slow,
                                entry_gate,
                                min_trend_gap_bps,
                                cooldown_bars,
                                max_atr_pct,
                            };
                            let report = run_backtest(
                                &candles,
                                cfg,
                                args.atr_stop_mult,
                                exec,
                                args.initial_quote,
                                args.force_close_at_end,
                            );
                            results.push((cfg, report));
                        }
                    }
                }
            }
        }
    }

    results.sort_by(|a, b| {
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

    let take_n = args.top_n.min(results.len());
    let mut rows = Vec::with_capacity(take_n);
    for (idx, (cfg, rep)) in results.iter().take(take_n).enumerate() {
        rows.push(SummaryRow {
            rank: idx + 1,
            ema_fast: cfg.ema_fast,
            ema_slow: cfg.ema_slow,
            entry_gate: format!("{:?}", cfg.entry_gate),
            min_trend_gap_bps: cfg.min_trend_gap_bps,
            cooldown_bars: cfg.cooldown_bars,
            max_atr_pct: cfg.max_atr_pct,
            trades: rep.trades,
            closed_trades: rep.closed_trades,
            stop_exits: rep.stop_exits,
            win_rate_pct: rep.win_rate_pct,
            profit_factor: rep.profit_factor,
            max_drawdown_pct: rep.max_drawdown_pct,
            pnl: rep.pnl,
            roi_pct: rep.roi_pct,
        });
    }

    write_summary(&args.summary_out, &rows).context("write summary failed")?;
    println!(
        "Sweep done: tested={} top_saved={} summary={}",
        results.len(),
        rows.len(),
        args.summary_out
    );
    if let Some(best) = rows.first() {
        println!(
            "Best: rank={} gate={} ema={}/{} gap_bps={:.2} cooldown={} max_atr_pct={:.2} roi={:.2}% pf={:.4} dd={:.2}%",
            best.rank,
            best.entry_gate,
            best.ema_fast,
            best.ema_slow,
            best.min_trend_gap_bps,
            best.cooldown_bars,
            best.max_atr_pct,
            best.roi_pct,
            best.profit_factor,
            best.max_drawdown_pct
        );
    }

    Ok(())
}
