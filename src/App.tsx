import { Loader2, Play, RefreshCw, Square } from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Notice } from "@/components/common"
import { Sidebar, type View } from "@/components/layout/sidebar"
import { useOffload } from "@/hooks/use-offload"
import {
  api,
  emptyScanProgress,
  formatError,
  type AppHealth,
  type CategorySummary,
  type CleanupActionRecord,
  type CleanupPlan,
  type DirectoryChild,
  type ExtensionSummary,
  type FileSearchResult,
  type GoogleDriveStatus,
  type OffloadJobStatus,
  type RecommendationPlan,
  type ScanProgress,
  type ScanRunRecord,
  type TopDirectory,
  type VolumeInfo,
} from "@/lib/api"
import {
  directoryBreadcrumbs,
  parentDirectoryPath,
  pruneCleanupSelection,
  shouldCommitRunDataResponse,
} from "@/lib/app-state"
import { formatNumber, statusText } from "@/lib/format"
import {
  getVisibleRecoverCandidates,
  selectedVisibleCandidateIds,
  selectedVisibleCandidateStats,
  type RecoverMode,
} from "@/lib/recover-space"
import { runDataRefreshKey, shouldRefreshDashboardCollectionsForProgress, shouldRefreshRunDataForProgress } from "@/lib/scan-refresh"
import { selectDashboardRun } from "@/lib/scan-selection"
import { selectDefaultScanRoot } from "@/lib/volume-selection"
import { AiInsightsView } from "@/views/ai-insights-view"
import { ExplorerView } from "@/views/explorer-view"
import { HistoryView } from "@/views/history-view"
import { OverviewView } from "@/views/overview-view"
import {
  RecoverSpaceView,
  type RecoverFilterState,
} from "@/views/recover-space-view"
import { SettingsView } from "@/views/settings-view"

function App() {
  const [view, setView] = useState<View>("overview")
  const [volumes, setVolumes] = useState<VolumeInfo[]>([])
  const [selectedRoot, setSelectedRoot] = useState("")
  const [progress, setProgress] = useState<ScanProgress>(emptyScanProgress)
  const [scanRuns, setScanRuns] = useState<ScanRunRecord[]>([])
  const [selectedRunId, setSelectedRunId] = useState<number | null>(null)
  const [topDirectories, setTopDirectories] = useState<TopDirectory[]>([])
  const [directoryChildren, setDirectoryChildren] = useState<DirectoryChild[]>(
    []
  )
  const [currentDirectory, setCurrentDirectory] = useState<string | null>(null)
  const [files, setFiles] = useState<FileSearchResult[]>([])
  const [staleFiles, setStaleFiles] = useState<FileSearchResult[]>([])
  const [fileQuery, setFileQuery] = useState("")
  const [extensionFilter, setExtensionFilter] = useState("")
  const [minSizeMbFilter, setMinSizeMbFilter] = useState("")
  const [extensions, setExtensions] = useState<ExtensionSummary[]>([])
  const [categories, setCategories] = useState<CategorySummary[]>([])
  const [cleanupPlan, setCleanupPlan] = useState<CleanupPlan | null>(null)
  const [recommendationPlan, setRecommendationPlan] =
    useState<RecommendationPlan | null>(null)
  const [recoverMode, setRecoverMode] = useState<RecoverMode>("clean")
  const [recoverFilters, setRecoverFilters] = useState<RecoverFilterState>({
    minSizeMb: "",
    minAgeDays: "",
  })
  const [selectedCleanupIds, setSelectedCleanupIds] = useState<Set<string>>(
    new Set()
  )
  const [cleanupHistory, setCleanupHistory] = useState<CleanupActionRecord[]>(
    []
  )
  const [offloadJobs, setOffloadJobs] = useState<OffloadJobStatus[]>([])
  const [driveStatus, setDriveStatus] = useState<GoogleDriveStatus | null>(null)
  const [health, setHealth] = useState<AppHealth | null>(null)
  const [aiKeyConfigured, setAiKeyConfigured] = useState(false)
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
  const [restorePendingItemId, setRestorePendingItemId] = useState<
    number | null
  >(null)
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
  const selectedVisibleStats = useMemo(
    () =>
      selectedVisibleCandidateStats(visibleRecommendations, selectedCleanupIds),
    [selectedCleanupIds, visibleRecommendations]
  )
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
        setStaleFiles([])
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
      const recoverMinSizeMb = Number(recoverFilters.minSizeMb)
      const recoverMinAgeDays = Number(recoverFilters.minAgeDays)
      const recommendationFilters = {
        minSize:
          Number.isFinite(recoverMinSizeMb) && recoverMinSizeMb > 0
            ? Math.round(recoverMinSizeMb * 1024 * 1024)
            : null,
        minAgeDays:
          Number.isFinite(recoverMinAgeDays) && recoverMinAgeDays > 0
            ? Math.round(recoverMinAgeDays)
            : null,
      }

      try {
        const [
          top,
          children,
          foundFiles,
          stale,
          extensionRows,
          categoryRows,
          plan,
          recommendations,
        ] = await Promise.all([
          api.topDirectories(resolvedRunId, 25),
          api.directoryChildren(resolvedRunId, parentPath ?? null, 250),
          api.fileSearch({
            runId: resolvedRunId,
            query: fileQuery || null,
            extension: extensionFilter || null,
            minSize,
            limit: 100,
          }),
          api.largestStaleFiles({ runId: resolvedRunId, limit: 25 }),
          api.extensionSummary(resolvedRunId, 20),
          api.categorySummary(resolvedRunId),
          api.cleanupPlan(resolvedRunId, 500),
          api.recommendations(resolvedRunId, recommendationFilters),
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
        setStaleFiles(stale)
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
    [
      extensionFilter,
      fileQuery,
      minSizeMbFilter,
      recoverFilters.minAgeDays,
      recoverFilters.minSizeMb,
      selectedRunId,
    ]
  )

  const refreshDashboard = useCallback(
    async (options?: { clearMessage?: boolean }) => {
      if (dashboardRefreshInFlight.current) {
        return
      }
      dashboardRefreshInFlight.current = true
      setLoadingData(true)
      try {
        const [
          nextProgress,
          runs,
          historyRows,
          jobRows,
          nextHealth,
          nextDriveStatus,
        ] = await Promise.all([
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

  const offload = useOffload({
    onJobSettled: () => {
      void refreshDashboard()
    },
  })

  const refreshScanStatus = useCallback(async () => {
    if (scanStatusRefreshInFlight.current) {
      return
    }
    scanStatusRefreshInFlight.current = true
    try {
      const nextProgress = await api.scanStatus()
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
    if (initialLoadStarted.current) {
      return
    }
    initialLoadStarted.current = true
    void refreshVolumes()
    void refreshDashboard()
    api
      .aiKeyStatus()
      .then((status) => setAiKeyConfigured(status.configured))
      .catch(() => {})
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
    setStaleFiles([])
    setCleanupPlan(null)
    setRecommendationPlan(null)
    setSelectedCleanupIds(new Set())
    setSelectedRunId(null)
    setCurrentDirectory(selectedVolume.rootPath)
    try {
      const nextProgress = await api.startScan(
        selectedVolume.rootPath,
        selectedVolume.fileSystem ?? null
      )
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
    setView("explorer")
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

  function toggleAllVisible(select: boolean) {
    setSelectedCleanupIds((current) => {
      const next = new Set(current)
      for (const candidate of visibleRecommendations) {
        if (select) {
          next.add(candidate.id)
        } else {
          next.delete(candidate.id)
        }
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
      const result = await api.executeCleanup(runId, candidateIds)
      toast.success(
        `Cleanup sent ${result.recycledCount} item(s) to Recycle Bin; ${result.skippedCount} skipped; ${result.failedCount} failed.`
      )
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
        toast.success(
          status.accountEmail
            ? `Google Drive connected as ${status.accountEmail}.`
            : "Google Drive connected."
        )
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
    if (!recommendationPlan) {
      return
    }
    const candidateIds = selectedVisibleCandidateIds(
      visibleRecommendations,
      selectedCleanupIds
    )
    await offload.start(recommendationPlan.scanRunId, candidateIds)
  }

  async function restoreOffloadItem(itemId: number) {
    if (restorePendingItemId !== null) {
      return
    }
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
    if (reportPending) {
      return
    }
    setReportPending(true)
    try {
      const artifact = await api.exportReport(activeRun?.id ?? null)
      if (artifact) {
        toast.success(`Report exported to ${artifact.path}`)
      }
    } catch (error) {
      toast.error(formatError(error))
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
      const artifact = await api.exportDiagnostics()
      toast.success(`Diagnostics exported to ${artifact.path}`)
    } catch (error) {
      toast.error(formatError(error))
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
    if (historyPending) {
      return
    }
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

  function applyAiSuggestion(mode: RecoverMode, candidateIds: string[]) {
    setRecoverMode(mode)
    setSelectedCleanupIds(new Set(candidateIds))
    setView("cleanup")
    toast.success(
      `Added ${candidateIds.length} item(s) to the ${mode} selection.`
    )
  }

  return (
    <main className="min-h-svh bg-background text-foreground">
      <div className="grid min-h-svh grid-cols-[260px_1fr]">
        <Sidebar
          view={view}
          onViewChange={setView}
          volumes={volumes}
          selectedRoot={selectedRoot}
          onSelectRoot={setSelectedRoot}
          loadingVolumes={loadingVolumes}
        />

        <section className="min-w-0 p-5">
          <header className="mb-5 flex flex-wrap items-start justify-between gap-3 border-b border-border pb-4">
            <div>
              <h2 className="text-2xl font-semibold tracking-tight">
                {viewTitle(view)}
              </h2>
              <p className="text-sm text-muted-foreground">
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
              topFiles={files}
              staleFiles={staleFiles}
              categories={categories}
              extensions={extensions}
              recommendationPlan={recommendationPlan}
              onSelectRun={selectRun}
              onOpenDirectory={openDirectory}
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
              cleanupWarnings={cleanupPlan?.warnings ?? []}
              visibleRecommendations={visibleRecommendations}
              recoverMode={recoverMode}
              onRecoverModeChange={(mode) => {
                setRecoverMode(mode)
                setSelectedCleanupIds(new Set())
              }}
              filters={recoverFilters}
              onFiltersChange={setRecoverFilters}
              selectedCleanupIds={selectedCleanupIds}
              selectedRecommendationBytes={selectedVisibleStats.bytes}
              cleanupPending={cleanupPending}
              isScanning={isScanning}
              driveStatus={driveStatus}
              activeOffloadJob={offload.activeJob}
              offloadRunning={offload.running}
              onToggleCandidate={toggleCleanupCandidate}
              onToggleAllVisible={toggleAllVisible}
              onClean={executeSelectedCleanup}
              onOffload={startSelectedOffload}
              onConnectDrive={connectGoogleDrive}
              onDisconnectDrive={disconnectGoogleDrive}
              onCancelOffload={offload.cancel}
            />
          )}
          {view === "insights" && (
            <AiInsightsView
              scanRuns={scanRuns}
              activeRun={activeRun}
              aiKeyConfigured={aiKeyConfigured}
              onApplySuggestion={applyAiSuggestion}
              onGoToSettings={() => setView("settings")}
            />
          )}
          {view === "history" && (
            <HistoryView
              cleanupHistory={cleanupHistory}
              offloadJobs={offloadJobs}
              scanRuns={scanRuns}
              historyPending={historyPending}
              restorePendingItemId={restorePendingItemId}
              onOpenRecycleBin={openRecycleBin}
              onDeleteRun={deleteRun}
              onClearScanHistory={clearScanHistory}
              onRestoreItem={restoreOffloadItem}
            />
          )}
          {view === "settings" && (
            <SettingsView
              health={health}
              driveStatus={driveStatus}
              onConnectDrive={connectGoogleDrive}
              onDisconnectDrive={disconnectGoogleDrive}
              onExportDiagnostics={exportDiagnostics}
              diagnosticsPending={diagnosticsPending}
              aiKeyConfigured={aiKeyConfigured}
              onAiKeyChanged={setAiKeyConfigured}
            />
          )}
        </section>
      </div>
    </main>
  )
}

function viewTitle(view: View) {
  if (view === "explorer") {
    return "Explorer"
  }
  if (view === "cleanup") {
    return "Recover space"
  }
  if (view === "insights") {
    return "AI insights"
  }
  if (view === "history") {
    return "History"
  }
  if (view === "settings") {
    return "Settings"
  }
  return "Overview"
}

export default App
