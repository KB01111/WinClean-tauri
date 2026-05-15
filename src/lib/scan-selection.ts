export type DashboardScanState =
  | "idle"
  | "running"
  | "completed"
  | "failed"
  | "canceled"

export type DashboardRunLike = {
  id: number
  rootPath: string
  status?: string
}

export type DashboardRunSelectionInput<Run extends DashboardRunLike> = {
  runs: Run[]
  selectedRunId: number | null
  progressState: DashboardScanState
  progressRunId?: number | null
  progressRootPath?: string | null
  currentDirectory?: string | null
}

export type DashboardRunSelection = {
  runId: number | null
  directory: string | null
}

export function selectDashboardRun<Run extends DashboardRunLike>({
  runs,
  selectedRunId,
  progressState,
  progressRunId = null,
  progressRootPath = null,
  currentDirectory = null,
}: DashboardRunSelectionInput<Run>): DashboardRunSelection {
  if (progressState === "running" && progressRunId != null) {
    const activeRun = runs.find((run) => run.id === progressRunId)
    return {
      runId: progressRunId,
      directory: progressRootPath ?? activeRun?.rootPath ?? null,
    }
  }

  const selectedRun =
    selectedRunId == null
      ? null
      : (runs.find((run) => run.id === selectedRunId) ?? null)
  if (selectedRun) {
    return {
      runId: selectedRun.id,
      directory: currentDirectory ?? selectedRun.rootPath,
    }
  }

  const latestRun =
    runs.find((run) => run.status == null || run.status === "completed") ?? null
  return {
    runId: latestRun?.id ?? null,
    directory: latestRun?.rootPath ?? null,
  }
}
