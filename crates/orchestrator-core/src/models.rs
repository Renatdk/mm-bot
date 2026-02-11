use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const RUN_QUEUE_KEY: &str = "mmbot:run_queue";

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    BacktestTrend,
    BacktestTrendSweep,
    BacktestMm,
    BacktestMmMtf,
    BacktestMmMtfSweep,
}

impl RunKind {
    pub fn engine_bin(self) -> &'static str {
        match self {
            Self::BacktestTrend => "backtest_trend",
            Self::BacktestTrendSweep => "backtest_trend_sweep",
            Self::BacktestMm => "backtest_mm",
            Self::BacktestMmMtf => "backtest_mm_mtf",
            Self::BacktestMmMtfSweep => "backtest_mm_mtf_sweep",
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunRequest {
    pub name: String,
    pub kind: RunKind,
    pub cli_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: Uuid,
    pub name: String,
    pub kind: RunKind,
    pub status: RunStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEventRecord {
    pub id: i64,
    pub run_id: Uuid,
    pub ts: DateTime<Utc>,
    pub level: String,
    pub message: String,
}
