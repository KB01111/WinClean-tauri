import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { toast } from "sonner"

import { useOffload } from "@/hooks/use-offload"
import { usePolling } from "@/hooks/use-polling"
import { useRunData } from "@/hooks/use-run-data"
import {
  api,
  emptyScanProgress,
  formatError,
  type AppHealth,
  type CleanupActionRecord,
  type GoogleDriveStatus,
  type OffloadJobStatus,
  type ScanProgress,
  type ScanRunRecord,
  type VolumeInfo,
} from "@/lib/api"
import { directoryBreadcrumbs, parentDirectoryPath } from "@/lib/app-state"
import { formatNumber } from "@/lib/format"
import { selectedVisibleCandidateIds, type RecoverMode } from "@/lib/recover-space"
import {
  runDataRefreshKey,
  shouldRefreshDashboardCollectionsForProgress,
  shouldRefreshRunDataForProgress,
} from "@/lib/scan-refresh"
import { selectDashboardRun } from "@/lib/scan-selection"
import { selectDefaultScanRoot } from "@/lib/volume-selection"
import type { View } from "@/lib/views"

/**
 * The storage workflow: volumes, scans, scan history, cleanup, Google Drive
 * offload and exports. Run-derived data lives in `useRunData`; this hook owns
 * the run selection and every action that changes what is on disk or in the
 * scan database.
 */
export function useStorage({ onNavigate }: { onNavigate: (view: View) => void }) {
  const [volumes, setVolumes] = useState<VolumeInfo[]>([])
  const [selectedRoot, setSelectedRoot] = useState("")
  const [progress, setProgress] = useState<ScanProgress>(emptyScanProgress)
  const [scanRuns, setScanRuns] = useState<ScanRunRecord[]>([])
  const [selectedRunId, setSelectedRunId] = useState<number | null>(null)
  const [currentDirectory, setCurrentDirectory] = useState<string | null>(null)
  const [cleanupHistory, setCleanupHistory] = useState<CleanupActionRecord[]>([])
  const [offloadJobs, setOffloadJobs] = useState<OffloadJobStatus[]>([])
  const [driveStatus, setDriveStatus] = useState<GoogleDriveStatus | null>(null)
  const [health, setHealth] = useState<AppHealth | null>(null)
  const [aiKeyConfigured, setAiKeyConfigured] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [loadingVolumes, setLoadingVolumes] = useState(true)
  const [loadingData, setLoadingData] = useState(false)
  const [scanPending, setScanPending] = useState(false)
  const [cancelPending, setCancelPending] = useState(false)
  const [cleanupPending, setCleanupPending] = useState(false)
  const [reportPending, setReportPending] = useState(false)
  const [diagnosticsPending, setDiagnosticsPending] = useState(false)
  const [historyPending, setHistoryPending] = useState(false)
  const [restorePendingItemId, setRestorePendingItemId] = useState<number | null>(null)
  const dashboardRefreshInFlight = useRef(false)
  const scanStatusRefreshInFlight = useRef(false)
  const initialLoadStarted = useRef(false)
  const lastRunDataRefreshKey = useRef<string | null>(null)

  const runData = useRunData({ selectedRunId, setMessage })
  const { refreshRunData } = runData

  const selectedVolume = useMemo(() => volumes.find((volume) => volume.rootPath === selectedRoot), [selectedRoot, volumes])
  const activeRun = useMemo(
    () => scanRuns.find((run) => run.id === selectedRunId) ?? scanRuns.find((run) => run.status === "completed") ?? null,
    [scanRuns, selectedRunId]
  )
  const isScanning = progress.state === "running"
  const explorerBreadcrumbs = useMemo(() => directoryBreadcrumbs(activeRun?.rootPath, currentDirectory), [activeRun?.rootPath, currentDirectory])
  const explorerParentDirectory = useMemo(() => parentDirectoryPath(currentDirectory, activeRun?.rootPath), [activeRun?.rootPath, currentDirectory])

  const refreshDashboard = useCallback(
    async (options?: { clearMessage?: boolean }) => {
      if (dashboardRefreshInFlight.current) return
      dashboardRefreshInFlight.current = true
      setLoadingData(true)
      try {
        const [nextProgress, runs, historyRows, jobRows, nextHealth, nextDriveStatus] = await Promise.all([
          api.scanStatus(),
          api.scanRuns(30),
          api.cleanupHistory(50),
          api.listOffloadJobs(20),
          api.appHealth(),
          api.googleDriveStatus(),
        ])
        setProgress(nextProgress)
        setScanRuns(runs)
        setCleanupHistory(historyRows)
        setOffloadJobs(jobRows)
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
          lastRunDataRefreshKey.current = runDataRefreshKey(nextRunId, nextDirectory)
          runDataRefreshSucceeded = await refreshRunData(nextRunId, nextDirectory, { clearMessage: options?.clearMessage })
        }
        if (runDataRefreshSucceeded && options?.clearMessage !== false) setMessage(null)
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

  const offload = useOffload({
    onJobSettled: () => {
      void refreshDashboard()
    },
  })

  const refreshScanStatus = useCallback(async () => {
    if (scanStatusRefreshInFlight.current) return
    scanStatusRefreshInFlight.current = true
    try {
      const nextProgress = await api.scanStatus()
      setProgress(nextProgress)
      if (shouldRefreshDashboardCollectionsForProgress(nextProgress)) await refreshDashboard()
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      scanStatusRefreshInFlight.current = false
    }
  }, [refreshDashboard])

  const refreshVolumes = useCallback(async () => {
    setLoadingVolumes(true)
    try {
      const nextVolumes = await api.listVolumes()
      setVolumes(nextVolumes)
      setSelectedRoot((current) => selectDefaultScanRoot(nextVolumes, current))
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setLoadingVolumes(false)
    }
  }, [])

  useEffect(() => {
    if (initialLoadStarted.current) return
    initialLoadStarted.current = true
    void refreshVolumes()
    void refreshDashboard()
    api
      .aiKeyStatus()
      .then((status) => setAiKeyConfigured(status.configured))
      .catch(() => {})
  }, [refreshDashboard, refreshVolumes])

  // Scan progress is polled only while a scan runs and the window is visible.
  usePolling(refreshScanStatus, isScanning ? 900 : null)

  const { fileQuery, extensionFilter } = runData
  useEffect(() => {
    if (!activeRun || isScanning) return
    const key = runDataRefreshKey(activeRun.id, currentDirectory)
    if (lastRunDataRefreshKey.current === key) {
      lastRunDataRefreshKey.current = null
      return
    }
    const timeout = window.setTimeout(() => {
      void refreshRunData(activeRun.id, currentDirectory ?? activeRun.rootPath)
    }, 200)
    return () => window.clearTimeout(timeout)
  }, [activeRun, currentDirectory, extensionFilter, fileQuery, isScanning, refreshRunData])

  async function startScan() {
    if (!selectedVolume || scanPending) return
    setScanPending(true)
    setMessage(null)
    runData.clear()
    setSelectedRunId(null)
    setCurrentDirectory(selectedVolume.rootPath)
    try {
      const nextProgress = await api.startScan(selectedVolume.rootPath, selectedVolume.fileSystem ?? null)
      setProgress(nextProgress)
      setSelectedRunId(nextProgress.runId ?? null)
      setCurrentDirectory(nextProgress.rootPath ?? selectedVolume.rootPath)
      onNavigate("overview")
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setScanPending(false)
    }
  }

  async function cancelScan() {
    if (cancelPending) return
    setCancelPending(true)
    try {
      setProgress(await api.cancelScan())
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
    onNavigate("explorer")
    setCurrentDirectory(path)
    await refreshRunData(activeRun?.id, path)
  }

  async function executeSelectedCleanup() {
    const runId = runData.recommendationPlan?.scanRunId ?? runData.cleanupPlan?.scanRunId
    const candidateIds = selectedVisibleCandidateIds(runData.visibleRecommendations, runData.selectedCleanupIds)
    if (!runId || candidateIds.length === 0 || cleanupPending) return
    setCleanupPending(true)
    try {
      const result = await api.executeCleanup(runId, candidateIds)
      toast.success(`Cleanup sent ${result.recycledCount} item(s) to Recycle Bin; ${result.skippedCount} skipped; ${result.failedCount} failed.`)
      await refreshDashboard({ clearMessage: false })
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setCleanupPending(false)
    }
  }

  async function connectGoogleDrive() {
    try {
      toast.info("Opening Google Drive sign-in in your browser…")
      const status = await api.beginGoogleDriveAuth()
      setDriveStatus(status)
      if (status.connected) {
        toast.success(status.accountEmail ? `Google Drive connected as ${status.accountEmail}.` : "Google Drive connected.")
      } else {
        toast.warning(status.error ?? "Google Drive is not connected.")
      }
      await refreshDashboard()
    } catch (error) {
      toast.error(formatError(error))
    }
  }

  async function disconnectGoogleDrive() {
    try {
      const status = await api.disconnectGoogleDrive()
      setDriveStatus(status)
      toast.success("Google Drive disconnected.")
      await refreshDashboard()
    } catch (error) {
      toast.error(formatError(error))
    }
  }

  async function startSelectedOffload() {
    if (!runData.recommendationPlan) return
    const candidateIds = selectedVisibleCandidateIds(runData.visibleRecommendations, runData.selectedCleanupIds)
    await offload.start(runData.recommendationPlan.scanRunId, candidateIds)
  }

  async function restoreOffloadItem(itemId: number) {
    if (restorePendingItemId !== null) return
    setRestorePendingItemId(itemId)
    try {
      const item = await api.restoreOffloadItem(itemId)
      toast.success(`Restored ${item.sourcePath} from Google Drive.`)
      await refreshDashboard({ clearMessage: false })
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setRestorePendingItemId(null)
    }
  }

  async function openRecycleBin() {
    try {
      await api.openRecycleBin()
      toast.success("Windows Recycle Bin opened.")
    } catch (error) {
      toast.error(formatError(error))
    }
  }

  async function exportReport() {
    if (reportPending) return
    setReportPending(true)
    try {
      const artifact = await api.exportReport(activeRun?.id ?? null)
      if (artifact) toast.success(`Report exported to ${artifact.path}`)
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setReportPending(false)
    }
  }

  async function exportDiagnostics() {
    if (diagnosticsPending) return
    setDiagnosticsPending(true)
    try {
      const artifact = await api.exportDiagnostics()
      toast.success(`Diagnostics exported to ${artifact.path}`)
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setDiagnosticsPending(false)
    }
  }

  async function deleteRun(runId: number) {
    if (historyPending) return
    setHistoryPending(true)
    try {
      const deleted = await api.deleteScanRun(runId)
      if (deleted) {
        if (selectedRunId === runId) {
          setSelectedRunId(null)
          setCurrentDirectory(null)
          lastRunDataRefreshKey.current = null
        }
        toast.success("Scan history entry deleted.")
      }
      await refreshDashboard({ clearMessage: false })
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setHistoryPending(false)
    }
  }

  async function clearScanHistory() {
    if (historyPending) return
    setHistoryPending(true)
    try {
      const deletedCount = await api.clearScanHistory()
      setSelectedRunId(null)
      setCurrentDirectory(null)
      lastRunDataRefreshKey.current = null
      toast.success(`Cleared ${formatNumber(deletedCount)} completed scan(s).`)
      await refreshDashboard({ clearMessage: false })
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setHistoryPending(false)
    }
  }

  /** Make the analyzed run active, then preselect the AI-suggested candidates in Recover space. */
  async function applyAiSuggestion(runId: number, mode: RecoverMode, candidateIds: string[]) {
    // Candidate ids are scoped to the analyzed run, so make it the active run first.
    if (runId !== activeRun?.id) {
      const analyzedRun = scanRuns.find((run) => run.id === runId)
      if (!analyzedRun) {
        toast.error("The analyzed scan run is no longer available.")
        return
      }
      await selectRun(analyzedRun)
    }
    runData.setRecoverMode(mode)
    runData.setSelectedCleanupIds(new Set(candidateIds))
    onNavigate("cleanup")
    toast.success(`Added ${candidateIds.length} item(s) to the ${mode} selection.`)
  }

  return {
    runData,
    offload,
    volumes,
    selectedRoot,
    setSelectedRoot,
    selectedVolume,
    progress,
    scanRuns,
    activeRun,
    currentDirectory,
    explorerBreadcrumbs,
    explorerParentDirectory,
    cleanupHistory,
    offloadJobs,
    driveStatus,
    health,
    aiKeyConfigured,
    setAiKeyConfigured,
    message,
    isScanning,
    loadingVolumes,
    loadingData,
    scanPending,
    cancelPending,
    cleanupPending,
    reportPending,
    diagnosticsPending,
    historyPending,
    restorePendingItemId,
    refreshDashboard,
    startScan,
    cancelScan,
    selectRun,
    openDirectory,
    executeSelectedCleanup,
    connectGoogleDrive,
    disconnectGoogleDrive,
    startSelectedOffload,
    restoreOffloadItem,
    openRecycleBin,
    exportReport,
    exportDiagnostics,
    deleteRun,
    clearScanHistory,
    applyAiSuggestion,
  }
}

export type StorageController = ReturnType<typeof useStorage>
