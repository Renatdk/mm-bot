export type RunStatus = 'queued' | 'running' | 'completed' | 'failed';

export interface RunRecord {
  id: string;
  name: string;
  kind: string;
  status: RunStatus;
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
  exit_code: number | null;
  error: string | null;
}

export interface RunEventRecord {
  id: number;
  run_id: string;
  ts: string;
  level: string;
  message: string;
}

export interface RunMetricsResponse {
  run_id: string;
  updated_at: string | null;
  payload: Record<string, unknown>;
}

export interface EquityPoint {
  ts: number;
  equity: number;
  close?: number | null;
}

export interface TradePoint {
  ts: number;
  side: string;
  price: number;
  qty?: number | null;
  pnl?: number | null;
}

export interface RunArtifact {
  id: number;
  run_id: string;
  kind: string;
  path: string;
  created_at: string;
}

export interface PresetRequest {
  symbol: string;
  start: string;
  end: string;
  maker_fee_bps_list?: string;
  htf_interval?: string;
  ltf_interval?: string;
  top_n?: number;
}
