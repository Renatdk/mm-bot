mod event;
mod tick;
mod sink;
mod feed;

use tokio::sync::mpsc;

use bybit::ws::{run_ws, MarketEvent};

use core::types::{Money, Qty, Ratio, Bps};

use state_machine::state::BotState;

use mm::grid::{Inventory, GridParams};

use policy::mm_policy::MmPolicyParams;

use structure::bos::BosParams;
use structure::pullback::PullbackParams;
use structure::structure::{detect_structure, StructureParams};

use tick::{EngineCtx, TickInput, tick};
use feed::CandleFeed;

#[tokio::main]
async fn main() {
    // --- configs ---
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

    // HTF candle feed
    let mut feed = CandleFeed::new(50);

    // structure params
    let structure_params = StructureParams {
        pivot_k: 1,
        min_atr_frac: 0.1,
    };

    // inventory пока мок (потом из Bybit REST/account WS)
    let inv = Inventory {
        base: Qty(0.0),
        quote: Money(1000.0),
    };

    // --- ws ---
    let (tx, mut rx) = mpsc::channel::<MarketEvent>(2048);

    tokio::spawn(async move {
        run_ws(tx).await;
    });

    // --- event loop ---
    while let Some(ev) = rx.recv().await {
        match ev {
            MarketEvent::Candle5m(candle) => {
                feed.push(candle);

                let (Some(atr), Some(mid)) = (feed.atr(), feed.mid()) else {
                    continue;
                };

                // структура на окне
                let ms = detect_structure(&feed.candles, structure_params);

                println!(
                    "HTF close={} last_high={:?} last_low={:?} bos={:?} pullback={}",
                    mid.0, ms.last_high.map(|p| p.0), ms.last_low.map(|p| p.0),
                    ctx.bos.state, ctx.pullback.triggered
                );


                // обновить BOS
                let last = feed.candles.last().unwrap();
                ctx.bos.on_candle_close(last, &ms, atr, ctx.bos_params);

                // обновить Pullback
                ctx.pullback.on_candle_close(last, &ctx.bos, atr, ctx.pullback_params);

                // тик engine
                let input = TickInput {
                    mid,
                    atr,
                    inv,
                    ltf_broken_down: false,
                    ltf_recovered: false,
                };

                let events = tick(&mut ctx, input);
                sink::consume(events);
            }

            MarketEvent::Ticker { mid: _ } => {
                // пока игнорируем, mid берём из close свечи
            }
        }
    }
}
