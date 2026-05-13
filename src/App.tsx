import {
  AlertTriangle,
  BarChart3,
  CheckCircle2,
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
  RotateCcw,
  Search,
  Settings,
  ShieldCheck,
  Square,
  Trash2,
} from "lucide-react"
import { useCallback, useEffect, useMemo, useState } from "react"
import { invoke } from "@tauri-apps/api/core"

import { Button } from "@/components/ui/button"
import {
  formatBytes,
  formatDateTime,
  formatNumber,
  formatPercentUsed,
  statusText,
} from "@/lib/format"

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
  localDiagnosticsOnly: boolean
  recycleBinCleanup: boolean
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
  const [fileQuery, setFileQuery] = useState("")
  const [extensionFilter, setExtensionFilter] = useState("")
  const [extensions, setExtensions] = useState<ExtensionSummary[]>([])
  const [categories, setCategories] = useState<CategorySummary[]>([])
  const [cleanupPlan, setCleanupPlan] = useState<CleanupPlan | null>(null)
  const [selectedCleanupIds, setSelectedCleanupIds] = useState<Set<string>>(
    new Set()
  )
  const [cleanupHistory, setCleanupHistory] = useState<CleanupActionRecord[]>(
    []
  )
  const [health, setHealth] = useState<AppHealth | null>(null)
  const [message, setMessage] = useState<string | null>(null)
  const [loadingVolumes, setLoadingVolumes] = useState(true)
  const [loadingData, setLoadingData] = useState(false)

  const selectedVolume = useMemo(
    () => volumes.find((volume) => volume.rootPath === selectedRoot),
    [selectedRoot, volumes]
  )
  const activeRun = useMemo(
    () =>
      scanRuns.find((run) => run.id === selectedRunId) ?? scanRuns[0] ?? null,
    [scanRuns, selectedRunId]
  )
  const isScanning = progress.state === "running"
  const selectedCleanupBytes = useMemo(() => {
    if (!cleanupPlan) {
      return 0
    }
    return cleanupPlan.candidates
      .filter((candidate) => selectedCleanupIds.has(candidate.id))
      .reduce((sum, candidate) => sum + candidate.size, 0)
  }, [cleanupPlan, selectedCleanupIds])

  const refreshRunData = useCallback(
    async (runId?: number | null, parentPath?: string | null) => {
      const resolvedRunId = runId ?? selectedRunId ?? null
      if (!resolvedRunId) {
        setTopDirectories([])
        setDirectoryChildren([])
        setFiles([])
        setExtensions([])
        setCategories([])
        setCleanupPlan(null)
        return
      }

      const [top, children, foundFiles, extensionRows, categoryRows, plan] =
        await Promise.all([
          invoke<TopDirectory[]>("top_directories", {
            runId: resolvedRunId,
            limit: 25,
          }),
          invoke<DirectoryChild[]>("directory_children", {
            runId: resolvedRunId,
            parentPath: parentPath ?? null,
            limit: 250,
          }),
          invoke<FileSearchResult[]>("file_search", {
            runId: resolvedRunId,
            query: fileQuery || null,
            extension: extensionFilter || null,
            minSize: null,
            limit: 100,
          }),
          invoke<ExtensionSummary[]>("extension_summary", {
            runId: resolvedRunId,
            limit: 20,
          }),
          invoke<CategorySummary[]>("category_summary", {
            runId: resolvedRunId,
          }),
          invoke<CleanupPlan | null>("cleanup_plan", {
            runId: resolvedRunId,
            limit: 100,
          }),
        ])
      setTopDirectories(top)
      setDirectoryChildren(children)
      setFiles(foundFiles)
      setExtensions(extensionRows)
      setCategories(categoryRows)
      setCleanupPlan(plan)
      setSelectedCleanupIds(new Set())
    },
    [extensionFilter, fileQuery, selectedRunId]
  )

  const refreshDashboard = useCallback(async () => {
    setLoadingData(true)
    try {
      const [nextProgress, runs, historyRows, nextHealth] = await Promise.all([
        invoke<ScanProgress>("scan_status"),
        invoke<ScanRunRecord[]>("scan_runs", { limit: 30 }),
        invoke<CleanupActionRecord[]>("cleanup_history", { limit: 50 }),
        invoke<AppHealth>("app_health"),
      ])
      setProgress(nextProgress)
      setScanRuns(runs)
      setCleanupHistory(historyRows)
      setHealth(nextHealth)
      const nextRunId =
        selectedRunId ?? nextProgress.runId ?? runs[0]?.id ?? null
      setSelectedRunId(nextRunId)
      const nextRun = runs.find((run) => run.id === nextRunId)
      const nextDirectory = currentDirectory ?? nextRun?.rootPath ?? null
      setCurrentDirectory(nextDirectory)
      await refreshRunData(nextRunId, nextDirectory)
      setMessage(null)
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setLoadingData(false)
    }
  }, [currentDirectory, refreshRunData, selectedRunId])

  const refreshVolumes = useCallback(async () => {
    setLoadingVolumes(true)
    try {
      const nextVolumes = await invoke<VolumeInfo[]>("list_volumes")
      setVolumes(nextVolumes)
      setSelectedRoot((current) => current || nextVolumes[0]?.rootPath || "")
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setLoadingVolumes(false)
    }
  }, [])

  useEffect(() => {
    void refreshVolumes()
    void refreshDashboard()
  }, [refreshDashboard, refreshVolumes])

  useEffect(() => {
    if (!isScanning) {
      return
    }
    const timer = window.setInterval(() => {
      void refreshDashboard()
    }, 900)
    return () => window.clearInterval(timer)
  }, [isScanning, refreshDashboard])

  useEffect(() => {
    if (!activeRun) {
      return
    }
    const timeout = window.setTimeout(() => {
      void refreshRunData(activeRun.id, currentDirectory ?? activeRun.rootPath)
    }, 200)
    return () => window.clearTimeout(timeout)
  }, [activeRun, currentDirectory, extensionFilter, fileQuery, refreshRunData])

  async function startScan() {
    if (!selectedVolume) {
      return
    }

    setMessage(null)
    setTopDirectories([])
    setDirectoryChildren([])
    setFiles([])
    setCleanupPlan(null)
    try {
      const nextProgress = await invoke<ScanProgress>("start_scan", {
        rootPath: selectedVolume.rootPath,
        fileSystem: selectedVolume.fileSystem ?? null,
      })
      setProgress(nextProgress)
      setView("overview")
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  async function cancelScan() {
    try {
      setProgress(await invoke<ScanProgress>("cancel_scan"))
      await refreshDashboard()
    } catch (error) {
      setMessage(formatError(error))
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
    if (!cleanupPlan || selectedCleanupIds.size === 0) {
      return
    }
    try {
      const result = await invoke<{
        recycledCount: number
        failedCount: number
        bytesRequested: number
      }>("execute_cleanup", {
        runId: cleanupPlan.scanRunId,
        candidateIds: Array.from(selectedCleanupIds),
      })
      setMessage(
        `Cleanup sent ${result.recycledCount} item(s) to Recycle Bin; ${result.failedCount} failed.`
      )
      await refreshDashboard()
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  async function restoreAction(actionId: number) {
    try {
      const action = await invoke<CleanupActionRecord>(
        "restore_cleanup_action",
        { actionId }
      )
      setMessage(action.explanation ?? "Restore request recorded.")
      await refreshDashboard()
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  async function exportReport() {
    try {
      const artifact = await invoke<ExportArtifact | null>("export_report", {
        runId: activeRun?.id ?? null,
      })
      if (artifact) {
        setMessage(`Report exported to ${artifact.path}`)
      }
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  async function exportDiagnostics() {
    try {
      const artifact = await invoke<ExportArtifact>("export_diagnostics")
      setMessage(`Diagnostics exported to ${artifact.path}`)
    } catch (error) {
      setMessage(formatError(error))
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
              <p className="text-sm text-[#667066]">{statusText(progress)}</p>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="outline"
                onClick={refreshDashboard}
                disabled={loadingData}
              >
                <RefreshCw className="size-4" aria-hidden="true" />
                Refresh
              </Button>
              {isScanning ? (
                <Button variant="destructive" onClick={cancelScan}>
                  <Square className="size-4" aria-hidden="true" />
                  Cancel
                </Button>
              ) : (
                <Button
                  onClick={startScan}
                  disabled={!selectedVolume || !selectedVolume.isReady}
                >
                  <Play className="size-4" aria-hidden="true" />
                  Scan
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
              categories={categories}
              extensions={extensions}
              onSelectRun={selectRun}
              onExportReport={exportReport}
            />
          )}
          {view === "explorer" && (
            <ExplorerView
              activeRun={activeRun}
              currentDirectory={currentDirectory}
              directoryChildren={directoryChildren}
              files={files}
              fileQuery={fileQuery}
              extensionFilter={extensionFilter}
              onFileQueryChange={setFileQuery}
              onExtensionFilterChange={setExtensionFilter}
              onOpenDirectory={openDirectory}
            />
          )}
          {view === "cleanup" && (
            <CleanupView
              cleanupPlan={cleanupPlan}
              selectedCleanupIds={selectedCleanupIds}
              selectedCleanupBytes={selectedCleanupBytes}
              onToggleCandidate={toggleCleanupCandidate}
              onExecute={executeSelectedCleanup}
            />
          )}
          {view === "history" && (
            <HistoryView
              cleanupHistory={cleanupHistory}
              scanRuns={scanRuns}
              onRestore={restoreAction}
            />
          )}
          {view === "settings" && (
            <SettingsView
              health={health}
              onExportDiagnostics={exportDiagnostics}
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
  categories,
  extensions,
  onSelectRun,
  onExportReport,
}: {
  progress: ScanProgress
  scanRuns: ScanRunRecord[]
  activeRun: ScanRunRecord | null
  topDirectories: TopDirectory[]
  categories: CategorySummary[]
  extensions: ExtensionSummary[]
  onSelectRun: (run: ScanRunRecord) => void
  onExportReport: () => void
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
        </div>

        <Panel
          title="Largest directories"
          action={
            <Button
              variant="outline"
              onClick={onExportReport}
              disabled={!activeRun}
            >
              <Download className="size-4" aria-hidden="true" />
              Export
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
  directoryChildren,
  files,
  fileQuery,
  extensionFilter,
  onFileQueryChange,
  onExtensionFilterChange,
  onOpenDirectory,
}: {
  activeRun: ScanRunRecord | null
  currentDirectory: string | null
  directoryChildren: DirectoryChild[]
  files: FileSearchResult[]
  fileQuery: string
  extensionFilter: string
  onFileQueryChange: (value: string) => void
  onExtensionFilterChange: (value: string) => void
  onOpenDirectory: (path: string) => void
}) {
  return (
    <div className="grid gap-4 xl:grid-cols-[1fr_420px]">
      <Panel title="Directory browser">
        <div className="mb-3 flex items-center gap-2 rounded-md bg-[#f3f5ef] px-3 py-2 text-sm text-[#596259]">
          <FolderOpen className="size-4" aria-hidden="true" />
          <span className="truncate">
            {currentDirectory ?? activeRun?.rootPath ?? "No scan selected"}
          </span>
        </div>
        <div className="max-h-[560px] overflow-auto">
          {directoryChildren.length === 0 ? (
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
        <div className="mb-3 grid grid-cols-[1fr_96px] gap-2">
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

function CleanupView({
  cleanupPlan,
  selectedCleanupIds,
  selectedCleanupBytes,
  onToggleCandidate,
  onExecute,
}: {
  cleanupPlan: CleanupPlan | null
  selectedCleanupIds: Set<string>
  selectedCleanupBytes: number
  onToggleCandidate: (id: string) => void
  onExecute: () => void
}) {
  return (
    <div className="space-y-4">
      <div className="grid gap-3 md:grid-cols-3">
        <Metric
          label="Candidates"
          value={formatNumber(cleanupPlan?.candidates.length ?? 0)}
        />
        <Metric
          label="Potential recovery"
          value={formatBytes(cleanupPlan?.totalBytes ?? 0)}
        />
        <Metric label="Selected" value={formatBytes(selectedCleanupBytes)} />
      </div>

      <Panel
        title="Safe cleanup preview"
        action={
          <Button
            onClick={onExecute}
            disabled={!cleanupPlan || selectedCleanupIds.size === 0}
          >
            <Trash2 className="size-4" aria-hidden="true" />
            Move to Recycle Bin
          </Button>
        }
      >
        {cleanupPlan?.warnings.map((warning) => (
          <Notice key={warning} tone="info" text={warning} />
        ))}
        <div className="mt-3 max-h-[560px] overflow-auto">
          {!cleanupPlan || cleanupPlan.candidates.length === 0 ? (
            <EmptyState text="No conservative cleanup candidates found" />
          ) : (
            cleanupPlan.candidates.map((candidate) => (
              <label
                key={candidate.id}
                className="grid cursor-pointer grid-cols-[24px_1fr_130px_90px] items-center gap-3 border-b border-[#edf0ea] py-3 text-sm"
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
                  {candidate.reason}
                </span>
                <span className="text-right tabular-nums">
                  {formatBytes(candidate.size)}
                </span>
              </label>
            ))
          )}
        </div>
      </Panel>
    </div>
  )
}

function HistoryView({
  cleanupHistory,
  scanRuns,
  onRestore,
}: {
  cleanupHistory: CleanupActionRecord[]
  scanRuns: ScanRunRecord[]
  onRestore: (actionId: number) => void
}) {
  return (
    <div className="grid gap-4 xl:grid-cols-2">
      <Panel title="Cleanup history">
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
              <Button
                variant="outline"
                size="sm"
                disabled={action.status !== "recycled"}
                onClick={() => void onRestore(action.id)}
              >
                <RotateCcw className="size-4" aria-hidden="true" />
                Restore
              </Button>
            </div>
          ))
        )}
      </Panel>

      <Panel title="Completed scans">
        {scanRuns.length === 0 ? (
          <EmptyState text="No scan history recorded yet" />
        ) : (
          <DataTable
            headers={["Root", "Indexed", "Finished"]}
            rows={scanRuns.map((run) => [
              run.rootPath,
              formatBytes(run.bytesIndexed),
              formatDateTime(run.finishedAtMs),
            ])}
          />
        )}
      </Panel>
    </div>
  )
}

function SettingsView({
  health,
  onExportDiagnostics,
}: {
  health: AppHealth | null
  onExportDiagnostics: () => void
}) {
  return (
    <div className="grid gap-4 xl:grid-cols-[1fr_380px]">
      <Panel
        title="Production posture"
        action={
          <Button variant="outline" onClick={onExportDiagnostics}>
            <Download className="size-4" aria-hidden="true" />
            Export diagnostics
          </Button>
        }
      >
        <div className="grid gap-3 md:grid-cols-2">
          <Posture
            icon={ShieldCheck}
            title="Privacy"
            text="Diagnostics and scan data stay local unless exported."
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
            icon={CheckCircle2}
            title="Release"
            text="Direct Windows installer path with signing and updater readiness."
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
      <Icon className="size-4" aria-hidden="true" />
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
        <Icon className="size-4 text-[#2d6654]" aria-hidden="true" />
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
