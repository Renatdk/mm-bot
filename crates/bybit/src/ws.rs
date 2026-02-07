use tokio::sync::mpsc::Sender;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use serde::Deserialize;

use core::types::{Price, Qty, TimestampMs};
use structure::candle::Candle;

/// События market data
#[derive(Debug, Clone)]
pub enum MarketEvent {
    Candle5m(Candle),
    Ticker { mid: Price },
}

#[derive(Debug, Deserialize)]
struct WsEnvelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct KlineData {
    start: i64,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
    confirm: bool,
}

#[derive(Debug, Deserialize)]
struct TickerData {
    #[serde(rename = "lastPrice")]
    last_price: String,
}

fn subscribe_messages() -> Vec<Message> {
    vec![
        Message::Text(
            serde_json::json!({
                "op": "subscribe",
                "args": ["kline.5.ETHUSDT"]
            })
            .to_string(),
        ),
        Message::Text(
            serde_json::json!({
                "op": "subscribe",
                "args": ["tickers.ETHUSDT"]
            })
            .to_string(),
        ),
    ]
}


pub async fn run_ws(tx: Sender<MarketEvent>) {
    // Spot public WS endpoint
    let url = "wss://stream.bybit.com/v5/public/spot";

    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("WS connect failed");

    let (mut write, mut read) = ws.split();

    // подписка
    for msg in subscribe_messages() {
        write.send(msg).await.expect("subscribe failed");
    }

    while let Some(msg) = read.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(_) => break,
        };

        let Message::Text(text) = msg else { continue };

        // kline
        if text.contains("kline.5.") {
            if let Ok(env) = serde_json::from_str::<WsEnvelope<Vec<KlineData>>>(&text) {
                for k in env.data {
                    if !k.confirm {
                        continue; // только закрытые свечи
                    }

                    let candle = Candle {
                        ts: TimestampMs(k.start),
                        open: Price(k.open.parse().unwrap_or(0.0)),
                        high: Price(k.high.parse().unwrap_or(0.0)),
                        low: Price(k.low.parse().unwrap_or(0.0)),
                        close: Price(k.close.parse().unwrap_or(0.0)),
                        volume: Qty(k.volume.parse().unwrap_or(0.0)),
                    };

                    let _ = tx.send(MarketEvent::Candle5m(candle)).await;
                }
            }
            continue;
        }

        // ticker
        if text.contains("tickers.") {
            if let Ok(env) = serde_json::from_str::<WsEnvelope<Vec<TickerData>>>(&text) {
                if let Some(t) = env.data.first() {
                    if let Ok(p) = t.last_price.parse::<f64>() {
                        let _ = tx.send(MarketEvent::Ticker { mid: Price(p) }).await;
                    }
                }
            }
        }
    }
}
