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
