import { describe, expect, test } from "bun:test"

import { selectDashboardRun } from "./scan-selection"

const runs = [
  { id: 1, rootPath: "C:\\" },
  { id: 2, rootPath: "D:\\" },
]

describe("dashboard run selection", () => {
  test("prefers the active scan over a stale selected run", () => {
    expect(
      selectDashboardRun({
        runs,
        selectedRunId: 1,
        progressState: "running",
        progressRunId: 2,
        progressRootPath: "D:\\",
        currentDirectory: "C:\\Users",
      })
    ).toEqual({ runId: 2, directory: "D:\\" })
  })

  test("keeps a valid selected run when no scan is active", () => {
    expect(
      selectDashboardRun({
        runs,
        selectedRunId: 1,
        progressState: "completed",
        progressRunId: 2,
        progressRootPath: "D:\\",
        currentDirectory: "C:\\Users",
      })
    ).toEqual({ runId: 1, directory: "C:\\Users" })
  })

  test("falls back to the latest run when the selected run is missing", () => {
    expect(
      selectDashboardRun({
        runs,
        selectedRunId: 99,
        progressState: "idle",
        progressRunId: null,
        progressRootPath: null,
        currentDirectory: "Z:\\",
      })
    ).toEqual({ runId: 1, directory: "C:\\" })
  })
})
