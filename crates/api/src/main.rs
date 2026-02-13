use std::{env, net::SocketAddr};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use orchestrator_core::models::{
    CreateRunRequest, RUN_QUEUE_KEY, RunEventRecord, RunKind, RunRecord, RunStatus,
};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    pg: PgPool,
    redis: redis::Client,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "api=info,axum=info".into()),
        )
        .init();

    let database_url = env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let redis_url = env::var("REDIS_URL").context("REDIS_URL is required")?;
    let bind_addr = resolve_bind_addr()?;

    let pg = PgPool::connect(&database_url).await?;
    sqlx::migrate!("../../migrations").run(&pg).await?;
    let redis = redis::Client::open(redis_url)?;
    let cors = build_cors_from_env();

    let state = AppState { pg, redis };

    let app = Router::new()
        .route("/health", get(health))
        .route("/runs", post(create_run).get(list_runs))
        .route("/runs/presets/mm_mtf_sweep", post(create_run_preset_mm_mtf_sweep))
        .route("/runs/{id}", get(get_run))
        .route("/runs/{id}/events", get(list_run_events))
        .route("/runs/{id}/metrics", get(get_run_metrics))
        .route("/runs/{id}/artifacts", get(get_run_artifacts))
        .layer(cors)
        .with_state(state);

    let addr: SocketAddr = bind_addr.parse().context("invalid BIND_ADDR")?;
    info!("api listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn build_cors_from_env() -> CorsLayer {
    let allow = env::var("CORS_ALLOW_ORIGINS").unwrap_or_else(|_| {
        "http://localhost:3000,http://127.0.0.1:3000".to_string()
    });

    let origins: Vec<axum::http::HeaderValue> = allow
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();

    if origins.is_empty() {
        CorsLayer::new()
            .allow_methods(Any)
            .allow_headers(Any)
            .allow_origin(Any)
    } else {
        CorsLayer::new()
            .allow_methods(Any)
            .allow_headers(Any)
            .allow_origin(origins)
    }
}

fn resolve_bind_addr() -> Result<String> {
    let port_env = env::var("PORT").ok();
    let fallback = format!("0.0.0.0:{}", port_env.unwrap_or_else(|| "8080".to_string()));

    let Some(raw) = env::var("BIND_ADDR").ok() else {
        return Ok(fallback);
    };

    let mut bind = raw.trim().to_string();
    if bind.contains("$PORT") || bind.contains("${PORT}") {
        let port = env::var("PORT").context("BIND_ADDR references PORT but PORT is missing")?;
        bind = bind.replace("${PORT}", &port).replace("$PORT", &port);
    }

    Ok(bind)
}

async fn health() -> impl IntoResponse {
    Json(json!({"ok": true}))
}

async fn create_run(
    State(state): State<AppState>,
    Json(req): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<RunRecord>), (StatusCode, Json<serde_json::Value>)> {
    enqueue_run(&state, req).await
}

#[derive(Debug, Deserialize)]
struct MmMtfSweepPresetRequest {
    symbol: String,
    start: String,
    end: String,
    htf_interval: Option<String>,
    ltf_interval: Option<String>,
    maker_fee_bps_list: Option<String>,
    top_n: Option<usize>,
    summary_out: Option<String>,
}

async fn create_run_preset_mm_mtf_sweep(
    State(state): State<AppState>,
    Json(req): Json<MmMtfSweepPresetRequest>,
) -> Result<(StatusCode, Json<RunRecord>), (StatusCode, Json<serde_json::Value>)> {
    if req.symbol.trim().is_empty() || req.start.trim().is_empty() || req.end.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "symbol, start, end are required"})),
        ));
    }

    let htf_interval = req.htf_interval.unwrap_or_else(|| "5".to_string());
    let ltf_interval = req.ltf_interval.unwrap_or_else(|| "1".to_string());
    let maker_fee_bps_list = req.maker_fee_bps_list.unwrap_or_else(|| "10".to_string());
    let top_n = req.top_n.unwrap_or(30).clamp(1, 200);
    let summary_out = req.summary_out.unwrap_or_else(|| {
        format!(
            "data/mm_mtf_sweep_{}_{}_{}.csv",
            req.symbol,
            req.start.replace('-', ""),
            req.end.replace('-', "")
        )
    });

    let run = CreateRunRequest {
        name: format!("mm_mtf_sweep {} {}..{}", req.symbol, req.start, req.end),
        kind: RunKind::BacktestMmMtfSweep,
        cli_args: vec![
            "--symbol".into(),
            req.symbol,
            "--htf-interval".into(),
            htf_interval,
            "--ltf-interval".into(),
            ltf_interval,
            "--start".into(),
            req.start,
            "--end".into(),
            req.end,
            "--htf-cache".into(),
            "data/mm_mtf_htf_5m.csv".into(),
            "--ltf-cache".into(),
            "data/mm_mtf_ltf_1m.csv".into(),
            "--levels-list".into(),
            "3,5,7".into(),
            "--step-bps-list".into(),
            "6,8,10,12".into(),
            "--base-quote-per-order-list".into(),
            "20,30,40".into(),
            "--max-size-mult-list".into(),
            "1.5,2.0,2.5".into(),
            "--soft-min-list".into(),
            "0.35,0.40".into(),
            "--soft-max-list".into(),
            "0.55,0.60".into(),
            "--hard-min-list".into(),
            "0.30,0.35".into(),
            "--hard-max-list".into(),
            "0.65,0.70".into(),
            "--maker-fee-bps-list".into(),
            maker_fee_bps_list,
            "--defensive-step-mult-list".into(),
            "1.2,1.5,1.8".into(),
            "--defensive-size-mult-list".into(),
            "0.35,0.5,0.7".into(),
            "--initial-quote".into(),
            "1000".into(),
            "--initial-base".into(),
            "0".into(),
            "--bootstrap-rebalance".into(),
            "--bootstrap-target-ratio".into(),
            "0.50".into(),
            "--force-close-at-end".into(),
            "--force-close-fee-bps".into(),
            "10".into(),
            "--force-close-spread-bps".into(),
            "8".into(),
            "--force-close-slippage-bps".into(),
            "2".into(),
            "--top-n".into(),
            top_n.to_string(),
            "--summary-out".into(),
            summary_out,
        ],
    };

    enqueue_run(&state, run).await
}

async fn enqueue_run(
    state: &AppState,
    req: CreateRunRequest,
) -> Result<(StatusCode, Json<RunRecord>), (StatusCode, Json<serde_json::Value>)> {
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name cannot be empty"})),
        ));
    }

    let run_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let run_kind = serde_json::to_string(&req.kind).map_err(internal_err)?;
    let run_kind = run_kind.trim_matches('"').to_string();
    let status = "queued";

    sqlx::query(
        r#"
        INSERT INTO runs (id, name, kind, status, created_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(run_id)
    .bind(&req.name)
    .bind(&run_kind)
    .bind(status)
    .bind(now)
    .execute(&state.pg)
    .await
    .map_err(internal_err)?;

    let args_json = serde_json::to_value(&req.cli_args).map_err(internal_err)?;
    sqlx::query(
        r#"
        INSERT INTO run_params (run_id, cli_args, created_at)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(run_id)
    .bind(args_json)
    .bind(now)
    .execute(&state.pg)
    .await
    .map_err(internal_err)?;

    sqlx::query(
        r#"
        INSERT INTO run_events (run_id, ts, level, message)
        VALUES ($1, $2, 'info', $3)
        "#,
    )
    .bind(run_id)
    .bind(now)
    .bind(format!("queued run {} ({})", req.name, run_kind))
    .execute(&state.pg)
    .await
    .map_err(internal_err)?;

    let mut conn = state
        .redis
        .get_multiplexed_tokio_connection()
        .await
        .map_err(redis_err)?;
    conn.lpush::<_, _, usize>(RUN_QUEUE_KEY, run_id.to_string())
        .await
        .map_err(redis_err)?;

    let out = RunRecord {
        id: run_id,
        name: req.name,
        kind: parse_run_kind(&run_kind).unwrap_or(RunKind::BacktestMmMtf),
        status: RunStatus::Queued,
        created_at: now,
        started_at: None,
        ended_at: None,
        exit_code: None,
        error: None,
    };
    Ok((StatusCode::ACCEPTED, Json(out)))
}

#[derive(Debug, Deserialize)]
struct ListRunsQuery {
    limit: Option<i64>,
}

async fn list_runs(
    State(state): State<AppState>,
    Query(q): Query<ListRunsQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);

    let rows = sqlx::query_as::<_, DbRun>(
        r#"
        SELECT id, name, kind, status, created_at, started_at, ended_at, exit_code, error
        FROM runs
        ORDER BY created_at DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(&state.pg)
    .await
    .map_err(internal_err)?;

    let out: Vec<RunRecord> = rows.into_iter().filter_map(|r| db_to_run_record(r).ok()).collect();
    Ok(Json(out))
}

async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let row = sqlx::query_as::<_, DbRun>(
        r#"
        SELECT id, name, kind, status, created_at, started_at, ended_at, exit_code, error
        FROM runs
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pg)
    .await
    .map_err(internal_err)?;

    let Some(row) = row else {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": "run not found"}))));
    };

    let out = db_to_run_record(row).map_err(internal_err)?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct ListEventsQuery {
    limit: Option<i64>,
}

async fn list_run_events(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<ListEventsQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    let rows = sqlx::query_as::<_, DbRunEvent>(
        r#"
        SELECT id, run_id, ts, level, message
        FROM run_events
        WHERE run_id = $1
        ORDER BY id DESC
        LIMIT $2
        "#,
    )
    .bind(id)
    .bind(limit)
    .fetch_all(&state.pg)
    .await
    .map_err(internal_err)?;

    let out: Vec<RunEventRecord> = rows
        .into_iter()
        .map(|e| RunEventRecord {
            id: e.id,
            run_id: e.run_id,
            ts: e.ts,
            level: e.level,
            message: e.message,
        })
        .collect();
    Ok(Json(out))
}

async fn get_run_metrics(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let row = sqlx::query_as::<_, DbRunMetrics>(
        r#"
        SELECT run_id, payload, updated_at
        FROM run_metrics
        WHERE run_id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pg)
    .await
    .map_err(internal_err)?;

    let out = match row {
        Some(row) => json!({
            "run_id": row.run_id,
            "updated_at": row.updated_at,
            "payload": row.payload
        }),
        None => json!({
            "run_id": id,
            "updated_at": serde_json::Value::Null,
            "payload": serde_json::json!({})
        }),
    };

    Ok(Json(out))
}

async fn get_run_artifacts(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let rows = sqlx::query_as::<_, DbRunArtifact>(
        r#"
        SELECT id, run_id, kind, path, created_at
        FROM run_artifacts
        WHERE run_id = $1
        ORDER BY id ASC
        "#,
    )
    .bind(id)
    .fetch_all(&state.pg)
    .await
    .map_err(internal_err)?;

    Ok(Json(rows))
}

#[derive(sqlx::FromRow)]
struct DbRun {
    id: Uuid,
    name: String,
    kind: String,
    status: String,
    created_at: chrono::DateTime<chrono::Utc>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    exit_code: Option<i32>,
    error: Option<String>,
}

#[derive(sqlx::FromRow)]
struct DbRunEvent {
    id: i64,
    run_id: Uuid,
    ts: chrono::DateTime<chrono::Utc>,
    level: String,
    message: String,
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct DbRunMetrics {
    run_id: Uuid,
    payload: serde_json::Value,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct DbRunArtifact {
    id: i64,
    run_id: Uuid,
    kind: String,
    path: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

fn db_to_run_record(r: DbRun) -> Result<RunRecord> {
    Ok(RunRecord {
        id: r.id,
        name: r.name,
        kind: parse_run_kind(&r.kind)?,
        status: parse_run_status(&r.status)?,
        created_at: r.created_at,
        started_at: r.started_at,
        ended_at: r.ended_at,
        exit_code: r.exit_code,
        error: r.error,
    })
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

fn parse_run_status(s: &str) -> Result<RunStatus> {
    match s {
        "queued" => Ok(RunStatus::Queued),
        "running" => Ok(RunStatus::Running),
        "completed" => Ok(RunStatus::Completed),
        "failed" => Ok(RunStatus::Failed),
        _ => anyhow::bail!("unknown run status: {}", s),
    }
}

fn internal_err<E: std::fmt::Display>(e: E) -> (StatusCode, Json<serde_json::Value>) {
    error!("internal error: {}", e);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal error"})),
    )
}

fn redis_err<E: std::fmt::Display>(e: E) -> (StatusCode, Json<serde_json::Value>) {
    error!("redis error: {}", e);
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error": "queue unavailable"})),
    )
}
