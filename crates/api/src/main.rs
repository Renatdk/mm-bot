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
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let pg = PgPool::connect(&database_url).await?;
    sqlx::migrate!("../../migrations").run(&pg).await?;
    let redis = redis::Client::open(redis_url)?;

    let state = AppState { pg, redis };

    let app = Router::new()
        .route("/health", get(health))
        .route("/runs", post(create_run).get(list_runs))
        .route("/runs/:id", get(get_run))
        .route("/runs/:id/events", get(list_run_events))
        .with_state(state);

    let addr: SocketAddr = bind_addr.parse().context("invalid BIND_ADDR")?;
    info!("api listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    Json(json!({"ok": true}))
}

async fn create_run(
    State(state): State<AppState>,
    Json(req): Json<CreateRunRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
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
