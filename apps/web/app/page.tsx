'use client';

import { useEffect, useMemo, useState } from 'react';
import Link from 'next/link';
import { useRouter } from 'next/navigation';
import { createMmMtfSweepPreset, listRuns } from '@/lib/api';
import type { RunRecord } from '@/lib/types';

function statusClass(status: string): string {
  return `status ${status}`;
}

export default function RunsPage() {
  const router = useRouter();
  const [runs, setRuns] = useState<RunRecord[]>([]);
  const [error, setError] = useState<string>('');
  const [loading, setLoading] = useState<boolean>(true);
  const [submitting, setSubmitting] = useState<boolean>(false);

  const [symbol, setSymbol] = useState('ETHUSDT');
  const [start, setStart] = useState('2026-01-01');
  const [end, setEnd] = useState('2026-02-10');
  const [statusFilter, setStatusFilter] = useState('all');
  const [kindFilter, setKindFilter] = useState('all');
  const [query, setQuery] = useState('');

  async function refresh() {
    try {
      const data = await listRuns();
      setRuns(data);
      setError('');
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 5000);
    return () => clearInterval(t);
  }, []);

  async function onCreatePreset() {
    setSubmitting(true);
    try {
      const run = await createMmMtfSweepPreset({ symbol, start, end, maker_fee_bps_list: '10' });
      await refresh();
      router.push(`/runs/${run.id}`);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  }

  const activeCount = useMemo(
    () => runs.filter((r) => r.status === 'queued' || r.status === 'running').length,
    [runs]
  );

  const knownKinds = useMemo(() => {
    const set = new Set<string>();
    for (const r of runs) set.add(r.kind);
    return Array.from(set).sort();
  }, [runs]);

  const filteredRuns = useMemo(() => {
    const q = query.trim().toLowerCase();
    return runs.filter((r) => {
      if (statusFilter !== 'all' && r.status !== statusFilter) return false;
      if (kindFilter !== 'all' && r.kind !== kindFilter) return false;
      if (!q) return true;
      return (
        r.name.toLowerCase().includes(q) ||
        r.id.toLowerCase().includes(q) ||
        r.kind.toLowerCase().includes(q)
      );
    });
  }, [runs, statusFilter, kindFilter, query]);

  return (
    <section className="stack">
      <div className="card stack">
        <h1>Runs</h1>
        <p className="muted">Запуск и мониторинг backtest/sweep задач в Railway.</p>

        <div className="form-grid">
          <label>
            Symbol
            <input value={symbol} onChange={(e) => setSymbol(e.target.value)} />
          </label>
          <label>
            Start
            <input value={start} onChange={(e) => setStart(e.target.value)} />
          </label>
          <label>
            End
            <input value={end} onChange={(e) => setEnd(e.target.value)} />
          </label>
        </div>

        <div className="row gap">
          <button onClick={onCreatePreset} disabled={submitting}>
            {submitting ? 'Creating...' : 'Run MM MTF Sweep Preset'}
          </button>
          <button className="ghost" onClick={refresh}>
            Refresh
          </button>
          <span className="muted tiny">Active: {activeCount}</span>
        </div>

        <div className="filter-grid">
          <label>
            Status
            <select value={statusFilter} onChange={(e) => setStatusFilter(e.target.value)}>
              <option value="all">all</option>
              <option value="queued">queued</option>
              <option value="running">running</option>
              <option value="completed">completed</option>
              <option value="failed">failed</option>
            </select>
          </label>
          <label>
            Kind
            <select value={kindFilter} onChange={(e) => setKindFilter(e.target.value)}>
              <option value="all">all</option>
              {knownKinds.map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
            </select>
          </label>
          <label>
            Search
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="name / id / kind"
            />
          </label>
        </div>

        {error ? <p className="error">{error}</p> : null}
      </div>

      <div className="card">
        {loading ? <p>Loading...</p> : null}
        <table>
          <thead>
            <tr>
              <th>Created</th>
              <th>Name</th>
              <th>Kind</th>
              <th>Status</th>
              <th>Action</th>
            </tr>
          </thead>
          <tbody>
            {filteredRuns.map((run) => (
              <tr key={run.id}>
                <td className="tiny">{new Date(run.created_at).toLocaleString()}</td>
                <td>{run.name}</td>
                <td className="mono tiny">{run.kind}</td>
                <td>
                  <span className={statusClass(run.status)}>{run.status}</span>
                </td>
                <td>
                  <Link href={`/runs/${run.id}`}>Open</Link>
                </td>
              </tr>
            ))}
            {!filteredRuns.length ? (
              <tr>
                <td colSpan={5} className="muted tiny">
                  No runs match the current filters
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
    </section>
  );
}
