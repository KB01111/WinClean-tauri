import { describe, expect, test } from "bun:test"

import { shouldRefreshRunDataForProgress } from "./scan-refresh"

describe("scan refresh policy", () => {
  test("defers heavyweight run data while a scan is active", () => {
    expect(shouldRefreshRunDataForProgress({ state: "running" })).toBe(false)
  })

  test("refreshes indexed data once a scan leaves the active state", () => {
    expect(shouldRefreshRunDataForProgress({ state: "completed" })).toBe(true)
    expect(shouldRefreshRunDataForProgress({ state: "failed" })).toBe(true)
    expect(shouldRefreshRunDataForProgress({ state: "canceled" })).toBe(true)
  })
})
