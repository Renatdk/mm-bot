use anyhow::{Context, Result};
use chrono::{NaiveDate, TimeZone, Utc};
use clap::Parser;

use bybit::rest::{BybitRest, download_range};
use core::types::{Bps, Money, Price, Qty, Ratio};
use engine::feed::CandleFeed;
use engine::sink;
use engine::tick::{EngineCtx, TickInput, tick};
use mm::grid::{GridParams, Inventory};
use policy::mm_policy::MmPolicyParams;
use state_machine::state::BotState;
use structure::bos::BosParams;
use structure::pullback::PullbackParams;
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
    #[arg(long, default_value = "data/backtest.csv")]
    cache: String,
    #[arg(long, default_value_t = false)]
    refresh: bool,
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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

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

    if candles.len() < 10 {
        anyhow::bail!("not enough candles: {}", candles.len());
    }

    println!("Loaded candles: {}", candles.len());

    let mm_policy = MmPolicyParams {
        soft_min: Ratio(0.40),
        soft_max: Ratio(0.60),
        hard_min: Ratio(0.35),
        hard_max: Ratio(0.65),
    };

    let grid = GridParams {
        levels: 5,
        step: Bps(12.0),
        base_quote_per_order: Money(25.0),
        max_size_mult: 2.0,
        soft_min: Ratio(0.40),
        soft_max: Ratio(0.60),
        hard_min: Ratio(0.35),
        hard_max: Ratio(0.65),
        min_base_qty: Qty(0.0001),
    };

    let bos_params = BosParams {
        confirm_candles: 2,
        epsilon_frac: 0.1,
    };

    let pullback_params = PullbackParams {
        epsilon_frac: 0.1,
        retrace_frac: 0.4,
    };

    let mut ctx = EngineCtx::new(
        BotState::IdleUSDT,
        mm_policy,
        grid,
        bos_params,
        pullback_params,
    );

    let mut feed = CandleFeed::new(200);

    let structure_params = StructureParams {
        pivot_k: 1,
        min_atr_frac: 0.1,
    };

    let inv = Inventory {
        base: Qty(0.0),
        quote: Money(1000.0),
    };

    let mut n_ticks = 0usize;

    for c in candles {
        feed.push(c);

        let (Some(atr), Some(mid)) = (feed.atr(), feed.mid()) else {
            continue;
        };

        let ms = detect_structure(&feed.candles, structure_params);

        let last = feed.candles.last().unwrap();
        ctx.bos.on_candle_close(last, &ms, atr, ctx.bos_params);
        ctx.pullback
            .on_candle_close(last, &ctx.bos, atr, ctx.pullback_params);

        let input = TickInput {
            mid,
            atr,
            inv,
            ltf_broken_down: false,
            ltf_recovered: false,
        };

        let events = tick(&mut ctx, input);
        sink::consume(events);

        n_ticks += 1;
    }

    println!("Backtest ticks processed: {}", n_ticks);
    Ok(())
}
