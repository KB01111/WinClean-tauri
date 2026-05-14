import { describe, expect, test } from "bun:test"

import {
  runDataRefreshKey,
  shouldRefreshRunDataForProgress,
} from "./scan-refresh"

describe("scan refresh policy", () => {
  test("defers heavyweight run data while a scan is active", () => {
    expect(shouldRefreshRunDataForProgress({ state: "running" })).toBe(false)
  })

  test("refreshes indexed data once a scan leaves the active state", () => {
    expect(shouldRefreshRunDataForProgress({ state: "completed" })).toBe(true)
    expect(shouldRefreshRunDataForProgress({ state: "failed" })).toBe(true)
    expect(shouldRefreshRunDataForProgress({ state: "canceled" })).toBe(true)
  })

  test("uses stable run-data keys to suppress duplicate completion refreshes", () => {
    expect(runDataRefreshKey(42, "C:\\Users")).toBe("42:C:\\Users")
    expect(runDataRefreshKey(null, null)).toBe("none:")
  })
})
