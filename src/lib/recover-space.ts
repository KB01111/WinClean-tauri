export type RecoverMode = "clean" | "offload" | "review"

export type RecoverCandidate = {
  id: string
  kind: RecoverMode
  recommendedAction: "recycleBin" | "googleDriveOffload" | "review"
  eligibleForOffload: boolean
  size: number
}

export type DriveStatus = {
  configured: boolean
  connected: boolean
}

export type OffloadActionInput = {
  selectedCount: number
  driveStatus: DriveStatus | null
  running: boolean
}

export type OffloadActionState = {
  disabled: boolean
  label: string
  reason: string | null
}

export type OffloadProgressInput = {
  totalItems: number
  completedItems: number
  failedItems: number
  uploadedBytes: number
  totalBytes: number
  status: string
}

export type RecoverCandidateSummary = {
  cleanBytes: number
  cleanCount: number
  offloadBytes: number
  offloadCount: number
  reviewBytes: number
  reviewCount: number
  totalBytes: number
  totalCount: number
}

export type RecoverCandidateWindow<T> = {
  rows: T[]
  hiddenCount: number
  totalCount: number
}

export function filterRecommendations<T extends RecoverCandidate>(
  candidates: readonly T[],
  mode: RecoverMode
): T[] {
  return candidates.filter((candidate) => candidate.kind === mode)
}

export function getVisibleRecoverCandidates<T extends RecoverCandidate>(
  candidates: readonly T[],
  mode: RecoverMode,
  executableCleanIds: ReadonlySet<string>
): T[] {
  const visible = filterRecommendations(candidates, mode)
  if (mode !== "clean") {
    return visible
  }
  return visible.filter((candidate) => executableCleanIds.has(candidate.id))
}

export function selectedVisibleCandidateIds<T extends RecoverCandidate>(
  candidates: readonly T[],
  selectedIds: ReadonlySet<string>
): string[] {
  return candidates
    .filter((candidate) => selectedIds.has(candidate.id))
    .map((candidate) => candidate.id)
}

export function summarizeRecoverCandidates(
  candidates: readonly RecoverCandidate[]
): RecoverCandidateSummary {
  return candidates.reduce<RecoverCandidateSummary>(
    (summary, candidate) => {
      summary.totalBytes += candidate.size
      summary.totalCount += 1
      if (candidate.kind === "clean") {
        summary.cleanBytes += candidate.size
        summary.cleanCount += 1
      } else if (candidate.kind === "offload") {
        summary.offloadBytes += candidate.size
        summary.offloadCount += 1
      } else {
        summary.reviewBytes += candidate.size
        summary.reviewCount += 1
      }
      return summary
    },
    {
      cleanBytes: 0,
      cleanCount: 0,
      offloadBytes: 0,
      offloadCount: 0,
      reviewBytes: 0,
      reviewCount: 0,
      totalBytes: 0,
      totalCount: 0,
    }
  )
}

export function visibleRecoverCandidateWindow<T>(
  candidates: readonly T[],
  maxRows: number
): RecoverCandidateWindow<T> {
  const rowLimit = Math.max(0, Math.floor(maxRows))
  const rows = candidates.slice(0, rowLimit)
  return {
    rows,
    hiddenCount: Math.max(0, candidates.length - rows.length),
    totalCount: candidates.length,
  }
}

export function getOffloadActionState({
  selectedCount,
  driveStatus,
  running,
}: OffloadActionInput): OffloadActionState {
  if (running) {
    return {
      disabled: true,
      label: "Offload running",
      reason: "Wait for the current offload batch to finish.",
    }
  }
  if (!driveStatus?.configured) {
    return {
      disabled: true,
      label: "Configure Google Drive",
      reason: "Google Drive credentials are not configured.",
    }
  }
  if (!driveStatus.connected) {
    return {
      disabled: true,
      label: "Connect Google Drive",
      reason: "Connect Google Drive before offloading files.",
    }
  }
  if (selectedCount === 0) {
    return {
      disabled: true,
      label: "Offload to Drive",
      reason: "Select at least one offload candidate.",
    }
  }
  return {
    disabled: false,
    label: "Offload to Drive",
    reason: null,
  }
}

export function getOffloadProgress(input: OffloadProgressInput): {
  percent: number
  text: string
} {
  const percent =
    input.totalBytes > 0
      ? Math.min(
          100,
          Math.round((input.uploadedBytes / input.totalBytes) * 100)
        )
      : input.totalItems > 0
        ? Math.min(
            100,
            Math.round((input.completedItems / input.totalItems) * 100)
          )
        : 0
  const attention =
    input.failedItems > 0 ? `; ${input.failedItems} needs attention` : ""
  return {
    percent,
    text: `${input.completedItems} of ${input.totalItems} uploaded${attention}`,
  }
}
