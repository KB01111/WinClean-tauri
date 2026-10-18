import { invoke } from "@tauri-apps/api/core"

import type { RecoverMode } from "@/lib/recover-space"

export type ScanMode = "ntfsUsn" | "recursive"
export type ScanState = "idle" | "running" | "completed" | "failed" | "canceled"

export type VolumeInfo = {
  name: string
  rootPath: string
  fileSystem?: string | null
  driveType: string
  isReady: boolean
  availableSpace?: number | null
  totalSpace?: number | null
}

export type ScanProgress = {
  state: ScanState
  runId?: number | null
  rootPath?: string | null
  mode?: ScanMode | null
  attemptedMode?: ScanMode | null
  fallbackReason?: string | null
  filesIndexed: number
  directoriesIndexed: number
  bytesIndexed: number
  accessDeniedCount: number
  skippedReparseCount: number
  errorCount: number
  lastError?: string | null
  workerCount?: number | null
  phase?: string | null
}

export type ScanRunRecord = {
  id: number
  rootPath: string
  fileSystem?: string | null
  scannerMode: string
  attemptedMode?: string | null
  fallbackReason?: string | null
  status: string
  startedAtMs: number
  finishedAtMs?: number | null
  filesIndexed: number
  directoriesIndexed: number
  bytesIndexed: number
  accessDeniedCount: number
  skippedReparseCount: number
  errorCount: number
  lastError?: string | null
}

export type TopDirectory = {
  path: string
  totalSize: number
  fileCount: number
  directoryCount: number
}

export type DirectoryChild = {
  id: number
  path: string
  parentPath?: string | null
  name: string
  extension?: string | null
  isDirectory: boolean
  size: number
  totalSize: number
  fileCount: number
  directoryCount: number
  modifiedAtMs?: number | null
  isReparsePoint: boolean
  isPlaceholder: boolean
  isSystem: boolean
}

export type FileSearchResult = {
  id: number
  path: string
  parentPath?: string | null
  name: string
  extension?: string | null
  size: number
  modifiedAtMs?: number | null
  isReparsePoint: boolean
  isPlaceholder: boolean
  isSystem: boolean
}

export type ExtensionSummary = {
  extension?: string | null
  fileCount: number
  totalSize: number
}

export type CategorySummary = {
  category: string
  fileCount: number
  totalSize: number
}

export type CleanupCandidate = {
  id: string
  entryId: number
  scanRunId: number
  path: string
  name: string
  size: number
  reason: string
  riskLevel: "low" | "review"
  actionKind: string
  modifiedAtMs?: number | null
}

export type CleanupPlan = {
  scanRunId: number
  candidates: CleanupCandidate[]
  totalBytes: number
  warnings: string[]
}

export type RecommendationCandidate = {
  id: string
  entryId: number
  scanRunId: number
  path: string
  name: string
  size: number
  kind: RecoverMode
  riskLevel: "low" | "review"
  recommendedAction: "recycleBin" | "googleDriveOffload" | "review"
  reason: string
  confidence: number
  ageDays?: number | null
  eligibleForOffload: boolean
  modifiedAtMs?: number | null
}

export type RecommendationPlan = {
  scanRunId: number
  candidates: RecommendationCandidate[]
  cleanBytes: number
  offloadBytes: number
  reviewBytes: number
  warnings: string[]
}

export type RecommendationFilters = {
  mode?: string | null
  minSize?: number | null
  minAgeDays?: number | null
  onlyOffloadEligible?: boolean | null
}

export type CleanupActionRecord = {
  id: number
  actionType: string
  status: string
  targetPath: string
  originalPath?: string | null
  destinationPath?: string | null
  scanRunId?: number | null
  explanation?: string | null
  createdAtMs: number
  completedAtMs?: number | null
  undoToken?: string | null
}

export type CleanupExecutionResult = {
  recycledCount: number
  failedCount: number
  skippedCount: number
  bytesRequested: number
}

export type AppHealth = {
  appVersion: string
  databasePath: string
  schemaVersion?: string | null
  latestCompletedRunId?: number | null
  activeScanState: ScanState
  pendingActionCount: number
  pendingOffloadCount: number
  localDiagnosticsOnly: boolean
  recycleBinCleanup: boolean
  googleDriveConfigured: boolean
  googleDriveConnected: boolean
}

export type GoogleDriveStatus = {
  configured: boolean
  connected: boolean
  accountEmail?: string | null
  error?: string | null
}

export type OffloadItemRecord = {
  id: number
  jobId: number
  entryId: number
  candidateId: string
  sourcePath: string
  size: number
  status: string
  driveFileId?: string | null
  driveWebUrl?: string | null
  uploadedBytes: number
  md5Checksum?: string | null
  error?: string | null
  recycleStatus?: string | null
  createdAtMs: number
  completedAtMs?: number | null
}

export type OffloadJobStatus = {
  id: number
  scanRunId: number
  status: string
  destinationLabel: string
  driveFolderId?: string | null
  totalItems: number
  completedItems: number
  failedItems: number
  totalBytes: number
  uploadedBytes: number
  createdAtMs: number
  startedAtMs?: number | null
  completedAtMs?: number | null
  lastError?: string | null
  items: OffloadItemRecord[]
}

export type ExportArtifact = {
  path: string
  createdAtMs: number
  bytes: number
}

export type AiKeyStatus = {
  configured: boolean
}

export const emptyScanProgress: ScanProgress = {
  state: "idle",
  filesIndexed: 0,
  directoriesIndexed: 0,
  bytesIndexed: 0,
  accessDeniedCount: 0,
  skippedReparseCount: 0,
  errorCount: 0,
}

export const api = {
  listVolumes: () => invoke<VolumeInfo[]>("list_volumes"),
  startScan: (rootPath: string, fileSystem: string | null) =>
    invoke<ScanProgress>("start_scan", { rootPath, fileSystem }),
  cancelScan: () => invoke<ScanProgress>("cancel_scan"),
  scanStatus: () => invoke<ScanProgress>("scan_status"),
  scanRuns: (limit = 30) => invoke<ScanRunRecord[]>("scan_runs", { limit }),
  topDirectories: (runId: number | null, limit = 25) =>
    invoke<TopDirectory[]>("top_directories", { runId, limit }),
  directoryChildren: (
    runId: number | null,
    parentPath: string | null,
    limit = 250
  ) => invoke<DirectoryChild[]>("directory_children", { runId, parentPath, limit }),
  fileSearch: (params: {
    runId: number | null
    query?: string | null
    extension?: string | null
    minSize?: number | null
    limit?: number
  }) =>
    invoke<FileSearchResult[]>("file_search", {
      runId: params.runId,
      query: params.query ?? null,
      extension: params.extension ?? null,
      minSize: params.minSize ?? null,
      limit: params.limit ?? 100,
    }),
  extensionSummary: (runId: number | null, limit = 20) =>
    invoke<ExtensionSummary[]>("extension_summary", { runId, limit }),
  categorySummary: (runId: number | null) =>
    invoke<CategorySummary[]>("category_summary", { runId }),
  cleanupPlan: (runId: number | null, limit = 500) =>
    invoke<CleanupPlan | null>("cleanup_plan", { runId, limit }),
  recommendations: (
    runId: number | null,
    filters: RecommendationFilters | null = null
  ) => invoke<RecommendationPlan | null>("recommendations", { runId, filters }),
  executeCleanup: (runId: number, candidateIds: string[]) =>
    invoke<CleanupExecutionResult>("execute_cleanup", { runId, candidateIds }),
  cleanupHistory: (limit = 50) =>
    invoke<CleanupActionRecord[]>("cleanup_history", { limit }),
  openRecycleBin: () => invoke("open_recycle_bin"),
  revealInExplorer: (path: string) =>
    invoke("reveal_in_explorer", { path }),
  deleteScanRun: (runId: number) => invoke<boolean>("delete_scan_run", { runId }),
  clearScanHistory: () => invoke<number>("clear_scan_history"),
  googleDriveStatus: () => invoke<GoogleDriveStatus>("google_drive_status"),
  beginGoogleDriveAuth: () => invoke<GoogleDriveStatus>("begin_google_drive_auth"),
  disconnectGoogleDrive: () =>
    invoke<GoogleDriveStatus>("disconnect_google_drive"),
  startOffload: (runId: number, candidateIds: string[]) =>
    invoke<OffloadJobStatus>("start_offload", { runId, candidateIds }),
  offloadStatus: (jobId: number) =>
    invoke<OffloadJobStatus>("offload_status", { jobId }),
  cancelOffload: (jobId: number) =>
    invoke<OffloadJobStatus>("cancel_offload", { jobId }),
  listOffloadJobs: (limit = 20) =>
    invoke<OffloadJobStatus[]>("list_offload_jobs", { limit }),
  activeOffloadJob: () => invoke<OffloadJobStatus | null>("active_offload_job"),
  restoreOffloadItem: (itemId: number, targetDir: string | null = null) =>
    invoke<OffloadItemRecord>("restore_offload_item", { itemId, targetDir }),
  largestStaleFiles: (params: {
    runId: number | null
    minSize?: number
    minAgeDays?: number
    limit?: number
  }) =>
    invoke<FileSearchResult[]>("largest_stale_files", {
      runId: params.runId,
      minSize: params.minSize ?? 50 * 1024 * 1024,
      minAgeDays: params.minAgeDays ?? 180,
      limit: params.limit ?? 25,
    }),
  openUrl: (url: string) => invoke("open_url", { url }),
  aiKeyStatus: () => invoke<AiKeyStatus>("ai_key_status"),
  setAiApiKey: (apiKey: string) => invoke<AiKeyStatus>("set_ai_api_key", { apiKey }),
  clearAiApiKey: () => invoke<AiKeyStatus>("clear_ai_api_key"),
  getAiApiKey: () => invoke<string | null>("get_ai_api_key"),
  exportReport: (runId: number | null) =>
    invoke<ExportArtifact | null>("export_report", { runId }),
  exportDiagnostics: () => invoke<ExportArtifact>("export_diagnostics"),
  appHealth: () => invoke<AppHealth>("app_health"),
  // WinClean 2.0 optimizer (types in src/lib/optimizer.ts)
  optimizerListServices: () =>
    invoke<import("./optimizer").ServiceInfo[]>("optimizer_list_services"),
  optimizerSetServiceStartType: (
    name: string,
    displayName: string,
    startType: string
  ) =>
    invoke<import("./optimizer").ServiceInfo>(
      "optimizer_set_service_start_type",
      { name, displayName, startType }
    ),
  optimizerListProcesses: () =>
    invoke<import("./optimizer").ProcessInfo[]>("optimizer_list_processes"),
  optimizerSetProcessEcoQos: (pid: number, name: string, enable: boolean) =>
    invoke<void>("optimizer_set_process_ecoqos", { pid, name, enable }),
  optimizerBatteryStatus: () =>
    invoke<import("./optimizer").BatteryStatus>("optimizer_battery_status"),
  optimizerPowerHistory: (limit = 200) =>
    invoke<import("./optimizer").PowerSample[]>("optimizer_power_history", {
      limit,
    }),
  optimizerApplyPowerProfile: (profileId: string) =>
    invoke<string>("optimizer_apply_power_profile", { profileId }),
  optimizerHardwareInfo: () =>
    invoke<import("./optimizer").HardwareInfo>("optimizer_hardware_info"),
  optimizerCapabilities: () =>
    invoke<import("./optimizer").CapabilityReport>("optimizer_capabilities"),
  optimizerHistory: (limit = 100) =>
    invoke<import("./optimizer").OptimizerHistory>("optimizer_history", {
      limit,
    }),
  optimizerRollback: (snapshotId: number) =>
    invoke<string>("optimizer_rollback", { snapshotId }),
}

export function formatError(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}
