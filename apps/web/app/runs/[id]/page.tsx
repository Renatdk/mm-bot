'use client';

import { useEffect, useMemo, useState } from 'react';
import Link from 'next/link';
import { getRun, getRunArtifacts, getRunEvents, getRunMetrics } from '@/lib/api';
import type {
  EquityPoint,
  RunArtifact,
  RunEventRecord,
  RunMetricsResponse,
  RunRecord,
  TradePoint
} from '@/lib/types';
import { CandlestickChart, type Candle } from '@/components/CandlestickChart';
import { LineChart } from '@/components/LineChart';
import { TradeOverlayChart } from '@/components/TradeOverlayChart';

function toNumber(v: unknown): number | null {
  if (typeof v === 'number' && Number.isFinite(v)) return v;
  if (typeof v === 'string') {
    const n = Number(v);
    if (Number.isFinite(n)) return n;
  }
  return null;
}

function normalizeTs(ts: number): number {
  return ts < 10_000_000_000 ? ts * 1000 : ts;
}

function extractMetric(metrics: RunMetricsResponse | null, key: string): number | null {
  if (!metrics) return null;
  return toNumber(metrics.payload[key]);
}

function parseEquityPoints(metrics: RunMetricsResponse | null): EquityPoint[] {
  const raw = metrics?.payload?.chart_equity;
  if (!Array.isArray(raw)) return [];
  const out: EquityPoint[] = [];
  for (const item of raw) {
    if (typeof item !== 'object' || item === null) continue;
    const ts = toNumber((item as { ts?: unknown }).ts);
    const equity = toNumber((item as { equity?: unknown }).equity);
    const close = toNumber((item as { close?: unknown }).close);
    if (ts === null || equity === null) continue;
    out.push({ ts: normalizeTs(ts), equity, close });
  }
  return out.sort((a, b) => a.ts - b.ts);
}

function parseTradePoints(metrics: RunMetricsResponse | null): TradePoint[] {
  const raw = metrics?.payload?.chart_trades;
  if (!Array.isArray(raw)) return [];
  const out: TradePoint[] = [];
  for (const item of raw) {
    if (typeof item !== 'object' || item === null) continue;
    const ts = toNumber((item as { ts?: unknown }).ts);
    const price = toNumber((item as { price?: unknown }).price);
    const side = (item as { side?: unknown }).side;
    const qty = toNumber((item as { qty?: unknown }).qty);
    const pnl = toNumber((item as { pnl?: unknown }).pnl);
    if (ts === null || price === null || typeof side !== 'string') continue;
    out.push({ ts: normalizeTs(ts), side, price, qty, pnl });
  }
  return out.sort((a, b) => a.ts - b.ts);
}

function buildCandles(points: EquityPoint[]): Candle[] {
  if (!points.length) return [];

  const minTs = points[0].ts;
  const maxTs = points[points.length - 1].ts;
  const span = Math.max(1, maxTs - minTs);

  let bucketMs = 60_000;
  if (span > 2 * 24 * 60 * 60 * 1000) bucketMs = 30 * 60_000;
  else if (span > 12 * 60 * 60 * 1000) bucketMs = 5 * 60_000;

  const byBucket = new Map<number, Candle>();
  for (const p of points) {
    const price = p.close ?? p.equity;
    if (!Number.isFinite(price)) continue;
    const ts = Math.floor(p.ts / bucketMs) * bucketMs;
    const prev = byBucket.get(ts);
    if (!prev) {
      byBucket.set(ts, { ts, open: price, high: price, low: price, close: price });
      continue;
    }
    prev.high = Math.max(prev.high, price);
    prev.low = Math.min(prev.low, price);
    prev.close = price;
  }

  return Array.from(byBucket.values()).sort((a, b) => a.ts - b.ts);
}

export default function RunDetailsPage({ params }: { params: { id: string } }) {
  const runId = params.id;

  const [run, setRun] = useState<RunRecord | null>(null);
  const [events, setEvents] = useState<RunEventRecord[]>([]);
  const [metrics, setMetrics] = useState<RunMetricsResponse | null>(null);
  const [artifacts, setArtifacts] = useState<RunArtifact[]>([]);
  const [error, setError] = useState<string>('');
  const [timeline, setTimeline] = useState<Array<{ x: number; y: number }>>([]);
  const isActive = run?.status === 'queued' || run?.status === 'running';

  async function refresh() {
    try {
      const [runData, eventsData, metricsData, artifactsData] = await Promise.all([
        getRun(runId),
        getRunEvents(runId),
        getRunMetrics(runId),
        getRunArtifacts(runId)
      ]);

      setRun(runData);
      setEvents(eventsData.reverse());
      setMetrics(metricsData);
      setArtifacts(artifactsData);
      setError('');

      const roi = extractMetric(metricsData, 'roi');
      if (roi !== null) {
        setTimeline((prev) => [...prev, { x: Date.now(), y: roi }].slice(-120));
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, isActive ? 1000 : 4000);
    return () => clearInterval(t);
  }, [runId, isActive]);

  const progressChart = useMemo(
    () =>
      [...events]
        .sort((a, b) => new Date(a.ts).getTime() - new Date(b.ts).getTime())
        .map((e, idx) => ({ x: new Date(e.ts).getTime(), y: idx + 1 })),
    [events]
  );
  const equityChart = useMemo(() => parseEquityPoints(metrics), [metrics]);
  const tradePoints = useMemo(() => parseTradePoints(metrics), [metrics]);
  const candles = useMemo(() => buildCandles(equityChart), [equityChart]);

  return (
    <section className="stack">
      <div className="row-between">
        <h1>Run Details</h1>
        <Link href="/">Back</Link>
      </div>

      {error ? <p className="error">{error}</p> : null}

      <div className="card stack">
        <div className="row gap wrap">
          <div>
            <div className="label">Run ID</div>
            <div className="mono tiny">{runId}</div>
          </div>
          <div>
            <div className="label">Status</div>
            <div>{run?.status || '-'}</div>
          </div>
          <div>
            <div className="label">Live Updates</div>
            <div>{isActive ? 'on (1s)' : 'idle (4s)'}</div>
          </div>
          <div>
            <div className="label">Kind</div>
            <div className="mono tiny">{run?.kind || '-'}</div>
          </div>
          <div>
            <div className="label">ROI %</div>
            <div>{extractMetric(metrics, 'roi')?.toFixed(2) ?? '-'}</div>
          </div>
          <div>
            <div className="label">PF</div>
            <div>{extractMetric(metrics, 'profit_factor')?.toFixed(3) ?? '-'}</div>
          </div>
          <div>
            <div className="label">Max DD %</div>
            <div>{extractMetric(metrics, 'max_drawdown')?.toFixed(2) ?? '-'}</div>
          </div>
        </div>
      </div>

      <div className="card stack">
        <h2>Testing Progress (events timeline)</h2>
        <LineChart points={progressChart} yLabel="Events" color="#3b82f6" />
      </div>

      <div className="card stack">
        <h2>Trading Result (live ROI track)</h2>
        <LineChart points={timeline} yLabel="ROI %" color="#17c964" />
      </div>

      <div className="card stack">
        <h2>Price Candles (live)</h2>
        <CandlestickChart candles={candles} trades={tradePoints} />
      </div>

      <div className="card stack">
        <h2>Equity + Trades</h2>
        <TradeOverlayChart equity={equityChart} trades={tradePoints} />
      </div>

      <div className="card stack">
        <h2>Artifacts</h2>
        <ul>
          {artifacts.map((a) => (
            <li key={a.id}>
              <span className="mono">{a.kind}</span> <span className="tiny muted">{a.path}</span>
            </li>
          ))}
          {!artifacts.length ? <li className="muted">No artifacts yet</li> : null}
        </ul>
      </div>

      <div className="card stack">
        <h2>Live Logs</h2>
        <div className="logs">
          {events.map((e) => (
            <div key={e.id} className={`logline ${e.level}`}>
              <span className="tiny muted">{new Date(e.ts).toLocaleTimeString()}</span>
              <span className="mono tiny">[{e.level}]</span>
              <span>{e.message}</span>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
