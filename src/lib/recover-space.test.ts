import { describe, expect, test } from "bun:test"

import {
  filterRecommendations,
  getVisibleRecoverCandidates,
  getOffloadActionState,
  getOffloadProgress,
  selectedVisibleCandidateIds,
} from "./recover-space"

const candidates = [
  {
    id: "clean",
    kind: "clean",
    recommendedAction: "recycleBin",
    eligibleForOffload: false,
    size: 10,
  },
  {
    id: "offload",
    kind: "offload",
    recommendedAction: "googleDriveOffload",
    eligibleForOffload: true,
    size: 30,
  },
  {
    id: "review",
    kind: "review",
    recommendedAction: "review",
    eligibleForOffload: true,
    size: 20,
  },
] as const

describe("Recover Space helpers", () => {
  test("filters candidates by clean offload and review modes", () => {
    expect(
      filterRecommendations(candidates, "clean").map((item) => item.id)
    ).toEqual(["clean"])
    expect(
      filterRecommendations(candidates, "offload").map((item) => item.id)
    ).toEqual(["offload"])
    expect(
      filterRecommendations(candidates, "review").map((item) => item.id)
    ).toEqual(["review"])
  })

  test("limits clean mode to cleanup candidates the backend can execute", () => {
    expect(
      getVisibleRecoverCandidates(candidates, "clean", new Set(["clean"])).map(
        (item) => item.id
      )
    ).toEqual(["clean"])
    expect(
      getVisibleRecoverCandidates(candidates, "clean", new Set()).map(
        (item) => item.id
      )
    ).toEqual([])
    expect(
      getVisibleRecoverCandidates(candidates, "offload", new Set()).map(
        (item) => item.id
      )
    ).toEqual(["offload"])
  })

  test("submits only selected candidates visible in the active mode", () => {
    expect(
      selectedVisibleCandidateIds(
        [candidates[1]],
        new Set(["clean", "offload", "hidden"])
      )
    ).toEqual(["offload"])
  })

  test("requires Google Drive connection before offload can start", () => {
    expect(
      getOffloadActionState({
        selectedCount: 1,
        driveStatus: { configured: false, connected: false },
        running: false,
      })
    ).toEqual({
      disabled: true,
      label: "Configure Google Drive",
      reason: "Google Drive credentials are not configured.",
    })

    expect(
      getOffloadActionState({
        selectedCount: 1,
        driveStatus: { configured: true, connected: false },
        running: false,
      })
    ).toEqual({
      disabled: true,
      label: "Connect Google Drive",
      reason: "Connect Google Drive before offloading files.",
    })
  })

  test("summarizes batch upload progress", () => {
    expect(
      getOffloadProgress({
        totalItems: 4,
        completedItems: 2,
        failedItems: 1,
        uploadedBytes: 50,
        totalBytes: 100,
        status: "running",
      })
    ).toEqual({
      percent: 50,
      text: "2 of 4 uploaded; 1 needs attention",
    })
  })
})
