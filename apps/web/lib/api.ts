import type {
  PresetRequest,
  RunArtifact,
  RunEventRecord,
  RunMetricsResponse,
  RunRecord
} from '@/lib/types';

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL;

function requireApiBase(): string {
  if (!API_BASE) {
    throw new Error('NEXT_PUBLIC_API_BASE_URL is not set');
  }
  return API_BASE;
}

async function jsonFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const base = requireApiBase();
  const res = await fetch(`${base}${path}`, {
    ...init,
    headers: {
      'content-type': 'application/json',
      ...(init?.headers || {})
    },
    cache: 'no-store'
  });

  if (!res.ok) {
    const text = await res.text();
    throw new Error(`API ${res.status}: ${text}`);
  }
  return res.json() as Promise<T>;
}

export function listRuns(): Promise<RunRecord[]> {
  return jsonFetch<RunRecord[]>('/runs?limit=100');
}

export function getRun(id: string): Promise<RunRecord> {
  return jsonFetch<RunRecord>(`/runs/${id}`);
}

export function getRunEvents(id: string): Promise<RunEventRecord[]> {
  return jsonFetch<RunEventRecord[]>(`/runs/${id}/events?limit=300`);
}

export async function getRunMetrics(id: string): Promise<RunMetricsResponse | null> {
  try {
    return await jsonFetch<RunMetricsResponse>(`/runs/${id}/metrics`);
  } catch {
    return null;
  }
}

export async function getRunArtifacts(id: string): Promise<RunArtifact[]> {
  try {
    return await jsonFetch<RunArtifact[]>(`/runs/${id}/artifacts`);
  } catch {
    return [];
  }
}

export function createMmMtfSweepPreset(req: PresetRequest): Promise<RunRecord> {
  return jsonFetch<RunRecord>('/runs/presets/mm_mtf_sweep', {
    method: 'POST',
    body: JSON.stringify(req)
  });
}
