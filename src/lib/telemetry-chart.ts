// Pure helpers for the battery power chart: bucket samples, build SVG paths.
// Kept free of React so the maths is unit-tested and the chart stays cheap to
// render (a few hundred points at most).

import type { TelemetrySample } from "@/lib/optimizer"

export type SeriesPoint = {
  atMs: number
  /** Mean discharge power in this bucket; null = no discharge reading (AC or unknown). */
  watts: number | null
  percent: number | null
  onAc: boolean
}

export type Series = {
  points: SeriesPoint[]
  minAtMs: number
  maxAtMs: number
  /** Upper bound of the watt axis (rounded up to a tidy value). */
  maxWatts: number
}

/** Round up to a "nice" axis maximum (5, 10, 15, 20, 30, 40, 60, 80, 100, …). */
export function niceMax(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 10
  const steps = [5, 10, 15, 20, 30, 40, 60, 80, 100, 150, 200, 300]
  return steps.find((s) => s >= value) ?? Math.ceil(value / 100) * 100
}

/**
 * Buckets samples into at most `maxPoints` points. Within a bucket the watts
 * are the mean of the discharge samples only; a bucket with none is a gap.
 */
export function buildSeries(samples: TelemetrySample[], maxPoints = 120): Series {
  const sorted = [...samples].filter((s) => Number.isFinite(s.atMs)).sort((a, b) => a.atMs - b.atMs)
  if (sorted.length === 0) return { points: [], minAtMs: 0, maxAtMs: 0, maxWatts: 10 }
  const minAtMs = sorted[0].atMs
  const maxAtMs = sorted[sorted.length - 1].atMs
  const buckets = Math.max(1, Math.min(maxPoints, sorted.length))
  const span = Math.max(1, maxAtMs - minAtMs)
  const groups: TelemetrySample[][] = Array.from({ length: buckets }, () => [])
  for (const s of sorted) {
    const idx = Math.min(buckets - 1, Math.floor(((s.atMs - minAtMs) / span) * buckets))
    groups[idx].push(s)
  }
  const points: SeriesPoint[] = []
  for (const g of groups) {
    if (g.length === 0) continue
    const watts = g
      .filter((s) => !s.onAc && s.rateMw !== null && s.rateMw < 0)
      .map((s) => -(s.rateMw as number) / 1000)
    const percents = g.map((s) => s.percent).filter((p): p is number => p !== null)
    points.push({
      atMs: Math.round(g.reduce((sum, s) => sum + s.atMs, 0) / g.length),
      watts: watts.length ? watts.reduce((a, b) => a + b, 0) / watts.length : null,
      percent: percents.length ? percents[percents.length - 1] : null,
      onAc: g.every((s) => s.onAc),
    })
  }
  const top = Math.max(0, ...points.map((p) => p.watts ?? 0))
  return { points, minAtMs, maxAtMs, maxWatts: niceMax(top * 1.1) }
}

/**
 * SVG path for the watt line. Gaps (null watts) break the line instead of
 * being interpolated, so a stretch on AC is not drawn as a measurement.
 */
export function linePath(series: Series, width: number, height: number, pad = 4): string {
  const { points, minAtMs, maxAtMs, maxWatts } = series
  if (points.length === 0) return ""
  const span = Math.max(1, maxAtMs - minAtMs)
  const w = width - pad * 2
  const h = height - pad * 2
  let d = ""
  let open = false
  for (const p of points) {
    if (p.watts === null) {
      open = false
      continue
    }
    const x = pad + ((p.atMs - minAtMs) / span) * w
    const y = pad + h - Math.min(1, p.watts / maxWatts) * h
    d += `${open ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`
    open = true
  }
  return d
}

/** Spans of `[x0, x1]` (in 0..width) where the machine was on AC, for shading. */
export function acSpans(series: Series, width: number, pad = 4): [number, number][] {
  const { points, minAtMs, maxAtMs } = series
  if (points.length === 0) return []
  const span = Math.max(1, maxAtMs - minAtMs)
  const w = width - pad * 2
  const xs = points.map((p) => pad + ((p.atMs - minAtMs) / span) * w)
  const out: [number, number][] = []
  let start: number | null = null
  points.forEach((p, i) => {
    if (p.onAc && start === null) start = xs[i]
    if (!p.onAc && start !== null) {
      out.push([start, xs[i]])
      start = null
    }
  })
  if (start !== null) out.push([start, xs[xs.length - 1]])
  return out.filter(([a, b]) => b > a)
}

/** Text alternative for the chart (accessibility): what a screen reader gets. */
export function describeSeries(series: Series): string {
  const watts = series.points.map((p) => p.watts).filter((w): w is number => w !== null)
  if (watts.length === 0) return "No discharge power readings yet. Unplug the charger to start measuring."
  const mean = watts.reduce((a, b) => a + b, 0) / watts.length
  return `Battery power draw over time: average ${mean.toFixed(1)} watts, lowest ${Math.min(...watts).toFixed(1)}, highest ${Math.max(...watts).toFixed(1)} watts across ${watts.length} readings.`
}

/**
 * Appends a newer live sample to the history already on screen (capped at
 * `limit`), so the chart can follow the battery without re-fetching the whole
 * history every few seconds. An older or duplicate sample leaves it unchanged.
 */
export function appendSample(samples: TelemetrySample[], sample: TelemetrySample | null, limit: number): TelemetrySample[] {
  if (!sample || !Number.isFinite(sample.atMs)) return samples
  const last = samples[samples.length - 1]
  if (last && sample.atMs <= last.atMs) return samples
  const next = [...samples, sample]
  return next.length > limit ? next.slice(next.length - limit) : next
}
