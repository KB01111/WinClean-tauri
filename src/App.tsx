import {
  AlertTriangle,
  BarChart3,
  Cloud,
  Database,
  Download,
  FileSearch,
  FolderOpen,
  FolderTree,
  HardDrive,
  History,
  Loader2,
  Play,
  RefreshCw,
  ChevronRight,
  ChevronUp,
  Search,
  Settings,
  ShieldCheck,
  Square,
  Trash2,
  UploadCloud,
} from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"

import { Button } from "@/components/ui/button"
import {
  cleanupActionDisabled,
  directoryBreadcrumbs,
  parentDirectoryPath,
  pruneCleanupSelection,
  shouldCommitRunDataResponse,
} from "@/lib/app-state"
import {
  formatBytes,
  formatDateTime,
  formatNumber,
  formatPercentUsed,
  statusText,
} from "@/lib/format"
import { selectDashboardRun } from "@/lib/scan-selection"
import {
  runDataRefreshKey,
  shouldRefreshDashboardCollectionsForProgress,
  shouldRefreshRunDataForProgress,
} from "@/lib/scan-refresh"
import { invokeWinClean } from "@/lib/tauri-api"
import {
  getVisibleRecoverCandidates,
  getOffloadActionState,
  getOffloadProgress,
  selectedVisibleCandidateIds,
  summarizeRecoverCandidates,
  visibleRecoverCandidateWindow,
  type RecoverMode,
  type RecoverCandidateSummary,
} from "@/lib/recover-space"
import { selectDefaultScanRoot } from "@/lib/volume-selection"

type View = "overview" | "explorer" | "cleanup" | "history" | "settings"
type ScanMode = "ntfsUsn" | "recursive"
type ScanState = "idle" | "running" | "completed" | "failed" | "canceled"

type VolumeInfo = {
  name: string
  rootPath: string
  fileSystem?: string | null
  driveType: string
  isReady: boolean
  availableSpace?: number | null
  totalSpace?: number | null
}

type ScanProgress = {
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

type ScanRunRecord = {
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

type TopDirectory = {
  path: string
  totalSize: number
  fileCount: number
  directoryCount: number
}

type DirectoryChild = {
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

type FileSearchResult = {
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

type ExtensionSummary = {
  extension?: string | null
  fileCount: number
  totalSize: number
}

type CategorySummary = {
  category: string
  fileCount: number
  totalSize: number
}

type CleanupCandidate = {
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

type CleanupPlan = {
  scanRunId: number
  candidates: CleanupCandidate[]
  totalBytes: number
  warnings: string[]
}

type RecommendationCandidate = {
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

type RecommendationPlan = {
  scanRunId: number
  candidates: RecommendationCandidate[]
  totalCandidateCount: number
  cleanBytes: number
  offloadBytes: number
  reviewBytes: number
  warnings: string[]
}

type CleanupActionRecord = {
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

type AppHealth = {
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

type GoogleDriveStatus = {
  configured: boolean
  connected: boolean
  accountEmail?: string | null
  error?: string | null
}

type OffloadItemRecord = {
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

type OffloadJobStatus = {
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

type ExportArtifact = {
  path: string
  createdAtMs: number
  bytes: number
}

const emptyProgress: ScanProgress = {
  state: "idle",
  filesIndexed: 0,
  directoriesIndexed: 0,
  bytesIndexed: 0,
  accessDeniedCount: 0,
  skippedReparseCount: 0,
  errorCount: 0,
}
const MAX_RECOVER_CANDIDATE_ROWS = 300

function App() {
  const [view, setView] = useState<View>("overview")
  const [volumes, setVolumes] = useState<VolumeInfo[]>([])
  const [selectedRoot, setSelectedRoot] = useState("")
  const [progress, setProgress] = useState<ScanProgress>(emptyProgress)
  const [scanRuns, setScanRuns] = useState<ScanRunRecord[]>([])
  const [selectedRunId, setSelectedRunId] = useState<number | null>(null)
  const [topDirectories, setTopDirectories] = useState<TopDirectory[]>([])
  const [directoryChildren, setDirectoryChildren] = useState<DirectoryChild[]>(
    []
  )
  const [currentDirectory, setCurrentDirectory] = useState<string | null>(null)
  const [files, setFiles] = useState<FileSearchResult[]>([])
  const [largestFiles, setLargestFiles] = useState<FileSearchResult[]>([])
  const [fileQuery, setFileQuery] = useState("")
  const [extensionFilter, setExtensionFilter] = useState("")
  const [minSizeMbFilter, setMinSizeMbFilter] = useState("")
  const [extensions, setExtensions] = useState<ExtensionSummary[]>([])
  const [categories, setCategories] = useState<CategorySummary[]>([])
  const [cleanupPlan, setCleanupPlan] = useState<CleanupPlan | null>(null)
  const [recommendationPlan, setRecommendationPlan] =
    useState<RecommendationPlan | null>(null)
  const [recoverMode, setRecoverMode] = useState<RecoverMode>("clean")
  const [selectedCleanupIds, setSelectedCleanupIds] = useState<Set<string>>(
    new Set()
  )
  const [cleanupHistory, setCleanupHistory] = useState<CleanupActionRecord[]>(
    []
  )
  const [driveStatus, setDriveStatus] = useState<GoogleDriveStatus | null>(null)
  const [activeOffloadJob, setActiveOffloadJob] =
    useState<OffloadJobStatus | null>(null)
  const [health, setHealth] = useState<AppHealth | null>(null)
  const [message, setMessage] = useState<string | null>(null)
  const [loadingVolumes, setLoadingVolumes] = useState(true)
  const [loadingData, setLoadingData] = useState(false)
  const [loadingRunData, setLoadingRunData] = useState(false)
  const [scanPending, setScanPending] = useState(false)
  const [cancelPending, setCancelPending] = useState(false)
  const [cleanupPending, setCleanupPending] = useState(false)
  const [reportPending, setReportPending] = useState(false)
  const [diagnosticsPending, setDiagnosticsPending] = useState(false)
  const [historyPending, setHistoryPending] = useState(false)
  const dashboardRefreshInFlight = useRef(false)
  const scanStatusRefreshInFlight = useRef(false)
  const initialLoadStarted = useRef(false)
  const lastRunDataRefreshKey = useRef<string | null>(null)
  const runDataRequestId = useRef(0)
  const cleanupPlanRunId = useRef<number | null>(null)

  const selectedVolume = useMemo(
    () => volumes.find((volume) => volume.rootPath === selectedRoot),
    [selectedRoot, volumes]
  )
  const activeRun = useMemo(
    () =>
      scanRuns.find((run) => run.id === selectedRunId) ??
      scanRuns.find((run) => run.status === "completed") ??
      null,
    [scanRuns, selectedRunId]
  )
  const isScanning = progress.state === "running"
  const executableCleanIds = useMemo(
    () =>
      new Set(cleanupPlan?.candidates.map((candidate) => candidate.id) ?? []),
    [cleanupPlan]
  )
  const visibleRecommendations = useMemo(() => {
    if (!recommendationPlan) {
      return []
    }
    return getVisibleRecoverCandidates(
      recommendationPlan.candidates,
      recoverMode,
      executableCleanIds
    )
  }, [executableCleanIds, recommendationPlan, recoverMode])
  const recoverSummary = useMemo(
    () => summarizeRecoverCandidates(recommendationPlan?.candidates ?? []),
    [recommendationPlan]
  )
  const recoverCandidateWindow = useMemo(
    () =>
      visibleRecoverCandidateWindow(
        visibleRecommendations,
        MAX_RECOVER_CANDIDATE_ROWS
      ),
    [visibleRecommendations]
  )
  const selectedRecommendationBytes = useMemo(() => {
    return visibleRecommendations
      .filter((candidate) => selectedCleanupIds.has(candidate.id))
      .reduce((sum, candidate) => sum + candidate.size, 0)
  }, [selectedCleanupIds, visibleRecommendations])
  const selectedVisibleCount = useMemo(
    () =>
      visibleRecommendations.filter((candidate) =>
        selectedCleanupIds.has(candidate.id)
      ).length,
    [selectedCleanupIds, visibleRecommendations]
  )
  const offloadRunning =
    activeOffloadJob?.status === "queued" ||
    activeOffloadJob?.status === "running"
  const offloadActionState = getOffloadActionState({
    selectedCount: selectedVisibleCount,
    driveStatus,
    running: offloadRunning,
  })
  const explorerBreadcrumbs = useMemo(
    () => directoryBreadcrumbs(activeRun?.rootPath, currentDirectory),
    [activeRun?.rootPath, currentDirectory]
  )
  const explorerParentDirectory = useMemo(
    () => parentDirectoryPath(currentDirectory, activeRun?.rootPath),
    [activeRun?.rootPath, currentDirectory]
  )

  const refreshRunData = useCallback(
    async (
      runId?: number | null,
      parentPath?: string | null,
      options?: { clearMessage?: boolean }
    ) => {
      const requestId = ++runDataRequestId.current
      const resolvedRunId = runId ?? selectedRunId ?? null
      setLoadingRunData(true)
      if (!resolvedRunId) {
        setTopDirectories([])
        setDirectoryChildren([])
        setFiles([])
        setLargestFiles([])
        setExtensions([])
        setCategories([])
        setCleanupPlan(null)
        setRecommendationPlan(null)
        cleanupPlanRunId.current = null
        setSelectedCleanupIds(new Set())
        setLoadingRunData(false)
        return true
      }
      const minSizeMb = Number(minSizeMbFilter)
      const minSize =
        Number.isFinite(minSizeMb) && minSizeMb > 0
          ? minSizeMb * 1024 * 1024
          : null

      try {
        const [
          top,
          children,
          foundFiles,
          topFiles,
          extensionRows,
          categoryRows,
          plan,
          recommendations,
        ] = await Promise.all([
          invokeWinClean<TopDirectory[]>("top_directories", {
            runId: resolvedRunId,
            limit: 25,
          }),
          invokeWinClean<DirectoryChild[]>("directory_children", {
            runId: resolvedRunId,
            parentPath: parentPath ?? null,
            limit: 250,
          }),
          invokeWinClean<FileSearchResult[]>("file_search", {
            runId: resolvedRunId,
            query: fileQuery || null,
            extension: extensionFilter || null,
            minSize,
            limit: 100,
          }),
          invokeWinClean<FileSearchResult[]>("file_search", {
            runId: resolvedRunId,
            query: null,
            extension: null,
            minSize: null,
            limit: 25,
          }),
          invokeWinClean<ExtensionSummary[]>("extension_summary", {
            runId: resolvedRunId,
            limit: 20,
          }),
          invokeWinClean<CategorySummary[]>("category_summary", {
            runId: resolvedRunId,
          }),
          invokeWinClean<CleanupPlan | null>("cleanup_plan", {
            runId: resolvedRunId,
            limit: 100,
          }),
          invokeWinClean<RecommendationPlan | null>("recommendations", {
            runId: resolvedRunId,
            filters: null,
          }),
        ])
        if (
          !shouldCommitRunDataResponse({
            requestId,
            latestRequestId: runDataRequestId.current,
          })
        ) {
          return false
        }

        setTopDirectories(top)
        setDirectoryChildren(children)
        setFiles(foundFiles)
        setLargestFiles(topFiles)
        setExtensions(extensionRows)
        setCategories(categoryRows)
        const previousPlanRunId = cleanupPlanRunId.current
        const nextPlanRunId =
          recommendations?.scanRunId ?? plan?.scanRunId ?? null
        cleanupPlanRunId.current = nextPlanRunId
        setCleanupPlan(plan)
        setRecommendationPlan(recommendations)
        setSelectedCleanupIds((current) =>
          previousPlanRunId === nextPlanRunId
            ? pruneCleanupSelection(
                current,
                (recommendations?.candidates ?? plan?.candidates ?? []).map(
                  (candidate) => candidate.id
                )
              )
            : new Set()
        )
        if (options?.clearMessage !== false) {
          setMessage(null)
        }
        return true
      } catch (error) {
        if (
          shouldCommitRunDataResponse({
            requestId,
            latestRequestId: runDataRequestId.current,
          })
        ) {
          setMessage(formatError(error))
        }
        return false
      } finally {
        if (
          shouldCommitRunDataResponse({
            requestId,
            latestRequestId: runDataRequestId.current,
          })
        ) {
          setLoadingRunData(false)
        }
      }
    },
    [extensionFilter, fileQuery, minSizeMbFilter, selectedRunId]
  )

  const refreshDashboard = useCallback(
    async (options?: { clearMessage?: boolean }) => {
      if (dashboardRefreshInFlight.current) {
        return
      }
      dashboardRefreshInFlight.current = true
      setLoadingData(true)
      try {
        const [nextProgress, runs, historyRows, nextHealth, nextDriveStatus] =
          await Promise.all([
            invokeWinClean<ScanProgress>("scan_status"),
            invokeWinClean<ScanRunRecord[]>("scan_runs", { limit: 30 }),
            invokeWinClean<CleanupActionRecord[]>("cleanup_history", { limit: 50 }),
            invokeWinClean<AppHealth>("app_health"),
            invokeWinClean<GoogleDriveStatus>("google_drive_status"),
          ])
        setProgress(nextProgress)
        setScanRuns(runs)
        setCleanupHistory(historyRows)
        setHealth(nextHealth)
        setDriveStatus(nextDriveStatus)
        const selection = selectDashboardRun({
          runs,
          selectedRunId,
          progressState: nextProgress.state,
          progressRunId: nextProgress.runId ?? null,
          progressRootPath: nextProgress.rootPath ?? null,
          currentDirectory,
        })
        const nextRunId = selection.runId
        const nextDirectory = selection.directory
        setSelectedRunId(nextRunId)
        setCurrentDirectory(nextDirectory)
        let runDataRefreshSucceeded = true
        if (shouldRefreshRunDataForProgress(nextProgress)) {
          lastRunDataRefreshKey.current = runDataRefreshKey(
            nextRunId,
            nextDirectory
          )
          runDataRefreshSucceeded = await refreshRunData(
            nextRunId,
            nextDirectory,
            { clearMessage: options?.clearMessage }
          )
        }
        if (runDataRefreshSucceeded && options?.clearMessage !== false) {
          setMessage(null)
        }
      } catch (error) {
        lastRunDataRefreshKey.current = null
        setMessage(formatError(error))
      } finally {
        dashboardRefreshInFlight.current = false
        setLoadingData(false)
      }
    },
    [currentDirectory, refreshRunData, selectedRunId]
  )

  const refreshScanStatus = useCallback(async () => {
    if (scanStatusRefreshInFlight.current) {
      return
    }
    scanStatusRefreshInFlight.current = true
    try {
      const nextProgress = await invokeWinClean<ScanProgress>("scan_status")
      setProgress(nextProgress)
      if (shouldRefreshDashboardCollectionsForProgress(nextProgress)) {
        await refreshDashboard()
      }
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      scanStatusRefreshInFlight.current = false
    }
  }, [refreshDashboard])

  const refreshVolumes = useCallback(async () => {
    setLoadingVolumes(true)
    try {
      const nextVolumes = await invokeWinClean<VolumeInfo[]>("list_volumes")
      setVolumes(nextVolumes)
      setSelectedRoot((current) => selectDefaultScanRoot(nextVolumes, current))
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setLoadingVolumes(false)
    }
  }, [])

  useEffect(() => {
    if (initialLoadStarted.current) {
      return
    }
    initialLoadStarted.current = true
    void refreshVolumes()
    void refreshDashboard()
  }, [refreshDashboard, refreshVolumes])

  useEffect(() => {
    if (!isScanning) {
      return
    }
    const timer = window.setInterval(() => {
      void refreshScanStatus()
    }, 900)
    return () => window.clearInterval(timer)
  }, [isScanning, refreshScanStatus])

  useEffect(() => {
    if (!activeOffloadJob || !offloadRunning) {
      return
    }
    const timer = window.setInterval(async () => {
      try {
        const nextJob = await invokeWinClean<OffloadJobStatus>("offload_status", {
          jobId: activeOffloadJob.id,
        })
        setActiveOffloadJob(nextJob)
        if (nextJob.status !== "queued" && nextJob.status !== "running") {
          await refreshDashboard()
        }
      } catch (error) {
        setMessage(formatError(error))
      }
    }, 1200)
    return () => window.clearInterval(timer)
  }, [activeOffloadJob, offloadRunning, refreshDashboard])

  useEffect(() => {
    if (!activeRun || isScanning) {
      return
    }
    const key = runDataRefreshKey(activeRun.id, currentDirectory)
    if (lastRunDataRefreshKey.current === key) {
      lastRunDataRefreshKey.current = null
      return
    }
    const timeout = window.setTimeout(() => {
      void refreshRunData(activeRun.id, currentDirectory ?? activeRun.rootPath)
    }, 200)
    return () => window.clearTimeout(timeout)
  }, [
    activeRun,
    currentDirectory,
    extensionFilter,
    fileQuery,
    isScanning,
    refreshRunData,
  ])

  async function startScan() {
    if (!selectedVolume || scanPending) {
      return
    }

    setScanPending(true)
    setMessage(null)
    setTopDirectories([])
    setDirectoryChildren([])
    setFiles([])
    setCleanupPlan(null)
    setRecommendationPlan(null)
    setSelectedCleanupIds(new Set())
    setSelectedRunId(null)
    setCurrentDirectory(selectedVolume.rootPath)
    try {
      const nextProgress = await invokeWinClean<ScanProgress>("start_scan", {
        rootPath: selectedVolume.rootPath,
        fileSystem: selectedVolume.fileSystem ?? null,
      })
      setProgress(nextProgress)
      setSelectedRunId(nextProgress.runId ?? null)
      setCurrentDirectory(nextProgress.rootPath ?? selectedVolume.rootPath)
      setView("overview")
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setScanPending(false)
    }
  }

  async function cancelScan() {
    if (cancelPending) {
      return
    }
    setCancelPending(true)
    try {
      setProgress(await invokeWinClean<ScanProgress>("cancel_scan"))
      await refreshDashboard()
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setCancelPending(false)
    }
  }

  async function selectRun(run: ScanRunRecord) {
    setSelectedRunId(run.id)
    setCurrentDirectory(run.rootPath)
    await refreshRunData(run.id, run.rootPath)
  }

  async function openDirectory(path: string) {
    setCurrentDirectory(path)
    await refreshRunData(activeRun?.id, path)
  }

  function toggleCleanupCandidate(id: string) {
    setSelectedCleanupIds((current) => {
      const next = new Set(current)
      if (next.has(id)) {
        next.delete(id)
      } else {
        next.add(id)
      }
      return next
    })
  }

  async function executeSelectedCleanup() {
    const runId = recommendationPlan?.scanRunId ?? cleanupPlan?.scanRunId
    const candidateIds = selectedVisibleCandidateIds(
      visibleRecommendations,
      selectedCleanupIds
    )
    if (!runId || candidateIds.length === 0 || cleanupPending) {
      return
    }
    setCleanupPending(true)
    try {
      const result = await invokeWinClean<{
        recycledCount: number
        failedCount: number
        skippedCount: number
        bytesRequested: number
      }>("execute_cleanup", {
        runId,
        candidateIds,
      })
      setMessage(
        `Cleanup sent ${result.recycledCount} item(s) to Recycle Bin; ${result.skippedCount} skipped; ${result.failedCount} failed.`
      )
      await refreshDashboard({ clearMessage: false })
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setCleanupPending(false)
    }
  }

  async function connectGoogleDrive() {
    try {
      setMessage("Opening Google Drive sign-in...")
      const status = await invokeWinClean<GoogleDriveStatus>("begin_google_drive_auth")
      setDriveStatus(status)
      setMessage(
        status.connected
          ? "Google Drive connected."
          : (status.error ?? "Google Drive is not connected.")
      )
      await refreshDashboard()
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  async function disconnectGoogleDrive() {
    try {
      const status = await invokeWinClean<GoogleDriveStatus>("disconnect_google_drive")
      setDriveStatus(status)
      setMessage("Google Drive disconnected.")
      await refreshDashboard()
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  async function startSelectedOffload() {
    if (!recommendationPlan) {
      return
    }
    const candidateIds = selectedVisibleCandidateIds(
      visibleRecommendations,
      selectedCleanupIds
    )
    if (candidateIds.length === 0) {
      return
    }
    try {
      const job = await invokeWinClean<OffloadJobStatus>("start_offload", {
        runId: recommendationPlan.scanRunId,
        candidateIds,
      })
      setActiveOffloadJob(job)
      setMessage(`Offload batch ${job.id} queued for Google Drive.`)
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  async function cancelActiveOffload() {
    if (!activeOffloadJob) {
      return
    }
    try {
      const job = await invokeWinClean<OffloadJobStatus>("cancel_offload", {
        jobId: activeOffloadJob.id,
      })
      setActiveOffloadJob(job)
      setMessage("Offload cancel requested.")
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  async function openRecycleBin() {
    try {
      await invokeWinClean("open_recycle_bin")
      setMessage("Windows Recycle Bin opened.")
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  async function exportReport() {
    if (reportPending) {
      return
    }
    setReportPending(true)
    try {
      const artifact = await invokeWinClean<ExportArtifact | null>("export_report", {
        runId: activeRun?.id ?? null,
      })
      if (artifact) {
        setMessage(`Report exported to ${artifact.path}`)
      }
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setReportPending(false)
    }
  }

  async function exportDiagnostics() {
    if (diagnosticsPending) {
      return
    }
    setDiagnosticsPending(true)
    try {
      const artifact = await invokeWinClean<ExportArtifact>("export_diagnostics")
      setMessage(`Diagnostics exported to ${artifact.path}`)
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setDiagnosticsPending(false)
    }
  }

  async function deleteRun(runId: number) {
    if (historyPending) {
      return
    }
    setHistoryPending(true)
    try {
      const deleted = await invokeWinClean<boolean>("delete_scan_run", { runId })
      if (deleted) {
        if (selectedRunId === runId) {
          setSelectedRunId(null)
          setCurrentDirectory(null)
          lastRunDataRefreshKey.current = null
        }
        setMessage("Scan history entry deleted.")
      }
      await refreshDashboard({ clearMessage: false })
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setHistoryPending(false)
    }
  }

  async function clearScanHistory() {
    if (historyPending) {
      return
    }
    setHistoryPending(true)
    try {
      const deletedCount = await invokeWinClean<number>("clear_scan_history")
      setSelectedRunId(null)
      setCurrentDirectory(null)
      lastRunDataRefreshKey.current = null
      setMessage(`Cleared ${formatNumber(deletedCount)} completed scan(s).`)
      await refreshDashboard({ clearMessage: false })
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setHistoryPending(false)
    }
  }

  return (
    <main className="min-h-svh bg-[#f6f7f4] text-[#20231f]">
      <div className="grid min-h-svh grid-cols-[260px_1fr]">
        <aside className="border-r border-[#d8ddd2] bg-[#fbfcf8] p-4">
          <div className="mb-6 flex items-center gap-3">
            <div className="flex size-10 items-center justify-center rounded-md bg-[#123c2f] text-white">
              <HardDrive className="size-5" aria-hidden="true" />
            </div>
            <div>
              <h1 className="text-xl font-semibold tracking-normal">
                WinClean
              </h1>
              <p className="text-xs text-[#657064]">Local disk analyzer</p>
            </div>
          </div>

          <nav className="space-y-1">
            <NavButton
              icon={BarChart3}
              label="Overview"
              active={view === "overview"}
              onClick={() => setView("overview")}
            />
            <NavButton
              icon={FolderTree}
              label="Explorer"
              active={view === "explorer"}
              onClick={() => setView("explorer")}
            />
            <NavButton
              icon={Trash2}
              label="Cleanup"
              active={view === "cleanup"}
              onClick={() => setView("cleanup")}
            />
            <NavButton
              icon={History}
              label="History"
              active={view === "history"}
              onClick={() => setView("history")}
            />
            <NavButton
              icon={Settings}
              label="Settings"
              active={view === "settings"}
              onClick={() => setView("settings")}
            />
          </nav>

          <div className="mt-6 rounded-lg border border-[#d8ddd2] bg-white p-3">
            <div className="mb-2 flex items-center justify-between">
              <span className="text-xs font-semibold text-[#596259] uppercase">
                Scan target
              </span>
              {loadingVolumes ? (
                <Loader2 className="size-4 animate-spin text-[#487968]" />
              ) : null}
            </div>
            <div className="space-y-2">
              {volumes.map((volume) => (
                <button
                  type="button"
                  key={volume.rootPath}
                  onClick={() => setSelectedRoot(volume.rootPath)}
                  className={[
                    "flex w-full items-center gap-2 rounded-md border p-2 text-left transition",
                    selectedRoot === volume.rootPath
                      ? "border-[#2d6654] bg-[#edf6ef]"
                      : "border-[#e1e5dd] bg-white hover:border-[#b9c8bd]",
                  ].join(" ")}
                >
                  <HardDrive
                    className="size-4 shrink-0 text-[#2d6654]"
                    aria-hidden="true"
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">
                      {volume.rootPath}
                    </span>
                    <span className="block truncate text-xs text-[#667066]">
                      {volume.fileSystem ?? "unknown"} ·{" "}
                      {formatPercentUsed(volume)}
                    </span>
                  </span>
                </button>
              ))}
            </div>
          </div>
        </aside>

        <section className="min-w-0 p-5">
          <header className="mb-5 flex flex-wrap items-start justify-between gap-3 border-b border-[#d8ddd2] pb-4">
            <div>
              <h2 className="text-2xl font-semibold tracking-normal">
                {viewTitle(view)}
              </h2>
              <p className="text-sm text-[#667066]">
                {statusText(progress)}
                {progress.workerCount && progress.phase
                  ? ` · ${progress.phase} on ${formatNumber(progress.workerCount)} workers`
                  : ""}
              </p>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="outline"
                onClick={() => void refreshDashboard()}
                disabled={loadingData || loadingRunData}
              >
                <RefreshCw className="size-4" aria-hidden="true" />
                Refresh
              </Button>
              {isScanning ? (
                <Button
                  variant="destructive"
                  onClick={cancelScan}
                  disabled={cancelPending}
                >
                  {cancelPending ? (
                    <Loader2
                      className="size-4 animate-spin"
                      aria-hidden="true"
                    />
                  ) : (
                    <Square className="size-4" aria-hidden="true" />
                  )}
                  {cancelPending ? "Canceling" : "Cancel"}
                </Button>
              ) : (
                <Button
                  onClick={startScan}
                  disabled={
                    !selectedVolume || !selectedVolume.isReady || scanPending
                  }
                >
                  {scanPending ? (
                    <Loader2
                      className="size-4 animate-spin"
                      aria-hidden="true"
                    />
                  ) : (
                    <Play className="size-4" aria-hidden="true" />
                  )}
                  {scanPending ? "Starting" : "Scan"}
                </Button>
              )}
            </div>
          </header>

          {(progress.fallbackReason || message || progress.lastError) && (
            <Notice
              tone={progress.lastError ? "danger" : "warning"}
              text={[progress.fallbackReason, message, progress.lastError]
                .filter(Boolean)
                .join(" ")}
            />
          )}

          {view === "overview" && (
            <OverviewView
              progress={progress}
              scanRuns={scanRuns}
              activeRun={activeRun}
              topDirectories={topDirectories}
              topFiles={largestFiles}
              categories={categories}
              extensions={extensions}
              recommendationPlan={recommendationPlan}
              onSelectRun={selectRun}
              onExportReport={exportReport}
              reportPending={reportPending}
            />
          )}
          {view === "explorer" && (
            <ExplorerView
              activeRun={activeRun}
              currentDirectory={currentDirectory}
              breadcrumbs={explorerBreadcrumbs}
              parentDirectory={explorerParentDirectory}
              directoryChildren={directoryChildren}
              files={files}
              fileQuery={fileQuery}
              extensionFilter={extensionFilter}
              loadingRunData={loadingRunData}
              minSizeMbFilter={minSizeMbFilter}
              onFileQueryChange={setFileQuery}
              onExtensionFilterChange={setExtensionFilter}
              onMinSizeMbFilterChange={setMinSizeMbFilter}
              onOpenDirectory={openDirectory}
            />
          )}
          {view === "cleanup" && (
            <RecoverSpaceView
              recommendationPlan={recommendationPlan}
              recoverSummary={recoverSummary}
              visibleRecommendations={recoverCandidateWindow.rows}
              hiddenCandidateCount={recoverCandidateWindow.hiddenCount}
              recoverMode={recoverMode}
              onRecoverModeChange={(mode) => {
                setRecoverMode(mode)
                setSelectedCleanupIds(new Set())
              }}
              selectedCleanupIds={selectedCleanupIds}
              selectedRecommendationBytes={selectedRecommendationBytes}
              topDirectories={topDirectories}
              topFiles={largestFiles}
              categories={categories}
              extensions={extensions}
              cleanupPending={cleanupPending}
              isScanning={isScanning}
              driveStatus={driveStatus}
              offloadActionState={offloadActionState}
              activeOffloadJob={activeOffloadJob}
              onToggleCandidate={toggleCleanupCandidate}
              onClean={executeSelectedCleanup}
              onOffload={startSelectedOffload}
              onConnectDrive={connectGoogleDrive}
              onDisconnectDrive={disconnectGoogleDrive}
              onCancelOffload={cancelActiveOffload}
            />
          )}
          {view === "history" && (
            <HistoryView
              cleanupHistory={cleanupHistory}
              scanRuns={scanRuns}
              historyPending={historyPending}
              onOpenRecycleBin={openRecycleBin}
              onDeleteRun={deleteRun}
              onClearScanHistory={clearScanHistory}
            />
          )}
          {view === "settings" && (
            <SettingsView
              health={health}
              onExportDiagnostics={exportDiagnostics}
              diagnosticsPending={diagnosticsPending}
            />
          )}
        </section>
      </div>
    </main>
  )
}

function OverviewView({
  progress,
  scanRuns,
  activeRun,
  topDirectories,
  topFiles,
  categories,
  extensions,
  recommendationPlan,
  onSelectRun,
  onExportReport,
  reportPending,
}: {
  progress: ScanProgress
  scanRuns: ScanRunRecord[]
  activeRun: ScanRunRecord | null
  topDirectories: TopDirectory[]
  topFiles: FileSearchResult[]
  categories: CategorySummary[]
  extensions: ExtensionSummary[]
  recommendationPlan: RecommendationPlan | null
  onSelectRun: (run: ScanRunRecord) => void
  onExportReport: () => void
  reportPending: boolean
}) {
  return (
    <div className="grid gap-4 xl:grid-cols-[1fr_360px]">
      <div className="space-y-4">
        <div className="grid gap-3 md:grid-cols-4">
          <Metric
            label="Files"
            value={formatNumber(
              progress.filesIndexed || activeRun?.filesIndexed || 0
            )}
          />
          <Metric
            label="Folders"
            value={formatNumber(
              progress.directoriesIndexed || activeRun?.directoriesIndexed || 0
            )}
          />
          <Metric
            label="Indexed"
            value={formatBytes(
              progress.bytesIndexed || activeRun?.bytesIndexed || 0
            )}
          />
          <Metric
            label="Denied"
            value={formatNumber(
              progress.accessDeniedCount || activeRun?.accessDeniedCount || 0
            )}
          />
          <Metric
            label="Recoverable"
            value={formatBytes(
              (recommendationPlan?.cleanBytes ?? 0) +
                (recommendationPlan?.offloadBytes ?? 0)
            )}
          />
        </div>

        <Panel
          title="Largest directories"
          action={
            <Button
              variant="outline"
              onClick={onExportReport}
              disabled={!activeRun || reportPending}
            >
              {reportPending ? (
                <Loader2 className="size-4 animate-spin" aria-hidden="true" />
              ) : (
                <Download className="size-4" aria-hidden="true" />
              )}
              {reportPending ? "Exporting" : "Export"}
            </Button>
          }
        >
          <DataTable
            headers={["Path", "Size", "Files", "Folders"]}
            empty="No directory data yet"
            rows={topDirectories.map((directory) => [
              directory.path,
              formatBytes(directory.totalSize),
              formatNumber(directory.fileCount),
              formatNumber(directory.directoryCount),
            ])}
          />
        </Panel>

        <Panel title="Largest files">
          <DataTable
            headers={["File", "Size", "Modified"]}
            empty="No file data yet"
            rows={topFiles
              .slice(0, 10)
              .map((file) => [
                file.path,
                formatBytes(file.size),
                formatDateTime(file.modifiedAtMs),
              ])}
          />
        </Panel>
      </div>

      <div className="space-y-4">
        <Panel title="Scan history">
          <div className="space-y-2">
            {scanRuns.length === 0 ? (
              <EmptyState text="No scans recorded yet" />
            ) : (
              scanRuns.map((run) => (
                <button
                  key={run.id}
                  type="button"
                  onClick={() => void onSelectRun(run)}
                  className={[
                    "w-full rounded-md border p-3 text-left transition",
                    activeRun?.id === run.id
                      ? "border-[#2d6654] bg-[#edf6ef]"
                      : "border-[#e1e5dd] bg-white hover:border-[#b9c8bd]",
                  ].join(" ")}
                >
                  <div className="flex items-center justify-between gap-3">
                    <span className="truncate text-sm font-semibold">
                      {run.rootPath}
                    </span>
                    <span className="text-xs text-[#667066] uppercase">
                      {run.status}
                    </span>
                  </div>
                  <div className="mt-1 text-xs text-[#667066]">
                    {formatBytes(run.bytesIndexed)} ·{" "}
                    {formatDateTime(run.finishedAtMs)}
                  </div>
                </button>
              ))
            )}
          </div>
        </Panel>

        <Panel title="File mix">
          <SummaryList
            rows={categories.map((row) => [
              row.category,
              formatBytes(row.totalSize),
            ])}
          />
        </Panel>

        <Panel title="Top extensions">
          <SummaryList
            rows={extensions
              .slice(0, 8)
              .map((row) => [
                row.extension ? `.${row.extension}` : "No extension",
                formatBytes(row.totalSize),
              ])}
          />
        </Panel>
      </div>
    </div>
  )
}

function ExplorerView({
  activeRun,
  currentDirectory,
  breadcrumbs,
  parentDirectory,
  directoryChildren,
  files,
  fileQuery,
  extensionFilter,
  loadingRunData,
  minSizeMbFilter,
  onFileQueryChange,
  onExtensionFilterChange,
  onMinSizeMbFilterChange,
  onOpenDirectory,
}: {
  activeRun: ScanRunRecord | null
  currentDirectory: string | null
  breadcrumbs: { label: string; path: string }[]
  parentDirectory: string | null
  directoryChildren: DirectoryChild[]
  files: FileSearchResult[]
  fileQuery: string
  extensionFilter: string
  loadingRunData: boolean
  minSizeMbFilter: string
  onFileQueryChange: (value: string) => void
  onExtensionFilterChange: (value: string) => void
  onMinSizeMbFilterChange: (value: string) => void
  onOpenDirectory: (path: string) => void
}) {
  return (
    <div className="grid gap-4 xl:grid-cols-[1fr_420px]">
      <Panel
        title="Directory browser"
        action={
          <Button
            variant="outline"
            size="sm"
            disabled={!parentDirectory || loadingRunData}
            onClick={() => parentDirectory && onOpenDirectory(parentDirectory)}
          >
            <ChevronUp className="size-4" aria-hidden="true" />
            Up
          </Button>
        }
      >
        <div className="mb-3 space-y-2 rounded-md bg-[#f3f5ef] px-3 py-2 text-sm text-[#596259]">
          <div className="flex items-center gap-2">
            <FolderOpen className="size-4" aria-hidden="true" />
            <span className="truncate">
              {currentDirectory ?? activeRun?.rootPath ?? "No scan selected"}
            </span>
          </div>
          {breadcrumbs.length > 0 ? (
            <div className="flex flex-wrap items-center gap-1 text-xs">
              {breadcrumbs.map((crumb, index) => (
                <span key={crumb.path} className="flex items-center gap-1">
                  {index > 0 ? (
                    <ChevronRight className="size-3" aria-hidden="true" />
                  ) : null}
                  <button
                    type="button"
                    className="max-w-44 truncate rounded px-1 py-0.5 text-[#2d6654] hover:bg-white"
                    onClick={() => onOpenDirectory(crumb.path)}
                    disabled={loadingRunData || crumb.path === currentDirectory}
                    title={crumb.path}
                  >
                    {crumb.label}
                  </button>
                </span>
              ))}
            </div>
          ) : null}
        </div>
        <div className="max-h-[560px] overflow-auto">
          {loadingRunData ? (
            <EmptyState text="Loading directory data" />
          ) : directoryChildren.length === 0 ? (
            <EmptyState text="No children for this directory" />
          ) : (
            directoryChildren.map((entry) => (
              <button
                key={entry.id}
                type="button"
                onClick={() =>
                  entry.isDirectory && void onOpenDirectory(entry.path)
                }
                disabled={!entry.isDirectory}
                className="grid w-full grid-cols-[1fr_120px_90px] items-center gap-3 border-b border-[#edf0ea] py-3 text-left text-sm disabled:cursor-default"
              >
                <span className="min-w-0">
                  <span className="flex items-center gap-2 truncate font-medium">
                    {entry.isDirectory ? (
                      <FolderTree
                        className="size-4 text-[#2d6654]"
                        aria-hidden="true"
                      />
                    ) : (
                      <FileSearch
                        className="size-4 text-[#697267]"
                        aria-hidden="true"
                      />
                    )}
                    {entry.name}
                  </span>
                  <span className="block truncate text-xs text-[#697267]">
                    {entry.path}
                  </span>
                </span>
                <span className="text-right tabular-nums">
                  {formatBytes(entry.totalSize || entry.size)}
                </span>
                <span className="text-right text-xs text-[#697267]">
                  {entry.isDirectory
                    ? `${formatNumber(entry.fileCount)} files`
                    : formatDateTime(entry.modifiedAtMs)}
                </span>
              </button>
            ))
          )}
        </div>
      </Panel>

      <Panel title="File search">
        <div className="mb-3 grid grid-cols-[1fr_90px_110px] gap-2">
          <label className="flex items-center gap-2 rounded-md border border-[#d8ddd2] bg-white px-3 py-2">
            <Search className="size-4 text-[#697267]" aria-hidden="true" />
            <input
              value={fileQuery}
              onChange={(event) => onFileQueryChange(event.target.value)}
              className="min-w-0 flex-1 bg-transparent text-sm outline-none"
              placeholder="Search files"
            />
          </label>
          <input
            value={extensionFilter}
            onChange={(event) => onExtensionFilterChange(event.target.value)}
            className="rounded-md border border-[#d8ddd2] bg-white px-3 py-2 text-sm outline-none"
            placeholder=".log"
          />
          <input
            value={minSizeMbFilter}
            onChange={(event) => onMinSizeMbFilterChange(event.target.value)}
            className="rounded-md border border-[#d8ddd2] bg-white px-3 py-2 text-sm outline-none"
            placeholder="Min MB"
            inputMode="numeric"
          />
        </div>
        <div className="max-h-[560px] overflow-auto">
          {files.length === 0 ? (
            <EmptyState text="No files match this filter" />
          ) : (
            files.map((file) => (
              <div
                key={file.id}
                className="border-b border-[#edf0ea] py-3 text-sm"
              >
                <div className="flex items-center justify-between gap-3">
                  <span className="truncate font-medium">{file.name}</span>
                  <span className="tabular-nums">{formatBytes(file.size)}</span>
                </div>
                <div className="mt-1 truncate text-xs text-[#697267]">
                  {file.path}
                </div>
              </div>
            ))
          )}
        </div>
      </Panel>
    </div>
  )
}

function RecoverSpaceView({
  recommendationPlan,
  recoverSummary,
  visibleRecommendations,
  hiddenCandidateCount,
  recoverMode,
  onRecoverModeChange,
  selectedCleanupIds,
  selectedRecommendationBytes,
  topDirectories,
  topFiles,
  categories,
  extensions,
  cleanupPending,
  isScanning,
  driveStatus,
  offloadActionState,
  activeOffloadJob,
  onToggleCandidate,
  onClean,
  onOffload,
  onConnectDrive,
  onDisconnectDrive,
  onCancelOffload,
}: {
  recommendationPlan: RecommendationPlan | null
  recoverSummary: RecoverCandidateSummary
  visibleRecommendations: RecommendationCandidate[]
  hiddenCandidateCount: number
  recoverMode: RecoverMode
  onRecoverModeChange: (mode: RecoverMode) => void
  selectedCleanupIds: Set<string>
  selectedRecommendationBytes: number
  topDirectories: TopDirectory[]
  topFiles: FileSearchResult[]
  categories: CategorySummary[]
  extensions: ExtensionSummary[]
  cleanupPending: boolean
  isScanning: boolean
  driveStatus: GoogleDriveStatus | null
  offloadActionState: ReturnType<typeof getOffloadActionState>
  activeOffloadJob: OffloadJobStatus | null
  onToggleCandidate: (id: string) => void
  onClean: () => void
  onOffload: () => void
  onConnectDrive: () => void
  onDisconnectDrive: () => void
  onCancelOffload: () => void
}) {
  const offloadProgress = activeOffloadJob
    ? getOffloadProgress(activeOffloadJob)
    : null
  const selectedCount = visibleRecommendations.filter((candidate) =>
    selectedCleanupIds.has(candidate.id)
  ).length
  const executeDisabled = cleanupActionDisabled({
    hasPlan: Boolean(recommendationPlan),
    selectedCount,
    pending: cleanupPending,
    scanning: isScanning,
  })
  const primaryAction =
    recoverMode === "clean" ? (
      <Button onClick={onClean} disabled={executeDisabled}>
        {cleanupPending ? (
          <Loader2 className="size-4 animate-spin" aria-hidden="true" />
        ) : (
          <Trash2 className="size-4" aria-hidden="true" />
        )}
        {cleanupPending ? "Moving" : "Move to Recycle Bin"}
      </Button>
    ) : recoverMode === "offload" ? (
      <Button onClick={onOffload} disabled={offloadActionState.disabled}>
        <UploadCloud className="size-4" aria-hidden="true" />
        {offloadActionState.label}
      </Button>
    ) : null

  return (
    <div className="space-y-4">
      <div className="grid gap-3 md:grid-cols-4">
        <Metric
          label={`Clean (${formatNumber(recoverSummary.cleanCount)})`}
          value={formatBytes(recoverSummary.cleanBytes)}
        />
        <Metric
          label={`Offload (${formatNumber(recoverSummary.offloadCount)})`}
          value={formatBytes(recoverSummary.offloadBytes)}
        />
        <Metric
          label={`Review (${formatNumber(recoverSummary.reviewCount)})`}
          value={formatBytes(recoverSummary.reviewBytes)}
        />
        <Metric
          label="Selected"
          value={formatBytes(selectedRecommendationBytes)}
        />
      </div>

      <Panel title="Space usage">
        <div className="grid gap-4 xl:grid-cols-2">
          <div>
            <h3 className="mb-2 text-sm font-semibold text-[#253128]">
              Largest directories
            </h3>
            <DataTable
              headers={["Path", "Size", "Files"]}
              empty="No directory data yet"
              rows={topDirectories.slice(0, 8).map((directory) => [
                directory.path,
                formatBytes(directory.totalSize),
                formatNumber(directory.fileCount),
              ])}
            />
          </div>
          <div>
            <h3 className="mb-2 text-sm font-semibold text-[#253128]">
              Largest files
            </h3>
            <DataTable
              headers={["File", "Size", "Modified"]}
              empty="No file data yet"
              rows={topFiles.slice(0, 8).map((file) => [
                file.path,
                formatBytes(file.size),
                formatDateTime(file.modifiedAtMs),
              ])}
            />
          </div>
          <div>
            <h3 className="mb-2 text-sm font-semibold text-[#253128]">
              File mix
            </h3>
            <SummaryList
              rows={categories
                .slice(0, 8)
                .map((row) => [row.category, formatBytes(row.totalSize)])}
            />
          </div>
          <div>
            <h3 className="mb-2 text-sm font-semibold text-[#253128]">
              Top extensions
            </h3>
            <SummaryList
              rows={extensions
                .slice(0, 8)
                .map((row) => [
                  row.extension ? `.${row.extension}` : "No extension",
                  formatBytes(row.totalSize),
                ])}
            />
          </div>
        </div>
      </Panel>

      <Panel
        title="Recover Space"
        action={
          <div className="flex flex-wrap items-center gap-2">
            {recoverMode === "offload" && (
              <Button
                variant="outline"
                onClick={
                  driveStatus?.connected ? onDisconnectDrive : onConnectDrive
                }
              >
                <Cloud className="size-4" aria-hidden="true" />
                {driveStatus?.connected
                  ? "Disconnect Drive"
                  : "Sign in with Google"}
              </Button>
            )}
            {primaryAction}
          </div>
        }
      >
        <div className="mb-3 flex flex-wrap gap-2">
          {(["clean", "offload", "review"] as const).map((mode) => (
            <button
              key={mode}
              type="button"
              onClick={() => onRecoverModeChange(mode)}
              className={[
                "rounded-md border px-3 py-2 text-sm font-medium capitalize transition",
                recoverMode === mode
                  ? "border-[#2d6654] bg-[#edf6ef] text-[#123c2f]"
                  : "border-[#d8ddd2] bg-white text-[#596259] hover:border-[#9db2a7]",
              ].join(" ")}
            >
              {mode}
            </button>
          ))}
        </div>

        {recommendationPlan?.warnings.map((warning) => (
          <Notice key={warning} tone="info" text={warning} />
        ))}
        {recoverMode === "offload" && driveStatus?.error ? (
          <Notice tone="warning" text={driveStatus.error} />
        ) : null}
        {recoverMode === "offload" && offloadActionState.reason ? (
          <Notice tone="warning" text={offloadActionState.reason} />
        ) : null}
        {activeOffloadJob && offloadProgress ? (
          <div className="my-3 rounded-md border border-[#d8ddd2] bg-white p-3">
            <div className="flex items-center justify-between gap-3 text-sm">
              <span className="font-medium">
                Batch {activeOffloadJob.id} · {activeOffloadJob.status}
              </span>
              <span className="tabular-nums">{offloadProgress.percent}%</span>
            </div>
            <div className="mt-2 h-2 overflow-hidden rounded-full bg-[#e1e5dd]">
              <div
                className="h-full bg-[#2d6654]"
                style={{ width: `${offloadProgress.percent}%` }}
              />
            </div>
            <div className="mt-2 flex items-center justify-between gap-3 text-xs text-[#697267]">
              <span>{offloadProgress.text}</span>
              {activeOffloadJob.status === "queued" ||
              activeOffloadJob.status === "running" ? (
                <button
                  type="button"
                  onClick={onCancelOffload}
                  className="font-medium text-[#8d2f24]"
                >
                  Cancel
                </button>
              ) : null}
            </div>
            {activeOffloadJob.items.length > 0 ? (
              <div className="mt-3 max-h-52 overflow-auto border-t border-[#edf0ea] pt-2">
                {activeOffloadJob.items.map((item) => (
                  <div
                    key={item.id}
                    className="grid grid-cols-[1fr_110px_110px] gap-3 py-1.5 text-xs text-[#596259]"
                  >
                    <span className="truncate" title={item.sourcePath}>
                      {item.sourcePath}
                    </span>
                    <span>{item.recycleStatus ?? item.status}</span>
                    <span className="text-right tabular-nums">
                      {formatBytes(item.uploadedBytes)}
                    </span>
                    {item.error ? (
                      <span className="col-span-3 text-[#8d2f24]">
                        {item.error}
                      </span>
                    ) : null}
                  </div>
                ))}
              </div>
            ) : null}
          </div>
        ) : null}
        <div className="mt-3 max-h-[560px] overflow-auto">
          {!recommendationPlan || visibleRecommendations.length === 0 ? (
            <EmptyState text="No candidates found for this mode" />
          ) : (
            visibleRecommendations.map((candidate) => (
              <label
                key={candidate.id}
                className="grid cursor-pointer grid-cols-[24px_1fr_150px_90px] items-center gap-3 border-b border-[#edf0ea] py-3 text-sm"
              >
                <input
                  type="checkbox"
                  checked={selectedCleanupIds.has(candidate.id)}
                  onChange={() => onToggleCandidate(candidate.id)}
                />
                <span className="min-w-0">
                  <span className="block truncate font-medium">
                    {candidate.name}
                  </span>
                  <span className="block truncate text-xs text-[#697267]">
                    {candidate.path}
                  </span>
                </span>
                <span className="text-xs text-[#697267]">
                  {candidate.reason} · {candidate.riskLevel}
                </span>
                <span className="text-right tabular-nums">
                  {formatBytes(candidate.size)}
                </span>
              </label>
            ))
          )}
        </div>
        {hiddenCandidateCount > 0 ? (
          <Notice
            tone="info"
            text={`Showing the first ${formatNumber(
              visibleRecommendations.length
            )} candidates in this mode; ${formatNumber(
              hiddenCandidateCount
            )} more are available in the scan data.`}
          />
        ) : null}
      </Panel>
    </div>
  )
}

function HistoryView({
  cleanupHistory,
  scanRuns,
  historyPending,
  onOpenRecycleBin,
  onDeleteRun,
  onClearScanHistory,
}: {
  cleanupHistory: CleanupActionRecord[]
  scanRuns: ScanRunRecord[]
  historyPending: boolean
  onOpenRecycleBin: () => void
  onDeleteRun: (runId: number) => void
  onClearScanHistory: () => void
}) {
  const clearableScanCount = scanRuns.filter(
    (run) => run.status !== "running"
  ).length

  return (
    <div className="grid gap-4 xl:grid-cols-2">
      <Panel
        title="Cleanup history"
        action={
          <Button variant="outline" size="sm" onClick={onOpenRecycleBin}>
            <FolderOpen className="size-4" aria-hidden="true" />
            Open Recycle Bin
          </Button>
        }
      >
        {cleanupHistory.length === 0 ? (
          <EmptyState text="No cleanup actions recorded yet" />
        ) : (
          cleanupHistory.map((action) => (
            <div
              key={action.id}
              className="flex items-center justify-between gap-3 border-b border-[#edf0ea] py-3 text-sm"
            >
              <div className="min-w-0">
                <div className="truncate font-medium">{action.targetPath}</div>
                <div className="text-xs text-[#697267]">
                  {action.status} · {action.destinationPath ?? "no destination"}{" "}
                  · {formatDateTime(action.createdAtMs)}
                </div>
              </div>
            </div>
          ))
        )}
      </Panel>

      <Panel
        title="Completed scans"
        action={
          <Button
            variant="outline"
            size="sm"
            disabled={historyPending || clearableScanCount === 0}
            onClick={onClearScanHistory}
          >
            {historyPending ? (
              <Loader2 className="size-4 animate-spin" aria-hidden="true" />
            ) : (
              <Trash2 className="size-4" aria-hidden="true" />
            )}
            Clear
          </Button>
        }
      >
        {scanRuns.length === 0 ? (
          <EmptyState text="No scan history recorded yet" />
        ) : (
          <div className="space-y-2">
            {scanRuns.map((run) => (
              <div
                key={run.id}
                className="flex items-center justify-between gap-3 border-b border-[#edf0ea] py-3 text-sm"
              >
                <div className="min-w-0">
                  <div className="truncate font-medium">{run.rootPath}</div>
                  <div className="text-xs text-[#697267]">
                    {run.status} · {formatBytes(run.bytesIndexed)} ·{" "}
                    {formatDateTime(run.finishedAtMs)}
                  </div>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={historyPending || run.status === "running"}
                  onClick={() => void onDeleteRun(run.id)}
                >
                  <Trash2 className="size-4" aria-hidden="true" />
                  Delete
                </Button>
              </div>
            ))}
          </div>
        )}
      </Panel>
    </div>
  )
}

function SettingsView({
  health,
  onExportDiagnostics,
  diagnosticsPending,
}: {
  health: AppHealth | null
  onExportDiagnostics: () => void
  diagnosticsPending: boolean
}) {
  return (
    <div className="grid gap-4 xl:grid-cols-[1fr_380px]">
      <Panel
        title="Production posture"
        action={
          <Button
            variant="outline"
            onClick={onExportDiagnostics}
            disabled={diagnosticsPending}
          >
            {diagnosticsPending ? (
              <Loader2 className="size-4 animate-spin" aria-hidden="true" />
            ) : (
              <Download className="size-4" aria-hidden="true" />
            )}
            {diagnosticsPending ? "Exporting" : "Export diagnostics"}
          </Button>
        }
      >
        <div className="grid gap-3 md:grid-cols-2">
          <Posture
            icon={ShieldCheck}
            title="Privacy"
            text="Scan data stays local; selected files leave the device only when offloaded."
          />
          <Posture
            icon={Trash2}
            title="Cleanup"
            text="Cleanup uses Windows Recycle Bin; permanent delete is not exposed."
          />
          <Posture
            icon={Database}
            title="Storage"
            text={`SQLite schema ${health?.schemaVersion ?? "unknown"}`}
          />
          <Posture
            icon={Cloud}
            title="Google Drive"
            text={
              health?.googleDriveConnected
                ? "Connected for verified offload batches."
                : health?.googleDriveConfigured
                  ? "Configured, waiting for sign-in."
                  : "Set WINCLEAN_GOOGLE_CLIENT_ID to enable offload."
            }
          />
        </div>
      </Panel>

      <Panel title="App health">
        <SummaryList
          rows={[
            ["Version", health?.appVersion ?? "unknown"],
            ["Active scan", health?.activeScanState ?? "unknown"],
            [
              "Latest run",
              health?.latestCompletedRunId
                ? String(health.latestCompletedRunId)
                : "none",
            ],
            ["Pending actions", formatNumber(health?.pendingActionCount ?? 0)],
            [
              "Pending offloads",
              formatNumber(health?.pendingOffloadCount ?? 0),
            ],
            ["Database", health?.databasePath ?? "unknown"],
          ]}
        />
      </Panel>
    </div>
  )
}

function NavButton({
  icon: Icon,
  label,
  active,
  onClick,
}: {
  icon: React.ComponentType<{ className?: string; "aria-hidden"?: boolean }>
  label: string
  active: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={[
        "flex h-9 w-full items-center gap-2 rounded-md px-3 text-left text-sm font-medium transition",
        active
          ? "bg-[#123c2f] text-white"
          : "text-[#4d574e] hover:bg-[#eef2eb]",
      ].join(" ")}
    >
      <Icon className="size-4" aria-hidden={true} />
      {label}
    </button>
  )
}

function Panel({
  title,
  action,
  children,
}: {
  title: string
  action?: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <section className="rounded-lg border border-[#d8ddd2] bg-white shadow-sm">
      <div className="flex items-center justify-between gap-3 border-b border-[#e3e7df] px-4 py-3">
        <h3 className="text-sm font-semibold text-[#596259] uppercase">
          {title}
        </h3>
        {action}
      </div>
      <div className="p-4">{children}</div>
    </section>
  )
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-[#d8ddd2] bg-white p-4 shadow-sm">
      <div className="text-xs text-[#6d756c] uppercase">{label}</div>
      <div className="mt-1 truncate text-xl font-semibold tabular-nums">
        {value}
      </div>
    </div>
  )
}

function Notice({
  text,
  tone,
}: {
  text: string
  tone: "warning" | "danger" | "info"
}) {
  const classes =
    tone === "danger"
      ? "border-[#efb1a3] bg-[#fff0ec] text-[#7b2c1d]"
      : tone === "warning"
        ? "border-[#ecd48f] bg-[#fff8df] text-[#69521d]"
        : "border-[#b8d7d1] bg-[#eef8f5] text-[#27574e]"
  return (
    <div className={`mb-3 flex gap-3 rounded-md border p-3 text-sm ${classes}`}>
      <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <p>{text}</p>
    </div>
  )
}

function DataTable({
  headers,
  rows,
  empty = "No data",
}: {
  headers: string[]
  rows: string[][]
  empty?: string
}) {
  return (
    <table className="w-full table-fixed border-collapse text-sm">
      <thead>
        <tr className="border-b border-[#dfe4dc] text-left text-xs text-[#697267] uppercase">
          {headers.map((header, index) => (
            <th
              key={header}
              className={`py-2 font-semibold ${index > 0 ? "text-right" : ""}`}
            >
              {header}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.length === 0 ? (
          <tr>
            <td
              className="py-10 text-center text-[#697267]"
              colSpan={headers.length}
            >
              {empty}
            </td>
          </tr>
        ) : (
          rows.map((row) => (
            <tr key={row.join("|")} className="border-b border-[#edf0ea]">
              {row.map((cell, index) => (
                <td
                  key={`${cell}-${index}`}
                  className={`truncate py-3 ${index > 0 ? "text-right tabular-nums" : "pr-3 font-medium"}`}
                  title={cell}
                >
                  {cell}
                </td>
              ))}
            </tr>
          ))
        )}
      </tbody>
    </table>
  )
}

function SummaryList({ rows }: { rows: [string, string][] }) {
  if (rows.length === 0) {
    return <EmptyState text="No summary data yet" />
  }
  return (
    <div className="space-y-2">
      {rows.map(([label, value]) => (
        <div
          key={label}
          className="flex items-center justify-between gap-3 text-sm"
        >
          <span className="truncate text-[#4d574e]">{label}</span>
          <span className="shrink-0 font-medium tabular-nums">{value}</span>
        </div>
      ))}
    </div>
  )
}

function Posture({
  icon: Icon,
  title,
  text,
}: {
  icon: React.ComponentType<{ className?: string; "aria-hidden"?: boolean }>
  title: string
  text: string
}) {
  return (
    <div className="rounded-md border border-[#e3e7df] bg-[#fbfcf8] p-3">
      <div className="mb-2 flex items-center gap-2 font-medium">
        <Icon className="size-4 text-[#2d6654]" aria-hidden={true} />
        {title}
      </div>
      <p className="text-sm text-[#667066]">{text}</p>
    </div>
  )
}

function EmptyState({ text }: { text: string }) {
  return <div className="py-10 text-center text-sm text-[#697267]">{text}</div>
}

function viewTitle(view: View) {
  if (view === "explorer") {
    return "Explorer"
  }
  if (view === "cleanup") {
    return "Safe cleanup"
  }
  if (view === "history") {
    return "History"
  }
  if (view === "settings") {
    return "Settings"
  }
  return "Overview"
}

function formatError(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export default App
