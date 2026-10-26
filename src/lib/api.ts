import { invoke } from "@tauri-apps/api/core"

import type * as O from "@/lib/optimizer"
import { describeError } from "@/lib/optimizer"
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

/** API-key providers whose key is stored in Windows Credential Manager. */
export type AiKeyProvider = "openai" | "openrouter"
export type AiKeyMap = Record<AiKeyProvider, boolean>

export type OpenCodeStatus = {
  installed: boolean
  version: string | null
  detail: string
}

export type StorageQuery = {
  mode?: "clean" | "offload" | "review"
  minSize?: number
  minAgeDays?: number
  nameContains?: string
  folder?: string
  extension?: string
  goalBytes?: number
  limit?: number
}

export type EvaluatedItem = {
  id: string
  path: string
  name: string
  size: number
  ageDays: number | null
  kind: "clean" | "offload" | "review"
  action: "recycleBin" | "googleDriveOffload" | "review"
  risk: "low" | "review"
  reason: string
  evidence: string[]
  duplicateGroup: string | null
  duplicateCopies: number | null
}

export type BlockedItem = {
  path: string
  name: string
  size: number
  reason: string
  detail: string
}

export type GoalSelection = {
  goalBytes: number
  selectedIds: string[]
  selectedBytes: number
  shortfallBytes: number
  note: string
}

export type StorageEvaluation = {
  scanRunId: number
  scanRoot: string
  scanAgeDays: number | null
  scanStale: boolean
  verified: EvaluatedItem[]
  manualReview: EvaluatedItem[]
  blocked: BlockedItem[]
  blockedTotal: number
  cleanBytes: number
  offloadBytes: number
  reviewBytes: number
  goal: GoalSelection | null
  warnings: string[]
}

export type ProposalInput = {
  title: string
  action: string
  candidateIds: string[]
  reason: string
  estimatedBytes?: number | null
}

export type ActionPreview = {
  summary: string
  fileCount: number
  totalBytes: number
  destination: string
  permanentDelete: boolean
  reversible: boolean
  requiresGoogleDrive: boolean
  steps: string[]
  notes: string[]
}

export type ValidatedProposal = {
  title: string
  reason: string
  action: "clean" | "offload" | "review" | null
  items: EvaluatedItem[]
  rejected: { candidateId: string; reason: string }[]
  verifiedBytes: number
  claimedBytes: number | null
  claimMatches: boolean
  preview: ActionPreview
}

export type DuplicateFile = {
  entryId: number
  candidateId: string
  path: string
  name: string
  size: number
  modifiedAtMs: number | null
}

export type DuplicateGroup = {
  status: "confirmed" | "possible"
  size: number
  fingerprint: string | null
  files: DuplicateFile[]
  reclaimableBytes: number
}

export type DuplicateReport = {
  scanRunId: number
  confirmed: DuplicateGroup[]
  possible: DuplicateGroup[]
  confirmedReclaimableBytes: number
  possibleReclaimableBytes: number
  protectedFiles: number
  notes: string[]
}

export type VerifySummary = {
  groupsExamined: number
  filesHashed: number
  bytesRead: number
  filesSkipped: number
  budgetExhausted: boolean
  cancelled: boolean
  stopped: string | null
}

export type DuplicateJobStatus = {
  state: "idle" | "running" | "completed" | "failed"
  runId: number | null
  filesHashed: number
  bytesRead: number
  summary: VerifySummary | null
  error: string | null
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
  /** Ask the native backend to reveal a path in Windows Explorer. */
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
  aiKeyStatus: (provider: AiKeyProvider = "openai") => invoke<AiKeyStatus>("ai_key_status", { provider }),
  aiKeyStatuses: () => invoke<AiKeyMap>("ai_key_statuses"),
  setAiApiKey: (apiKey: string, provider: AiKeyProvider = "openai") =>
    invoke<AiKeyStatus>("set_ai_api_key", { apiKey, provider }),
  clearAiApiKey: (provider: AiKeyProvider = "openai") => invoke<AiKeyStatus>("clear_ai_api_key", { provider }),
  getAiApiKey: (provider: AiKeyProvider = "openai") => invoke<string | null>("get_ai_api_key", { provider }),
  opencodeStatus: () => invoke<OpenCodeStatus>("opencode_status"),
  opencodeModels: () => invoke<string[]>("opencode_models"),
  opencodeRun: (model: string, instruction: string, data: string) =>
    invoke<string>("opencode_run", { model, instruction, data }),
  storageEvaluate: (runId: number | null, query?: StorageQuery) =>
    invoke<StorageEvaluation | null>("storage_evaluate", { runId, query: query ?? null }),
  storageValidateProposals: (runId: number, proposals: ProposalInput[]) =>
    invoke<ValidatedProposal[]>("storage_validate_proposals", { runId, proposals }),
  duplicateReport: (runId: number | null, minSize?: number) =>
    invoke<DuplicateReport | null>("duplicate_report_for", { runId, minSize: minSize ?? null }),
  duplicatesStart: (runId: number | null, options?: { minSize?: number; maxBytes?: number; maxFiles?: number }) =>
    invoke<DuplicateJobStatus>("duplicates_start", {
      runId,
      minSize: options?.minSize ?? null,
      maxBytes: options?.maxBytes ?? null,
      maxFiles: options?.maxFiles ?? null,
    }),
  duplicatesStatus: () => invoke<DuplicateJobStatus>("duplicates_status"),
  duplicatesCancel: () => invoke<DuplicateJobStatus>("duplicates_cancel"),
  storageExclusions: () => invoke<string[]>("storage_exclusions"),
  storageAddExclusion: (path: string) => invoke<string[]>("storage_add_exclusion", { path }),
  storageRemoveExclusion: (path: string) => invoke<string[]>("storage_remove_exclusion", { path }),
  exportReport: (runId: number | null) =>
    invoke<ExportArtifact | null>("export_report", { runId }),
  exportDiagnostics: () => invoke<ExportArtifact>("export_diagnostics"),
  appHealth: () => invoke<AppHealth>("app_health"),
  // WinClean 2.0 optimizer (types in src/lib/optimizer.ts)
  /** Service list with classification, dependents, trigger flag and real memory. */
  optimizerListServices: () => invoke<O.ServiceInfo[]>("optimizer_list_services"),
  /** Deterministic recommendations (never for protected services); nothing is applied. */
  optimizerServiceRecommendations: () =>
    invoke<O.ServiceRecommendation[]>("optimizer_service_recommendations"),
  /** Validate a plan with the backend policy and obtain a single-use approval token. */
  optimizerPrepareServicePlan: (changes: O.ServiceChange[], origin: "user" | "ai" = "user") =>
    invoke<O.PreparedPlanView>("optimizer_prepare_service_plan", { changes, origin }),
  /** Apply exactly the plan the token was issued for (all-or-nothing, verified, snapshotted). */
  optimizerApplyServicePlan: (token: string) =>
    invoke<O.ServiceApplyOutcome>("optimizer_apply_service_plan", { token }),
  /** Single-service change through the same transactional path as a plan of one. */
  optimizerSetServiceStartType: (name: string, startType: O.StartType) =>
    invoke<O.ServiceApplyOutcome>("optimizer_set_service_start_type", { name, startType }),
  optimizerHelperStatus: () => invoke<O.HelperStatus>("optimizer_helper_status"),
  /** Shows the UAC prompt and starts the administrator helper. */
  optimizerHelperConnect: (allowUntrustedLocation = false) =>
    invoke<O.HelperStatus>("optimizer_helper_connect", { allowUntrustedLocation }),
  optimizerHelperDisconnect: () => invoke<O.HelperStatus>("optimizer_helper_disconnect"),
  optimizerListProcesses: () => invoke<O.ProcessInfo[]>("optimizer_list_processes"),
  /** Audited EcoQoS change for a PID; false restores the system-managed default. */
  optimizerSetProcessEcoQos: (pid: number, name: string, enable: boolean) =>
    invoke<void>("optimizer_set_process_ecoqos", { pid, name, enable }),
  optimizerWorkload: () => invoke<O.WorkloadSnapshot>("optimizer_workload"),
  /** AC/DC, percentage and active scheme (legacy status call). */
  optimizerBatteryStatus: () => invoke<O.BatteryStatus>("optimizer_battery_status"),
  /** Real battery telemetry: watts, health, cycles, runtime. */
  optimizerBatteryTelemetry: () => invoke<O.TelemetryView>("optimizer_battery_telemetry"),
  optimizerTelemetryHistory: (limit = 240) =>
    invoke<O.TelemetrySample[]>("optimizer_telemetry_history", { limit }),
  optimizerPowerHistory: (limit = 200) =>
    invoke<O.PowerSample[]>("optimizer_power_history", { limit }),
  /** Keeps fast sampling alive while the Battery view is visible. */
  optimizerTelemetryLease: (active: boolean) =>
    invoke<void>("optimizer_telemetry_lease", { active }),
  optimizerListProfiles: () => invoke<O.ProfilesView>("optimizer_list_profiles"),
  optimizerApplyPowerProfile: (profileId: string) =>
    invoke<O.ProfileOutcome>("optimizer_apply_power_profile", { profileId }),
  optimizerSaveCustomProfile: (profile: O.PowerProfile) =>
    invoke<O.ProfilesView>("optimizer_save_custom_profile", { profile }),
  optimizerDeleteCustomProfile: (profileId: string) =>
    invoke<O.ProfilesView>("optimizer_delete_custom_profile", { profileId }),
  optimizerSetAutoSwitch: (config: O.AutoSwitchConfig) =>
    invoke<O.ProfilesView>("optimizer_set_auto_switch", { config }),
  optimizerMeasurementStart: (label: string, profileId: string) =>
    invoke<O.MeasurementView>("optimizer_measurement_start", { label, profileId }),
  optimizerMeasurementSwitch: (id: number) =>
    invoke<O.MeasurementView>("optimizer_measurement_switch", { id }),
  optimizerMeasurementFinish: (id: number) =>
    invoke<O.MeasurementView>("optimizer_measurement_finish", { id }),
  optimizerMeasurementList: () => invoke<O.MeasurementView[]>("optimizer_measurement_list"),
  /** Hardware discovery; never touches a GPU API (see docs/winclean-2.0.md). */
  optimizerHardwareInfo: (refresh = false) =>
    invoke<O.HardwareInfo>("optimizer_hardware_info", { refresh }),
  /** Windows ML execution-provider assessment; `modelPath` is an optional local .onnx file. */
  optimizerCapabilities: (modelPath?: string) =>
    invoke<O.CapabilityReport>("optimizer_capabilities", { modelPath: modelPath ?? null }),
  optimizerMcpStatus: (refresh = false) => invoke<O.McpStatus>("optimizer_mcp_status", { refresh }),
  optimizerMcpVerify: () => invoke<O.McpStatus>("optimizer_mcp_verify"),
  optimizerMcpCancel: () => invoke<void>("optimizer_mcp_cancel"),
  optimizerMcpSearchRoots: () => invoke<string[]>("optimizer_mcp_search_roots"),
  optimizerMcpAddSearchRoot: (path: string) => invoke<string[]>("optimizer_mcp_add_search_root", { path }),
  optimizerMcpRemoveSearchRoot: (path: string) => invoke<string[]>("optimizer_mcp_remove_search_root", { path }),
  optimizerMcpVerifyPermission: (folder: string) => invoke<O.McpStatus>("optimizer_mcp_verify_permission", { folder }),
  optimizerMcpSearch: (args: O.McpSearchArgs) => invoke<O.McpSearchOutcome>("optimizer_mcp_search", { args }),
  optimizerMcpFileDetails: (path: string) => invoke<unknown>("optimizer_mcp_file_details", { path }),
  optimizerHistory: (limit = 100) => invoke<O.OptimizerHistory>("optimizer_history", { limit }),
  optimizerRollback: (snapshotId: number) => invoke<string>("optimizer_rollback", { snapshotId }),
  optimizerRecoveryReport: () => invoke<O.RecoveryReport>("optimizer_recovery_report"),
  /** Native window integration info (Mica availability). */
  nativeInfo: () => invoke<NativeInfo>("native_info"),
}

export type NativeInfo = { windowsBuild: number | null; mica: boolean }

/** Readable text for a command error: a thrown Error, a `{code, message}` object, or a string. */
export function formatError(error: unknown) {
  return describeError(error)
}
