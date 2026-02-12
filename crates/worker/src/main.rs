use std::{
    env,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use orchestrator_core::models::{RUN_QUEUE_KEY, RunKind};
use sqlx::PgPool;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use tracing::{error, info};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "worker=info".into()),
        )
        .init();

    let database_url = env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let redis_url = env::var("REDIS_URL").context("REDIS_URL is required")?;
    let workspace_root = env::var("WORKSPACE_ROOT").unwrap_or_else(|_| "/app".to_string());
    let engine_bin_dir = env::var("ENGINE_BIN_DIR").unwrap_or_else(|_| "/usr/local/bin".to_string());

    let pg = PgPool::connect(&database_url).await?;
    sqlx::migrate!("../../migrations").run(&pg).await?;

    let redis = redis::Client::open(redis_url)?;
    let mut conn = redis
        .get_multiplexed_tokio_connection()
        .await
        .context("redis connection failed")?;

    info!("worker started");

    loop {
        let resp: (String, String) = redis::cmd("BRPOP")
            .arg(RUN_QUEUE_KEY)
            .arg(0)
            .query_async(&mut conn)
            .await
            .context("queue pop failed")?;

        let run_id: Uuid = match resp.1.parse() {
            Ok(v) => v,
            Err(e) => {
                error!("invalid run id in queue '{}': {}", resp.1, e);
                continue;
            }
        };

        if let Err(e) = process_run(&pg, run_id, &workspace_root, &engine_bin_dir).await {
            error!("run {} failed: {}", run_id, e);
            let _ = mark_failed(&pg, run_id, None, &format!("{}", e)).await;
        }
    }
}

async fn process_run(pg: &PgPool, run_id: Uuid, workspace_root: &str, engine_bin_dir: &str) -> Result<()> {
    let row = sqlx::query_as::<_, DbRunAndParams>(
        r#"
        SELECT r.id, r.kind, p.cli_args
        FROM runs r
        JOIN run_params p ON p.run_id = r.id
        WHERE r.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(pg)
    .await?;

    let Some(row) = row else {
        anyhow::bail!("run {} not found", run_id);
    };

    let run_kind = parse_run_kind(&row.kind)?;
    let cli_args: Vec<String> = serde_json::from_value(row.cli_args)
        .context("failed to decode cli_args for run")?;

    sqlx::query(
        r#"
        UPDATE runs
        SET status = 'running', started_at = NOW(), error = NULL, exit_code = NULL
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .execute(pg)
    .await?;

    append_event(pg, run_id, "info", "started worker execution").await?;

    let engine_bin_path = format!("{}/{}", engine_bin_dir.trim_end_matches('/'), run_kind.engine_bin());
    let mut cmd = Command::new(&engine_bin_path);
    cmd.args(&cli_args)
        .current_dir(workspace_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn backtest process: {}", engine_bin_path))?;
    let stdout = child.stdout.take().context("stdout unavailable")?;
    let stderr = child.stderr.take().context("stderr unavailable")?;

    let mut out_reader = BufReader::new(stdout).lines();
    let mut err_reader = BufReader::new(stderr).lines();
    let mut metrics = serde_json::Map::<String, serde_json::Value>::new();
    let mut artifacts: Vec<ArtifactEntry> = Vec::new();
    let mut last_progress_persist = Instant::now();

    loop {
        tokio::select! {
            out = out_reader.next_line() => {
                match out {
                    Ok(Some(line)) => {
                        collect_results_from_line(&line, &mut metrics, &mut artifacts);
                        append_event(pg, run_id, "info", &line).await?;
                        persist_progress_if_due(
                            pg,
                            run_id,
                            workspace_root,
                            &metrics,
                            &artifacts,
                            &mut last_progress_persist
                        ).await?;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        append_event(pg, run_id, "error", &format!("stdout read error: {}", e)).await?;
                    }
                }
            }
            err = err_reader.next_line() => {
                match err {
                    Ok(Some(line)) => {
                        collect_results_from_line(&line, &mut metrics, &mut artifacts);
                        append_event(pg, run_id, "error", &line).await?;
                        persist_progress_if_due(
                            pg,
                            run_id,
                            workspace_root,
                            &metrics,
                            &artifacts,
                            &mut last_progress_persist
                        ).await?;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        append_event(pg, run_id, "error", &format!("stderr read error: {}", e)).await?;
                    }
                }
            }
            status = child.wait() => {
                let status = status.context("failed to wait for child process")?;
                let code = status.code().unwrap_or(-1);

                // Process may exit before we consume buffered stdout/stderr lines.
                // Drain remaining output so metrics/artifacts are not lost.
                while let Ok(Some(line)) = out_reader.next_line().await {
                    collect_results_from_line(&line, &mut metrics, &mut artifacts);
                    append_event(pg, run_id, "info", &line).await?;
                }
                while let Ok(Some(line)) = err_reader.next_line().await {
                    collect_results_from_line(&line, &mut metrics, &mut artifacts);
                    append_event(pg, run_id, "error", &line).await?;
                }

                if status.success() {
                    persist_results(pg, run_id, workspace_root, &metrics, &artifacts).await?;
                    sqlx::query(
                        r#"
                        UPDATE runs
                        SET status = 'completed', ended_at = NOW(), exit_code = $2
                        WHERE id = $1
                        "#,
                    )
                    .bind(run_id)
                    .bind(code)
                    .execute(pg)
                    .await?;
                    append_event(pg, run_id, "info", "run completed").await?;
                } else {
                    mark_failed(pg, run_id, Some(code), "engine process exited with failure").await?;
                }
                break;
            }
        }
    }

    Ok(())
}

const LIVE_PERSIST_INTERVAL: Duration = Duration::from_secs(2);

async fn persist_progress_if_due(
    pg: &PgPool,
    run_id: Uuid,
    workspace_root: &str,
    metrics: &serde_json::Map<String, serde_json::Value>,
    artifacts: &[ArtifactEntry],
    last_persist: &mut Instant,
) -> Result<()> {
    if last_persist.elapsed() < LIVE_PERSIST_INTERVAL {
        return Ok(());
    }

    persist_results(pg, run_id, workspace_root, metrics, artifacts).await?;
    *last_persist = Instant::now();
    Ok(())
}

#[derive(Debug, Clone)]
struct ArtifactEntry {
    kind: String,
    path: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct EquityPoint {
    ts: i64,
    equity: f64,
    close: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct TradePoint {
    ts: i64,
    side: String,
    price: f64,
    qty: Option<f64>,
    pnl: Option<f64>,
}

fn collect_results_from_line(
    line: &str,
    metrics: &mut serde_json::Map<String, serde_json::Value>,
    artifacts: &mut Vec<ArtifactEntry>,
) {
    if let Some(rest) = line.strip_prefix("artifacts:") {
        for token in rest.split_whitespace() {
            if let Some((k, v)) = token.split_once('=') {
                let kind = k.trim().to_string();
                let path = v.trim().trim_end_matches(',').to_string();
                if !kind.is_empty() && !path.is_empty() {
                    artifacts.push(ArtifactEntry { kind, path });
                }
            }
        }
    }

    for token in line
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter(|s| !s.is_empty())
    {
        if let Some((k, v_raw)) = token.split_once('=') {
            let key = k.trim();
            if key.is_empty() {
                continue;
            }
            let value = v_raw.trim().trim_matches('"').trim_end_matches(',');
            if value.is_empty() {
                continue;
            }
            if let Ok(num) = value.trim_end_matches('%').parse::<f64>() {
                metrics.insert(key.to_string(), serde_json::json!(num));
            } else {
                metrics.insert(key.to_string(), serde_json::json!(value));
            }
        }
    }
}

async fn persist_results(
    pg: &PgPool,
    run_id: Uuid,
    workspace_root: &str,
    metrics: &serde_json::Map<String, serde_json::Value>,
    artifacts: &[ArtifactEntry],
) -> Result<()> {
    let mut payload_map = metrics.clone();
    append_chart_snapshots(workspace_root, artifacts, &mut payload_map);

    if !payload_map.is_empty() {
        let payload = serde_json::Value::Object(payload_map);
        sqlx::query(
            r#"
            INSERT INTO run_metrics (run_id, payload, updated_at)
            VALUES ($1, $2, NOW())
            ON CONFLICT (run_id)
            DO UPDATE SET payload = EXCLUDED.payload, updated_at = NOW()
            "#,
        )
        .bind(run_id)
        .bind(payload)
        .execute(pg)
        .await?;
    }

    if !artifacts.is_empty() {
        sqlx::query("DELETE FROM run_artifacts WHERE run_id = $1")
            .bind(run_id)
            .execute(pg)
            .await?;

        for a in artifacts {
            sqlx::query(
                r#"
                INSERT INTO run_artifacts (run_id, kind, path, created_at)
                VALUES ($1, $2, $3, NOW())
                "#,
            )
            .bind(run_id)
            .bind(&a.kind)
            .bind(&a.path)
            .execute(pg)
            .await?;
        }
    }

    Ok(())
}

fn append_chart_snapshots(
    workspace_root: &str,
    artifacts: &[ArtifactEntry],
    payload: &mut serde_json::Map<String, serde_json::Value>,
) {
    let equity_artifact = artifacts.iter().find(|a| a.kind.contains("equity"));
    if let Some(a) = equity_artifact {
        let path = resolve_artifact_path(workspace_root, &a.path);
        if let Ok(points) = read_equity_points(&path, 800) {
            payload.insert("chart_equity".to_string(), serde_json::json!(points));
        }
    }

    let trade_artifact = artifacts
        .iter()
        .find(|a| a.kind.contains("fills") || a.kind.contains("trades"));
    if let Some(a) = trade_artifact {
        let path = resolve_artifact_path(workspace_root, &a.path);
        if let Ok(points) = read_trade_points(&path, 1200) {
            payload.insert("chart_trades".to_string(), serde_json::json!(points));
        }
    }
}

fn resolve_artifact_path(workspace_root: &str, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        p
    } else {
        PathBuf::from(workspace_root).join(p)
    }
}

fn find_header_idx(headers: &csv::StringRecord, names: &[&str]) -> Option<usize> {
    headers
        .iter()
        .position(|h| names.iter().any(|n| h.eq_ignore_ascii_case(n)))
}

fn parse_f64_cell(rec: &csv::StringRecord, idx: Option<usize>) -> Option<f64> {
    let i = idx?;
    rec.get(i)?.trim().parse::<f64>().ok()
}

fn parse_i64_cell(rec: &csv::StringRecord, idx: Option<usize>) -> Option<i64> {
    let i = idx?;
    rec.get(i)?.trim().parse::<i64>().ok()
}

fn sample_evenly<T: Clone>(points: &[T], max_points: usize) -> Vec<T> {
    if points.len() <= max_points {
        return points.to_vec();
    }
    if max_points < 2 {
        return vec![points[points.len() - 1].clone()];
    }

    let span = points.len() - 1;
    let mut out = Vec::with_capacity(max_points);
    for i in 0..max_points {
        let idx = i * span / (max_points - 1);
        out.push(points[idx].clone());
    }
    out
}

fn read_equity_points(path: &PathBuf, max_points: usize) -> Result<Vec<EquityPoint>> {
    let mut rdr = csv::Reader::from_path(path)?;
    let headers = rdr.headers()?.clone();
    let ts_idx = find_header_idx(&headers, &["ts", "timestamp"]);
    let equity_idx = find_header_idx(&headers, &["equity", "final_equity"]);
    let close_idx = find_header_idx(&headers, &["close", "price"]);

    if ts_idx.is_none() || equity_idx.is_none() {
        anyhow::bail!("equity csv missing required columns");
    }

    let mut points = Vec::new();
    for rec in rdr.records() {
        let rec = rec?;
        let Some(ts) = parse_i64_cell(&rec, ts_idx) else {
            continue;
        };
        let Some(equity) = parse_f64_cell(&rec, equity_idx) else {
            continue;
        };
        let close = parse_f64_cell(&rec, close_idx);
        points.push(EquityPoint { ts, equity, close });
    }
    Ok(sample_evenly(&points, max_points))
}

fn read_trade_points(path: &PathBuf, max_points: usize) -> Result<Vec<TradePoint>> {
    let mut rdr = csv::Reader::from_path(path)?;
    let headers = rdr.headers()?.clone();
    let ts_idx = find_header_idx(&headers, &["ts", "timestamp"]);
    let side_idx = find_header_idx(&headers, &["side"]);
    let price_idx = find_header_idx(&headers, &["price", "fill_price", "mid_price"]);
    let qty_idx = find_header_idx(&headers, &["qty", "quantity"]);
    let pnl_idx = find_header_idx(&headers, &["realized_pnl", "trade_pnl", "pnl"]);

    if ts_idx.is_none() || side_idx.is_none() || price_idx.is_none() {
        anyhow::bail!("trade csv missing required columns");
    }
    let Some(side_idx) = side_idx else {
        anyhow::bail!("trade csv missing side column");
    };

    let mut points = Vec::new();
    for rec in rdr.records() {
        let rec = rec?;
        let Some(ts) = parse_i64_cell(&rec, ts_idx) else {
            continue;
        };
        let Some(price) = parse_f64_cell(&rec, price_idx) else {
            continue;
        };
        let side = rec.get(side_idx).unwrap_or("").trim().to_uppercase();
        if side.is_empty() {
            continue;
        }
        let qty = parse_f64_cell(&rec, qty_idx);
        let pnl = parse_f64_cell(&rec, pnl_idx);
        points.push(TradePoint {
            ts,
            side,
            price,
            qty,
            pnl,
        });
    }
    Ok(sample_evenly(&points, max_points))
}

async fn append_event(pg: &PgPool, run_id: Uuid, level: &str, message: &str) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO run_events (run_id, ts, level, message)
        VALUES ($1, NOW(), $2, $3)
        "#,
    )
    .bind(run_id)
    .bind(level)
    .bind(message)
    .execute(pg)
    .await?;
    Ok(())
}

async fn mark_failed(pg: &PgPool, run_id: Uuid, code: Option<i32>, error: &str) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE runs
        SET status = 'failed', ended_at = NOW(), exit_code = $2, error = $3
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .bind(code.unwrap_or(-1))
    .bind(error)
    .execute(pg)
    .await?;
    append_event(pg, run_id, "error", error).await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct DbRunAndParams {
    #[allow(dead_code)]
    id: Uuid,
    kind: String,
    cli_args: serde_json::Value,
}

fn parse_run_kind(s: &str) -> Result<RunKind> {
    match s {
        "backtest_trend" => Ok(RunKind::BacktestTrend),
        "backtest_trend_sweep" => Ok(RunKind::BacktestTrendSweep),
        "backtest_mm" => Ok(RunKind::BacktestMm),
        "backtest_mm_mtf" => Ok(RunKind::BacktestMmMtf),
        "backtest_mm_mtf_sweep" => Ok(RunKind::BacktestMmMtfSweep),
        _ => anyhow::bail!("unknown run kind: {}", s),
    }
}
