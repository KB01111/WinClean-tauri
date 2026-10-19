import { describe, expect, test } from "bun:test"

import { acSpans, buildSeries, describeSeries, linePath, niceMax } from "./telemetry-chart"
import type { TelemetrySample } from "./optimizer"

const sample = (atMs: number, rateMw: number | null, onAc = false, percent: number | null = 50): TelemetrySample => ({
  atMs, percent, onAc, rateMw, voltageMv: null, capacityMwh: null, profileId: null, workload: null,
})

describe("niceMax", () => {
  test("rounds up to tidy axis maxima", () => {
    expect(niceMax(3)).toBe(5)
    expect(niceMax(12.1)).toBe(15)
    expect(niceMax(61)).toBe(80)
    expect(niceMax(0)).toBe(10)
    expect(niceMax(Number.NaN)).toBe(10)
    expect(niceMax(450)).toBe(500)
  })
})

describe("buildSeries", () => {
  test("empty input yields an empty series", () => {
    expect(buildSeries([]).points).toEqual([])
  })

  test("discharge samples are averaged per bucket and AC samples are gaps", () => {
    const samples = [sample(0, -10_000), sample(1000, -12_000), sample(2000, 30_000, true), sample(3000, 25_000, true)]
    const s = buildSeries(samples, 2)
    expect(s.points).toHaveLength(2)
    expect(s.points[0].watts).toBeCloseTo(11, 5)
    expect(s.points[1].watts).toBeNull()
    expect(s.points[1].onAc).toBe(true)
  })

  test("never produces more points than requested and sorts unordered input", () => {
    const samples = Array.from({ length: 500 }, (_, i) => sample((499 - i) * 1000, -8_000))
    const s = buildSeries(samples, 100)
    expect(s.points.length).toBeLessThanOrEqual(100)
    const times = s.points.map((p) => p.atMs)
    expect([...times].sort((a, b) => a - b)).toEqual(times)
    expect(s.maxWatts).toBeGreaterThanOrEqual(8)
  })

  test("non-finite timestamps are dropped", () => {
    expect(buildSeries([sample(Number.NaN, -1000), sample(5, -1000)], 10).points).toHaveLength(1)
  })
})

describe("linePath", () => {
  test("gaps break the line instead of interpolating across AC time", () => {
    const series = buildSeries([sample(0, -10_000), sample(1000, -10_000), sample(2000, 30_000, true), sample(3000, -10_000), sample(4000, -10_000)], 5)
    const d = linePath(series, 100, 50)
    expect((d.match(/M/g) ?? []).length).toBe(2)
    expect(d.startsWith("M")).toBe(true)
  })

  test("no data gives an empty path", () => {
    expect(linePath(buildSeries([]), 100, 50)).toBe("")
  })

  test("values stay inside the box", () => {
    const series = buildSeries([sample(0, -5_000), sample(1000, -50_000)], 2)
    const nums = linePath(series, 200, 80, 4).match(/-?\d+\.\d/g)!.map(Number)
    for (const n of nums) {
      expect(n).toBeGreaterThanOrEqual(0)
      expect(n).toBeLessThanOrEqual(200)
    }
  })
})

describe("acSpans and description", () => {
  test("AC stretches become shaded spans", () => {
    const series = buildSeries([sample(0, -1000), sample(1000, 1000, true), sample(2000, 1000, true), sample(3000, -1000)], 4)
    const spans = acSpans(series, 100)
    expect(spans).toHaveLength(1)
    expect(spans[0][1]).toBeGreaterThan(spans[0][0])
  })

  test("description reflects real numbers or says there are none", () => {
    expect(describeSeries(buildSeries([sample(0, 1000, true)], 5))).toContain("No discharge power readings")
    expect(describeSeries(buildSeries([sample(0, -10_000), sample(1000, -20_000)], 5))).toContain("average 15.0 watts")
  })
})
