'use client';

import type { EquityPoint, TradePoint } from '@/lib/types';

function nearestEquityByTs(points: EquityPoint[], ts: number): EquityPoint | null {
  if (!points.length) return null;
  let lo = 0;
  let hi = points.length - 1;
  while (lo < hi) {
    const mid = Math.floor((lo + hi + 1) / 2);
    if (points[mid].ts <= ts) lo = mid;
    else hi = mid - 1;
  }
  return points[lo] ?? null;
}

export function TradeOverlayChart({
  equity,
  trades,
  height = 220
}: {
  equity: EquityPoint[];
  trades: TradePoint[];
  height?: number;
}) {
  const width = 900;
  if (!equity.length) {
    return <div className="muted">No equity data yet</div>;
  }

  const minX = equity[0].ts;
  const maxX = equity[equity.length - 1].ts;
  const minY = Math.min(...equity.map((p) => p.equity));
  const maxY = Math.max(...equity.map((p) => p.equity));
  const xRange = maxX - minX || 1;
  const yRange = maxY - minY || 1;

  const toX = (ts: number) => ((ts - minX) / xRange) * (width - 20) + 10;
  const toY = (v: number) => height - (((v - minY) / yRange) * (height - 20) + 10);

  const path = equity
    .map((p, i) => `${i === 0 ? 'M' : 'L'}${toX(p.ts).toFixed(2)},${toY(p.equity).toFixed(2)}`)
    .join(' ');

  const tradeDots = trades
    .map((t) => {
      const base = nearestEquityByTs(equity, t.ts);
      if (!base) return null;
      return {
        ts: t.ts,
        side: t.side.toUpperCase(),
        x: toX(t.ts),
        y: toY(base.equity)
      };
    })
    .filter((v): v is { ts: number; side: string; x: number; y: number } => v !== null);

  return (
    <div>
      <div className="label">Equity + BUY/SELL markers</div>
      <svg viewBox={`0 0 ${width} ${height}`} className="chart" aria-label="equity and trades chart">
        <rect x="0" y="0" width={width} height={height} fill="transparent" />
        <path d={path} stroke="#22c55e" strokeWidth="2.2" fill="none" />
        {tradeDots.map((d, i) => (
          <circle
            key={`${d.ts}-${i}`}
            cx={d.x}
            cy={d.y}
            r={2.8}
            fill={d.side === 'BUY' ? '#3b82f6' : '#f97316'}
            opacity={0.9}
          />
        ))}
      </svg>
      <div className="row gap tiny">
        <span className="legend-dot legend-buy" /> <span className="muted">BUY</span>
        <span className="legend-dot legend-sell" /> <span className="muted">SELL</span>
      </div>
    </div>
  );
}
