export type ScanRefreshProgress = {
  state: "idle" | "running" | "completed" | "failed" | "canceled"
}

export function shouldRefreshRunDataForProgress(
  progress: ScanRefreshProgress
) {
  return progress.state !== "running"
}
