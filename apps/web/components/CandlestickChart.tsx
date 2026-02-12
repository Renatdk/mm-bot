'use client';

import type { TradePoint } from '@/lib/types';

export type Candle = {
  ts: number;
  open: number;
  high: number;
  low: number;
  close: number;
};

export function CandlestickChart({
  candles,
  trades,
  height = 260
}: {
  candles: Candle[];
  trades: TradePoint[];
  height?: number;
}) {
  const width = 900;
  if (!candles.length) {
    return <div className="muted">No candle data yet</div>;
  }

  const minX = candles[0].ts;
  const maxX = candles[candles.length - 1].ts;
  const minY = Math.min(...candles.map((c) => c.low));
  const maxY = Math.max(...candles.map((c) => c.high));
  const xRange = maxX - minX || 1;
  const yRange = maxY - minY || 1;

  const toX = (ts: number) => ((ts - minX) / xRange) * (width - 20) + 10;
  const toY = (v: number) => height - (((v - minY) / yRange) * (height - 20) + 10);

  const slot = (width - 20) / Math.max(candles.length, 1);
  const bodyWidth = Math.max(2, Math.min(10, slot * 0.7));

  const tradeDots = trades
    .map((t) => {
      if (t.ts < minX || t.ts > maxX || !Number.isFinite(t.price)) return null;
      return {
        ts: t.ts,
        side: t.side.toUpperCase(),
        x: toX(t.ts),
        y: toY(t.price)
      };
    })
    .filter((v): v is { ts: number; side: string; x: number; y: number } => v !== null);

  return (
    <div>
      <div className="label">Candles (OHLC) + BUY/SELL markers</div>
      <svg viewBox={`0 0 ${width} ${height}`} className="chart" aria-label="candlestick chart">
        <rect x="0" y="0" width={width} height={height} fill="transparent" />
        {candles.map((c, i) => {
          const x = toX(c.ts);
          const highY = toY(c.high);
          const lowY = toY(c.low);
          const openY = toY(c.open);
          const closeY = toY(c.close);
          const up = c.close >= c.open;
          const bodyY = Math.min(openY, closeY);
          const bodyH = Math.max(1, Math.abs(closeY - openY));
          const color = up ? '#22c55e' : '#ef4444';

          return (
            <g key={`${c.ts}-${i}`}>
              <line x1={x} y1={highY} x2={x} y2={lowY} stroke={color} strokeWidth="1.2" opacity="0.9" />
              <rect x={x - bodyWidth / 2} y={bodyY} width={bodyWidth} height={bodyH} fill={color} opacity="0.85" />
            </g>
          );
        })}
        {tradeDots.map((d, i) => (
          <circle
            key={`${d.ts}-${i}`}
            cx={d.x}
            cy={d.y}
            r={2.6}
            fill={d.side === 'BUY' ? '#3b82f6' : '#f97316'}
            opacity={0.95}
          />
        ))}
      </svg>
      <div className="row-between tiny muted">
        <span>{minY.toFixed(2)}</span>
        <span>{maxY.toFixed(2)}</span>
      </div>
      <div className="row gap tiny">
        <span className="legend-dot legend-candle-up" /> <span className="muted">Bull</span>
        <span className="legend-dot legend-candle-down" /> <span className="muted">Bear</span>
        <span className="legend-dot legend-buy" /> <span className="muted">BUY</span>
        <span className="legend-dot legend-sell" /> <span className="muted">SELL</span>
      </div>
    </div>
  );
}
