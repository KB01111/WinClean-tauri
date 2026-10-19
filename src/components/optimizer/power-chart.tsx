import { useMemo } from "react"

import { acSpans, buildSeries, describeSeries, linePath } from "@/lib/telemetry-chart"
import type { TelemetrySample } from "@/lib/optimizer"

const W = 640
const H = 160

/**
 * Battery draw over time as a single SVG path: a few hundred points at most,
 * no animation and no chart library, so it costs almost nothing to render.
 * Stretches on AC are shaded, not drawn as measurements.
 */
export function PowerChart({ samples }: { samples: TelemetrySample[] }) {
  const series = useMemo(() => buildSeries(samples), [samples])
  const path = useMemo(() => linePath(series, W, H), [series])
  const ac = useMemo(() => acSpans(series, W), [series])
  const description = describeSeries(series)
  const hasLine = path !== ""

  return (
    <figure className="m-0">
      <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label={description} className="h-40 w-full text-primary">
        {[0.25, 0.5, 0.75].map((f) => (
          <line key={f} x1={0} x2={W} y1={H * f} y2={H * f} className="stroke-border" strokeWidth={1} strokeDasharray="3 4" />
        ))}
        {ac.map(([a, b]) => (
          <rect key={`${a}-${b}`} x={a} y={0} width={b - a} height={H} className="fill-primary/10" />
        ))}
        {hasLine ? <path d={path} fill="none" stroke="currentColor" strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" /> : null}
        <text x={6} y={12} className="fill-muted-foreground text-[10px]">
          {series.maxWatts} W
        </text>
        <text x={6} y={H - 4} className="fill-muted-foreground text-[10px]">
          0 W
        </text>
      </svg>
      <figcaption className="mt-1 text-xs text-muted-foreground">
        {hasLine ? "Battery power draw. Shaded areas: plugged in (not measured)." : description}
      </figcaption>
    </figure>
  )
}
