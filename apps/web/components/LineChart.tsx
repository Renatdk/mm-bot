'use client';

type Point = { x: number; y: number };

export function LineChart({
  points,
  height = 160,
  color = '#17c964',
  yLabel
}: {
  points: Point[];
  height?: number;
  color?: string;
  yLabel?: string;
}) {
  const width = 720;
  if (!points.length) {
    return <div className="muted">No chart data yet</div>;
  }

  const minX = Math.min(...points.map((p) => p.x));
  const maxX = Math.max(...points.map((p) => p.x));
  const minY = Math.min(...points.map((p) => p.y));
  const maxY = Math.max(...points.map((p) => p.y));

  const xRange = maxX - minX || 1;
  const yRange = maxY - minY || 1;

  const path = points
    .map((p, i) => {
      const x = ((p.x - minX) / xRange) * (width - 20) + 10;
      const y = height - (((p.y - minY) / yRange) * (height - 20) + 10);
      return `${i === 0 ? 'M' : 'L'}${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(' ');

  return (
    <div>
      {yLabel ? <div className="label">{yLabel}</div> : null}
      <svg viewBox={`0 0 ${width} ${height}`} className="chart" aria-label={yLabel || 'chart'}>
        <rect x="0" y="0" width={width} height={height} fill="transparent" />
        <path d={path} stroke={color} strokeWidth="2.5" fill="none" />
      </svg>
      <div className="row-between tiny muted">
        <span>{minY.toFixed(2)}</span>
        <span>{maxY.toFixed(2)}</span>
      </div>
    </div>
  );
}
