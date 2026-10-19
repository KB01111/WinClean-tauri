import { describe, expect, test } from "bun:test"

import { normalizeProposals, systemReportSchema } from "./ai-proposals"
import { OPTIMIZER_TOOL_NAMES } from "./ai-tools"
import type { PowerProfile, ServiceInfo } from "./optimizer"

const svc = (name: string, risk: "protected" | "caution" | "safe", startType = "automatic"): ServiceInfo => ({
  name, displayName: name, state: "running", startType, delayedAutoStart: false, pid: null, dependencies: [], dependents: [],
  canChange: risk !== "protected", ownProcess: true, triggerStart: false, memoryMb: null, binaryPath: null,
  classification: { risk, known: true, category: "unknown", canDisable: risk === "safe", reason: "r", recommendation: "k" },
})

const profiles = [{ id: "balanced", label: "Balanced" }, { id: "gaming", label: "Gaming" }] as PowerProfile[]
const services = [svc("WinDefend", "protected"), svc("DiagTrack", "caution"), svc("Fax", "safe", "manual")]

describe("AI proposals are filtered before they reach review", () => {
  test("valid proposals pass with their reasons", () => {
    const r = normalizeProposals({ serviceProposals: [{ serviceName: "diagtrack", startType: "manual", reason: "telemetry" }], profileProposal: { profileId: "balanced", reason: "idle" } }, services, profiles)
    expect(r.serviceChanges).toEqual([{ serviceName: "DiagTrack", startType: "manual" }])
    expect(r.reasons.DiagTrack).toBe("telemetry")
    expect(r.profile).toEqual({ profileId: "balanced", label: "Balanced", reason: "idle" })
    expect(r.dropped).toEqual([])
  })

  test("protected, unknown, duplicate, no-op and malformed proposals are dropped with a reason", () => {
    const r = normalizeProposals(
      {
        serviceProposals: [
          { serviceName: "WinDefend", startType: "disabled", reason: "x" },
          { serviceName: "Nope", startType: "manual", reason: "x" },
          { serviceName: "DiagTrack", startType: "manual", reason: "x" },
          { serviceName: "DiagTrack", startType: "disabled", reason: "x" },
          { serviceName: "Fax", startType: "manual", reason: "x" },
          { serviceName: "DiagTrack", startType: "sideways" as never, reason: "x" },
        ],
        profileProposal: { profileId: "turbo", reason: "x" },
      },
      services,
      profiles
    )
    expect(r.serviceChanges).toHaveLength(1)
    expect(r.dropped.join("|")).toContain("WinDefend: protected")
    expect(r.dropped.join("|")).toContain("Nope: not a service")
    expect(r.dropped.join("|")).toContain("proposed more than once")
    expect(r.dropped.join("|")).toContain("already manual")
    expect(r.dropped.join("|")).toContain("invalid start type")
    expect(r.profile).toBeNull()
    expect(r.dropped.join("|")).toContain("Profile 'turbo'")
  })
})

describe("structured output schema", () => {
  test("accepts a well-formed report and rejects anything with extra action types", () => {
    const ok = { summary: "s", findings: [{ area: "battery", severity: "info", title: "t", detail: "d" }], serviceProposals: [], profileProposal: null }
    expect(systemReportSchema.safeParse(ok).success).toBe(true)
    expect(systemReportSchema.safeParse({ ...ok, findings: [{ area: "kernel", severity: "info", title: "t", detail: "d" }] }).success).toBe(false)
    expect(systemReportSchema.safeParse({ ...ok, serviceProposals: [{ serviceName: "x", startType: "stop", reason: "r" }] }).success).toBe(false)
  })

  test("the schema has no way to express a process, file or registry action", () => {
    const keys = Object.keys(systemReportSchema.shape)
    expect(keys.sort()).toEqual(["findings", "profileProposal", "serviceProposals", "summary"])
  })
})

describe("assistant tools are read-only", () => {
  test("no tool name suggests a mutation", () => {
    for (const name of OPTIMIZER_TOOL_NAMES) {
      expect(name.startsWith("get_")).toBe(true)
      for (const word of ["set", "apply", "delete", "write", "rollback", "disable", "kill", "throttle"]) {
        expect(name.includes(word) && word !== "set").toBe(false)
      }
    }
  })
})
