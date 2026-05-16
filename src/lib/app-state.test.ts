import { describe, expect, test } from "bun:test"

import {
  cleanupActionDisabled,
  directoryBreadcrumbs,
  parentDirectoryPath,
  pruneCleanupSelection,
  shouldCommitRunDataResponse,
} from "./app-state"

describe("app state helpers", () => {
  test("ignores stale run-data responses", () => {
    expect(
      shouldCommitRunDataResponse({
        requestId: 2,
        latestRequestId: 2,
      })
    ).toBe(true)
    expect(
      shouldCommitRunDataResponse({
        requestId: 1,
        latestRequestId: 2,
      })
    ).toBe(false)
  })

  test("disables cleanup while pending or without selected candidates", () => {
    expect(
      cleanupActionDisabled({
        hasPlan: true,
        selectedCount: 1,
        pending: false,
        scanning: false,
      })
    ).toBe(false)
    expect(
      cleanupActionDisabled({
        hasPlan: true,
        selectedCount: 1,
        pending: true,
        scanning: false,
      })
    ).toBe(true)
    expect(
      cleanupActionDisabled({
        hasPlan: true,
        selectedCount: 0,
        pending: false,
        scanning: false,
      })
    ).toBe(true)
  })

  test("builds Windows parent navigation without escaping the scan root", () => {
    expect(parentDirectoryPath("C:\\Users\\Kevin", "C:\\")).toBe("C:\\Users")
    expect(parentDirectoryPath("C:\\Users", "C:\\Users")).toBeNull()
    expect(parentDirectoryPath("C:\\Users", "D:\\")).toBeNull()
  })

  test("creates breadcrumbs from root to current directory", () => {
    expect(directoryBreadcrumbs("C:\\", "C:\\Users\\Kevin")).toEqual([
      { label: "C:\\", path: "C:\\" },
      { label: "Users", path: "C:\\Users" },
      { label: "Kevin", path: "C:\\Users\\Kevin" },
    ])
    expect(directoryBreadcrumbs("C:\\Users", "C:\\Users")).toEqual([
      { label: "C:\\Users", path: "C:\\Users" },
    ])
  })

  test("prunes cleanup selection to candidates that still exist", () => {
    expect(pruneCleanupSelection(new Set(["a", "b"]), ["b", "c"])).toEqual(
      new Set(["b"])
    )
  })
})
