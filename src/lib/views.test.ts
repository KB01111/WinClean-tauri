import { describe, expect, test } from "bun:test"

import { DEFAULT_VIEW, NAV_GROUPS, VIEW_META, isStorageView, type View } from "./views"

describe("navigation model", () => {
  const grouped = NAV_GROUPS.flatMap((g) => g.views)

  test("every view is reachable exactly once", () => {
    const all = Object.keys(VIEW_META) as View[]
    expect([...grouped].sort()).toEqual([...all].sort())
    expect(new Set(grouped).size).toBe(grouped.length)
  })

  test("the default view exists and is in the navigation", () => {
    expect(grouped).toContain(DEFAULT_VIEW)
  })

  test("the ten required areas are all present", () => {
    const labels = grouped.map((v) => VIEW_META[v].label)
    for (const required of [
      "System Overview",
      "Windows Services",
      "Processes & EcoQoS",
      "Battery & Power",
      "Hardware & NPU",
      "Optimization Profiles",
      "AI Insights",
      "MCP Integrations",
      "History & Rollback",
    ]) {
      expect(labels).toContain(required)
    }
    expect(NAV_GROUPS.some((g) => g.label === "Storage and Cleanup")).toBe(true)
  })

  test("scan controls appear only on storage views", () => {
    expect(isStorageView("overview")).toBe(true)
    expect(isStorageView("cleanup")).toBe(true)
    for (const v of ["system", "services", "processes", "power", "hardware", "profiles", "mcp", "optimizerHistory", "settings"] as View[]) {
      expect(isStorageView(v)).toBe(false)
    }
  })
})
