'use client';

import { useEffect, useMemo, useState } from 'react';
import Link from 'next/link';
import { getRun, getRunArtifacts, getRunEvents, getRunMetrics } from '@/lib/api';
import type { RunArtifact, RunEventRecord, RunMetricsResponse, RunRecord } from '@/lib/types';
import { LineChart } from '@/components/LineChart';

function toNumber(v: unknown): number | null {
  if (typeof v === 'number' && Number.isFinite(v)) return v;
  if (typeof v === 'string') {
    const n = Number(v);
    if (Number.isFinite(n)) return n;
  }
  return null;
}

function extractMetric(metrics: RunMetricsResponse | null, key: string): number | null {
  if (!metrics) return null;
  return toNumber(metrics.payload[key]);
}

export default function RunDetailsPage({ params }: { params: { id: string } }) {
  const runId = params.id;

  const [run, setRun] = useState<RunRecord | null>(null);
  const [events, setEvents] = useState<RunEventRecord[]>([]);
  const [metrics, setMetrics] = useState<RunMetricsResponse | null>(null);
  const [artifacts, setArtifacts] = useState<RunArtifact[]>([]);
  const [error, setError] = useState<string>('');
  const [timeline, setTimeline] = useState<Array<{ x: number; y: number }>>([]);

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
    const t = setInterval(refresh, 3000);
    return () => clearInterval(t);
  }, [runId]);

  const progressChart = useMemo(
    () => events.map((e) => ({ x: new Date(e.ts).getTime(), y: e.id })),
    [events]
  );

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
