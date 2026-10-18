import { describe, expect, test } from "bun:test"

import {
  dischargeWatts,
  ecoQosRestorePids,
  suggestProfile,
  validateAiServiceSuggestions,
  type ServiceInfo,
} from "./optimizer"

const svc = (name: string, risk: string): ServiceInfo =>
  ({
    name,
    displayName: name,
    state: "stopped",
    startType: "manual",
    delayedAutoStart: false,
    pid: null,
    dependencies: [],
    canChange: risk !== "protected",
    classification: { risk, reason: "r", recommendation: "keep" },
  }) as unknown as ServiceInfo

describe("optimizer helpers", () => {
  test("protected services are rejected by AI validation", () => {
    const services = [svc("WinDefend", "protected"), svc("DiagTrack", "caution")]
    const ok = validateAiServiceSuggestions(
      [{ serviceName: "DiagTrack", startType: "manual" }],
      services
    )
    expect(ok.accepted).toHaveLength(1)
    const bad = validateAiServiceSuggestions(
      [{ serviceName: "WinDefend", startType: "disabled" }],
      services
    )
    expect(bad.accepted).toHaveLength(0)
    expect(bad.rejected[0]).toContain("protected")
  })

  test("unknown services and start types are rejected", () => {
    const r = validateAiServiceSuggestions(
      [
        { serviceName: "Nope", startType: "manual" },
        { serviceName: "DiagTrack", startType: "bogus" },
      ],
      [svc("DiagTrack", "caution")]
    )
    expect(r.accepted).toHaveLength(0)
    expect(r.rejected).toHaveLength(2)
  })

  test("EcoQoS restore list only contains applied pids", () => {
    expect(
      ecoQosRestorePids([
        { pid: 1, ecoQosApplied: true },
        { pid: 2, ecoQosApplied: false },
      ])
    ).toEqual([1])
  })

  test("profile suggestion mirrors the Rust policy", () => {
    expect(suggestProfile({ onAc: false, percent: 10 })).toBe("batterySaver")
    expect(suggestProfile({ onAc: true, percent: 10 })).toBe("balanced")
    expect(suggestProfile({ onAc: false, percent: 90, gaming: true })).toBe(
      "gaming"
    )
  })

  test("discharge watts needs a real drop while on battery", () => {
    expect(
      dischargeWatts(
        { atMs: 0, percent: 80, onAc: false },
        { atMs: 600_000, percent: 78, onAc: false },
        50_000
      )
    ).toBe(6)
    expect(
      dischargeWatts(
        { atMs: 0, percent: 80, onAc: false },
        { atMs: 600_000, percent: 78, onAc: false },
        null
      )
    ).toBeNull()
  })
})
