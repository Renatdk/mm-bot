use serde::Deserialize;
use core::types::{Price, Qty, TimestampMs};
use structure::candle::Candle;

#[derive(Clone)]
pub struct BybitRest {
    client: reqwest::Client,
    base: String,
}

impl BybitRest {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base: "https://api.bybit.com".to_string(),
        }
    }

    pub async fn get_klines_spot(
        &self,
        symbol: &str,
        interval: &str,          // "1","3","5","15","60","D"...
        start_ms: i64,
        end_ms: i64,
        limit: u16,              // 1..=1000
    ) -> anyhow::Result<Vec<Candle>> {
        let url = format!("{}/v5/market/kline", self.base);

        let resp: KlineResp = self.client
            .get(url)
            .query(&[
                ("category", "spot"),
                ("symbol", symbol),
                ("interval", interval),
                ("start", &start_ms.to_string()),
                ("end", &end_ms.to_string()),
                ("limit", &limit.to_string()),
            ])
            .send().await?
            .error_for_status()?
            .json().await?;

        let mut out = Vec::new();
        let list = resp.result.list;

        // Bybit возвращает reverse sort by startTime, поэтому разворачиваем
        for row in list.into_iter().rev() {
            let ts: i64 = row[0].parse()?;
            let open: f64 = row[1].parse()?;
            let high: f64 = row[2].parse()?;
            let low: f64 = row[3].parse()?;
            let close: f64 = row[4].parse()?;
            let vol: f64 = row[5].parse()?;

            out.push(Candle {
                ts: TimestampMs(ts),
                open: Price(open),
                high: Price(high),
                low: Price(low),
                close: Price(close),
                volume: Qty(vol),
            });
        }

        Ok(out)
    }
}

#[derive(Debug, Deserialize)]
struct KlineResp {
    #[allow(dead_code)]
    retCode: i64,
    #[allow(dead_code)]
    retMsg: String,
    result: KlineResult,
}

#[derive(Debug, Deserialize)]
struct KlineResult {
    list: Vec<Vec<String>>,
}


pub async fn download_range(
    api: &BybitRest,
    symbol: &str,
    interval: &str,
    start_ms: i64,
    end_ms: i64,
) -> anyhow::Result<Vec<Candle>> {
    let mut all: Vec<Candle> = Vec::new();
    let mut cursor_end = end_ms;

    // 1000 — максимум на страницу
    let limit = 1000u16;

    loop {
        if cursor_end <= start_ms { break; }

        let page = api.get_klines_spot(symbol, interval, start_ms, cursor_end, limit).await?;
        if page.is_empty() { break; }

        // page уже в возрастающем порядке (мы rev сделали)
        let first_ts = page.first().unwrap().ts.0;
        all.extend(page);

        // дальше “идём назад” по времени
        // чтобы не зациклиться на той же первой свече:
        cursor_end = first_ts - 1;

        // лёгкий троттлинг (можно сделать умнее)
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }

    // all будет “кусочками” от конца к началу — отсортируем и удалим дубликаты
    all.sort_by_key(|c| c.ts.0);
    all.dedup_by_key(|c| c.ts.0);

    // обрежем точно по диапазону
    all.retain(|c| c.ts.0 >= start_ms && c.ts.0 <= end_ms);

    Ok(all)
}
