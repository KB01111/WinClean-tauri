export type ScanRefreshProgress = {
  state: "idle" | "running" | "completed" | "failed" | "canceled"
}

export function shouldRefreshRunDataForProgress(
  progress: ScanRefreshProgress
) {
  return progress.state !== "running"
}

export function shouldRefreshDashboardCollectionsForProgress(
  progress: ScanRefreshProgress
) {
  return progress.state !== "running"
}

export function runDataRefreshKey(
  runId: number | null | undefined,
  parentPath: string | null | undefined
) {
  return `${runId ?? "none"}:${parentPath ?? ""}`
}
