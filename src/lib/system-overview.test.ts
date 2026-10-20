import { describe, expect, test } from "bun:test"

import { nextActions, type OverviewFacts } from "./system-overview"

const quiet: OverviewFacts = {
  serviceRecommendations: 0,
  serviceRecommendationMb: 0,
  battery: { onAc: true, acKnown: true, percent: 90 },
  activeProfileId: "balanced",
  autoSwitchEnabled: true,
  recoverableBytes: null,
  workload: "general",
  dgpu: "poweredDown",
  interruptedChanges: 0,
}
const ids = (f: Partial<OverviewFacts>) => nextActions({ ...quiet, ...f }).map((a) => a.id)

describe("next actions", () => {
  test("a healthy, quiet system has nothing to suggest", () => {
    expect(ids({})).toEqual([])
  })

  test("interrupted changes come first and are warnings", () => {
    const a = nextActions({ ...quiet, interruptedChanges: 2, serviceRecommendations: 3 })
    expect(a[0]).toMatchObject({ id: "interrupted", tone: "warn", view: "optimizerHistory" })
    expect(a[0].detail).toContain("2 changes were")
  })

  test("low battery suggests a profile only when nothing automatic handles it", () => {
    const low = { battery: { onAc: false, acKnown: true, percent: 15 }, autoSwitchEnabled: false }
    expect(ids(low)).toContain("low-battery")
    expect(ids({ ...low, autoSwitchEnabled: true })).not.toContain("low-battery")
    expect(ids({ ...low, activeProfileId: "batterySaver" })).not.toContain("low-battery")
    expect(ids({ ...low, battery: { onAc: true, acKnown: true, percent: 15 } })).not.toContain("low-battery")
    expect(ids({ ...low, battery: { onAc: false, acKnown: false, percent: 15 } })).not.toContain("low-battery")
  })

  test("an awake dGPU is only a concern on battery and when no workload needs it", () => {
    const onBattery = { battery: { onAc: false, acKnown: true, percent: 80 }, dgpu: "awake" as const }
    expect(ids(onBattery)).toContain("dgpu-awake")
    expect(ids({ ...onBattery, workload: "gaming" })).not.toContain("dgpu-awake")
    expect(ids({ ...onBattery, workload: "aiInference" })).not.toContain("dgpu-awake")
    expect(ids({ dgpu: "awake" })).not.toContain("dgpu-awake")
    expect(ids({ ...onBattery, dgpu: "unknown" })).not.toContain("dgpu-awake")
  })

  test("service and storage suggestions state real numbers only", () => {
    const a = nextActions({ ...quiet, serviceRecommendations: 4, serviceRecommendationMb: 52.4, recoverableBytes: 3 * 1024 ** 3 })
    expect(a.find((x) => x.id === "services")?.detail).toContain("52 MB")
    expect(a.find((x) => x.id === "services")?.title).toBe("4 services could start on demand")
    expect(nextActions({ ...quiet, serviceRecommendations: 1 })[0].title).toBe("1 service could start on demand")
    expect(a.find((x) => x.id === "storage")?.title).toContain("3")
    expect(nextActions({ ...quiet, serviceRecommendations: 2 })[0].detail).not.toContain("MB")
  })

  test("warnings sort ahead of information and the list is capped", () => {
    const a = nextActions({
      ...quiet,
      serviceRecommendations: 1,
      recoverableBytes: 10,
      autoSwitchEnabled: false,
      battery: { onAc: false, acKnown: true, percent: 10 },
      interruptedChanges: 1,
      dgpu: "awake",
    })
    expect(a.length).toBeLessThanOrEqual(5)
    const firstInfo = a.findIndex((x) => x.tone === "info")
    expect(a.slice(0, firstInfo).every((x) => x.tone === "warn")).toBe(true)
  })
})
