import { describe, expect, test } from "bun:test"

import {
  auditTone,
  canRollback,
  clampInt,
  customProfileId,
  describeError,
  describeMeasurement,
  ecoQosRestorePids,
  filterServices,
  formatMwh,
  formatRuntime,
  formatWatts,
  healthLabel,
  isConflict,
  isElevationRequired,
  itemsToChanges,
  newCustomProfile,
  parseOptimizerEvent,
  profileLabel,
  recommendationMemoryMb,
  routeHeadline,
  startTypeLabel,
  suggestProfile,
  summarizeServices,
  type OptimizationSnapshot,
  type PowerProfile,
  type ServiceInfo,
  type ServiceRecommendation,
} from "./optimizer"

const svc = (name: string, risk: "protected" | "caution" | "safe", over: Partial<ServiceInfo> = {}): ServiceInfo => ({
  name,
  displayName: name,
  state: "stopped",
  startType: "manual",
  delayedAutoStart: false,
  pid: null,
  dependencies: [],
  dependents: [],
  canChange: risk !== "protected",
  ownProcess: true,
  triggerStart: false,
  memoryMb: null,
  binaryPath: null,
  classification: { risk, known: risk !== "caution", category: "unknown", canDisable: risk === "safe", reason: "r", recommendation: "keep" },
  ...over,
})

describe("errors", () => {
  test("command errors, Error objects and strings all become readable text", () => {
    expect(describeError({ code: "rejected", message: "protected service" })).toBe("protected service")
    expect(describeError(new Error("boom"))).toBe("boom")
    expect(describeError("plain")).toBe("plain")
    expect(describeError(42)).toBe("42")
  })

  test("elevation and conflict codes are recognised", () => {
    expect(isElevationRequired({ code: "elevationRequired", message: "x" })).toBe(true)
    expect(isElevationRequired({ code: "failed", message: "x" })).toBe(false)
    expect(isElevationRequired(new Error("elevation"))).toBe(false)
    expect(isConflict({ code: "conflict", message: "x" })).toBe(true)
    expect(isConflict(null)).toBe(false)
  })
})

describe("formatting never invents numbers", () => {
  test("watts", () => {
    expect(formatWatts(12.34)).toBe("12.3 W")
    expect(formatWatts(120)).toBe("120 W")
    expect(formatWatts(null)).toBe("—")
    expect(formatWatts(undefined)).toBe("—")
    expect(formatWatts(Number.NaN)).toBe("—")
  })

  test("runtime", () => {
    expect(formatRuntime(185)).toBe("3 h 05 min")
    expect(formatRuntime(42)).toBe("42 min")
    expect(formatRuntime(null)).toBe("—")
    expect(formatRuntime(-5)).toBe("—")
    expect(formatRuntime(1e9)).toBe("99 h 00 min")
  })

  test("energy and health", () => {
    expect(formatMwh(45_000)).toBe("45.0 Wh")
    expect(formatMwh(null)).toBe("—")
    expect(healthLabel(92)).toEqual({ label: "Good", tone: "good" })
    expect(healthLabel(75).tone).toBe("fair")
    expect(healthLabel(60).tone).toBe("poor")
    expect(healthLabel(null).tone).toBe("unknown")
  })
})

describe("services", () => {
  const list = [
    svc("WinDefend", "protected", { state: "running" }),
    svc("DiagTrack", "caution", { state: "running", startType: "automatic" }),
    svc("Fax", "safe"),
  ]

  test("filtering by text, risk, running and changeable", () => {
    const base = { query: "", risk: "all" as const, onlyRunning: false, onlyChangeable: false }
    expect(filterServices(list, base)).toHaveLength(3)
    expect(filterServices(list, { ...base, query: "diag" }).map((s) => s.name)).toEqual(["DiagTrack"])
    expect(filterServices(list, { ...base, risk: "protected" }).map((s) => s.name)).toEqual(["WinDefend"])
    expect(filterServices(list, { ...base, onlyRunning: true })).toHaveLength(2)
    expect(filterServices(list, { ...base, onlyChangeable: true }).map((s) => s.name)).toEqual(["DiagTrack", "Fax"])
  })

  test("summary counts only real memory", () => {
    const s = summarizeServices([...list, svc("Own", "caution", { memoryMb: 40.4, state: "running" })])
    expect(s).toEqual({ total: 4, running: 3, protectedCount: 1, autoStart: 1, memoryMb: 40 })
  })

  test("recommendation memory sums running services only", () => {
    const rec = (running: boolean, memoryMb: number | null): ServiceRecommendation => ({
      serviceName: "x", displayName: "x", currentStartType: "automatic", suggestedStartType: "manual", risk: "caution", category: "optional",
      rationale: "r", confidence: "medium", running, memoryMb, startsAtBoot: true, requiresElevation: true,
    })
    expect(recommendationMemoryMb([rec(true, 10.04), rec(true, null), rec(false, 500)])).toBe(10)
  })

  test("labels and change requests", () => {
    expect(startTypeLabel("automaticDelayed")).toBe("Automatic (delayed)")
    expect(startTypeLabel("weird")).toBe("weird")
    expect(itemsToChanges([{ name: "A", to: "manual" }])).toEqual([{ serviceName: "A", startType: "manual" }])
  })
})

describe("history and profiles", () => {
  const snap = (kind: string, rolledBackAtMs: number | null): OptimizationSnapshot => ({ id: 1, kind, target: "t", payloadJson: "{}", createdAtMs: 0, rolledBackAtMs })

  test("only unused, supported snapshots can be rolled back", () => {
    expect(canRollback(snap("serviceBatch", null))).toBe(true)
    expect(canRollback(snap("service", null))).toBe(true)
    expect(canRollback(snap("powerScheme", null))).toBe(true)
    expect(canRollback(snap("powerScheme", 5))).toBe(false)
    expect(canRollback(snap("other", null))).toBe(false)
  })

  test("audit statuses map to tones", () => {
    expect(auditTone("completed")).toBe("ok")
    for (const s of ["partial", "interrupted", "rolledBack", "skipped"]) expect(auditTone(s)).toBe("warn")
    expect(auditTone("failed")).toBe("bad")
    expect(auditTone("pending")).toBe("neutral")
  })

  test("profile labels fall back to the id", () => {
    const profiles = [{ id: "gaming", label: "Gaming" } as PowerProfile]
    expect(profileLabel(profiles, "gaming")).toBe("Gaming")
    expect(profileLabel(profiles, "custom:x")).toBe("custom:x")
    expect(profileLabel(profiles, null)).toBe("None")
  })

  test("profile suggestion mirrors the backend rule", () => {
    expect(suggestProfile({ onAc: false, percent: 12 })).toBe("batterySaver")
    expect(suggestProfile({ onAc: true, percent: 12 })).toBe("balanced")
    expect(suggestProfile({ onAc: false, percent: 90, gaming: true })).toBe("gaming")
  })

  test("EcoQoS restore list only includes throttled processes", () => {
    expect(ecoQosRestorePids([{ pid: 1, ecoQosApplied: true }, { pid: 2, ecoQosApplied: false }])).toEqual([1])
  })

  test("route headline", () => {
    expect(routeHeadline([])).toBe("CPU only")
    expect(routeHeadline([{ ep: "VitisAIExecutionProvider", device: "npu", status: "candidate", reason: "" }])).toBe("VitisAI (NPU) first")
    expect(routeHeadline([{ ep: "CPUExecutionProvider", device: "cpu", status: "fallback", reason: "" }])).toBe("CPU only")
  })
})

describe("optimizer events", () => {
  test("valid events pass, anything else is ignored", () => {
    expect(parseOptimizerEvent({ kind: "profileChanged", data: { profileId: "gaming" } })?.kind).toBe("profileChanged")
    expect(parseOptimizerEvent({ kind: "workload", data: { kind: "coding" } })?.kind).toBe("workload")
    expect(parseOptimizerEvent({ kind: "nope", data: {} })).toBeNull()
    expect(parseOptimizerEvent({ kind: "workload" })).toBeNull()
    expect(parseOptimizerEvent(null)).toBeNull()
    expect(parseOptimizerEvent("string")).toBeNull()
  })
})

describe("custom profiles", () => {
  const valid = (id: string) => /^custom:[A-Za-z0-9-]+$/.test(id) && id.length >= 8 && id.length <= 48

  test("ids follow the backend rules and stay unique", () => {
    expect(customProfileId("Travel mode!", [])).toBe("custom:travel-mode")
    expect(customProfileId("Travel mode!", ["custom:travel-mode"])).toBe("custom:travel-mode-2")
    expect(customProfileId("!!!", [])).toBe("custom:profile")
    expect(valid(customProfileId("x", []))).toBe(true)
    for (const label of ["Ünïcode café", "a".repeat(80), "  spaced  out  ", "UPPER lower 123"]) {
      expect(valid(customProfileId(label, []))).toBe(true)
    }
  })

  test("a new profile derives the throttling flag from its mode and is never built in", () => {
    const p = newCustomProfile({ label: "  Train  ", description: " on the move ", schemeGuid: "a1841308-3541-4fab-bc81-f71556f20b4a", ecoQosMode: "battery", existingIds: [] })
    expect(p).toMatchObject({ id: "custom:train", label: "Train", description: "on the move", builtin: false, ecoQosForBackground: true })
    expect(newCustomProfile({ label: "Plain", description: "", schemeGuid: "x", ecoQosMode: "off", existingIds: [] }).ecoQosForBackground).toBe(false)
  })

  test("clampInt bounds and sanitises form input", () => {
    expect(clampInt(150, 5, 95, 20)).toBe(95)
    expect(clampInt(-3, 5, 95, 20)).toBe(5)
    expect(clampInt(Number.NaN, 5, 95, 20)).toBe(20)
    expect(clampInt(42.6, 5, 95, 20)).toBe(43)
  })
})

describe("measurement wording", () => {
  const base = { baseline: null, after: null, ci95W: null, notes: [] }
  test("each verdict is described without overclaiming", () => {
    expect(describeMeasurement(null).tone).toBe("neutral")
    expect(describeMeasurement({ ...base, verdict: "improved", deltaW: -2.5, deltaPercent: -20 })).toEqual({
      headline: "Draw fell by 2.5 W (20%), beyond measurement noise.",
      tone: "good",
    })
    expect(describeMeasurement({ ...base, verdict: "worse", deltaW: 1.2, deltaPercent: 10 }).tone).toBe("warn")
    expect(describeMeasurement({ ...base, verdict: "inconclusive", deltaW: 0.1, deltaPercent: 1 }).headline).toContain("No difference")
    expect(describeMeasurement({ ...base, verdict: "insufficientData", deltaW: null, deltaPercent: null }).headline).toContain("Not enough")
  })
})
