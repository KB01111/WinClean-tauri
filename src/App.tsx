import {
  AlertTriangle,
  Database,
  FolderTree,
  HardDrive,
  Loader2,
  Play,
  RefreshCw,
  Square,
} from "lucide-react"
import { useCallback, useEffect, useMemo, useState } from "react"
import { invoke } from "@tauri-apps/api/core"

import { Button } from "@/components/ui/button"

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

type TopDirectory = {
  path: string
  totalSize: number
  fileCount: number
  directoryCount: number
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
  const [volumes, setVolumes] = useState<VolumeInfo[]>([])
  const [selectedRoot, setSelectedRoot] = useState("")
  const [progress, setProgress] = useState<ScanProgress>(emptyProgress)
  const [topDirectories, setTopDirectories] = useState<TopDirectory[]>([])
  const [message, setMessage] = useState<string | null>(null)
  const [loadingVolumes, setLoadingVolumes] = useState(true)

  const selectedVolume = useMemo(
    () => volumes.find((volume) => volume.rootPath === selectedRoot),
    [selectedRoot, volumes],
  )

  const isScanning = progress.state === "running"

  const refreshTopDirectories = useCallback(async (runId?: number | null) => {
    try {
      const rows = await invoke<TopDirectory[]>("top_directories", {
        runId: runId ?? null,
        limit: 25,
      })
      setTopDirectories(rows)
    } catch (error) {
      setMessage(formatError(error))
    }
  }, [])

  const refreshStatus = useCallback(async () => {
    try {
      const nextProgress = await invoke<ScanProgress>("scan_status")
      setProgress(nextProgress)
      if (nextProgress.state !== "running") {
        void refreshTopDirectories(nextProgress.runId)
      }
    } catch (error) {
      setMessage(formatError(error))
    }
  }, [refreshTopDirectories])

  const refreshVolumes = useCallback(async () => {
    setLoadingVolumes(true)
    try {
      const nextVolumes = await invoke<VolumeInfo[]>("list_volumes")
      setVolumes(nextVolumes)
      setSelectedRoot((current) => current || nextVolumes[0]?.rootPath || "")
      setMessage(null)
    } catch (error) {
      setMessage(formatError(error))
    } finally {
      setLoadingVolumes(false)
    }
  }, [])

  useEffect(() => {
    void refreshVolumes()
    void refreshStatus()
    void refreshTopDirectories()
  }, [refreshStatus, refreshTopDirectories, refreshVolumes])

  useEffect(() => {
    if (!isScanning) {
      return
    }

    const timer = window.setInterval(() => {
      void refreshStatus()
      void refreshTopDirectories(progress.runId)
    }, 900)

    return () => window.clearInterval(timer)
  }, [isScanning, progress.runId, refreshStatus, refreshTopDirectories])

  async function startScan() {
    if (!selectedVolume) {
      return
    }

    setMessage(null)
    setTopDirectories([])
    try {
      const nextProgress = await invoke<ScanProgress>("start_scan", {
        rootPath: selectedVolume.rootPath,
        fileSystem: selectedVolume.fileSystem ?? null,
      })
      setProgress(nextProgress)
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  async function cancelScan() {
    try {
      setProgress(await invoke<ScanProgress>("cancel_scan"))
    } catch (error) {
      setMessage(formatError(error))
    }
  }

  return (
    <main className="min-h-svh bg-[#f7f7f3] text-[#20231f]">
      <div className="mx-auto flex w-full max-w-[1440px] flex-col gap-5 px-6 py-5">
        <header className="flex flex-wrap items-center justify-between gap-4 border-b border-[#d8ddd2] pb-4">
          <div className="flex items-center gap-3">
            <div className="flex size-10 items-center justify-center rounded-md bg-[#123c2f] text-white">
              <HardDrive className="size-5" aria-hidden="true" />
            </div>
            <div>
              <h1 className="text-2xl font-semibold tracking-normal">WinClean</h1>
              <p className="text-sm text-[#657064]">Disk analyzer</p>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <Button variant="outline" onClick={refreshVolumes} disabled={loadingVolumes}>
              <RefreshCw className="size-4" aria-hidden="true" />
              Refresh
            </Button>
            {isScanning ? (
              <Button variant="destructive" onClick={cancelScan}>
                <Square className="size-4" aria-hidden="true" />
                Cancel
              </Button>
            ) : (
              <Button onClick={startScan} disabled={!selectedVolume || !selectedVolume.isReady}>
                <Play className="size-4" aria-hidden="true" />
                Scan
              </Button>
            )}
          </div>
        </header>

        <section className="grid gap-4 lg:grid-cols-[360px_1fr]">
          <aside className="flex min-h-[520px] flex-col gap-4 rounded-lg border border-[#d8ddd2] bg-white p-4 shadow-sm">
            <div className="flex items-center justify-between gap-3">
              <h2 className="text-sm font-semibold uppercase text-[#596259]">Local disks</h2>
              {loadingVolumes ? <Loader2 className="size-4 animate-spin text-[#487968]" /> : null}
            </div>

            <div className="flex flex-col gap-2">
              {volumes.map((volume) => (
                <button
                  type="button"
                  key={volume.rootPath}
                  onClick={() => setSelectedRoot(volume.rootPath)}
                  className={[
                    "flex w-full items-center gap-3 rounded-md border p-3 text-left transition",
                    selectedRoot === volume.rootPath
                      ? "border-[#2d6654] bg-[#edf6ef]"
                      : "border-[#e1e5dd] bg-white hover:border-[#b9c8bd]",
                  ].join(" ")}
                >
                  <HardDrive className="size-5 shrink-0 text-[#2d6654]" aria-hidden="true" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">{volume.rootPath}</span>
                    <span className="block truncate text-xs text-[#667066]">
                      {volume.fileSystem ?? "unknown"} · {volume.driveType}
                    </span>
                  </span>
                  <span className="text-right text-xs text-[#667066]">
                    {formatPercentUsed(volume)}
                  </span>
                </button>
              ))}
            </div>

            <div className="mt-auto grid grid-cols-2 gap-2">
              <Metric label="Files" value={formatNumber(progress.filesIndexed)} />
              <Metric label="Folders" value={formatNumber(progress.directoriesIndexed)} />
              <Metric label="Indexed" value={formatBytes(progress.bytesIndexed)} />
              <Metric label="Denied" value={formatNumber(progress.accessDeniedCount)} />
            </div>
          </aside>

          <section className="flex min-h-[520px] flex-col rounded-lg border border-[#d8ddd2] bg-white shadow-sm">
            <div className="flex flex-wrap items-start justify-between gap-3 border-b border-[#e3e7df] p-4">
              <div>
                <h2 className="flex items-center gap-2 text-lg font-semibold">
                  <FolderTree className="size-5 text-[#2d6654]" aria-hidden="true" />
                  Top directories
                </h2>
                <p className="text-sm text-[#667066]">{statusText(progress)}</p>
              </div>
              <div className="flex items-center gap-2 rounded-md bg-[#f3f5ef] px-3 py-2 text-sm">
                <Database className="size-4 text-[#2d6654]" aria-hidden="true" />
                {progress.runId ? `Run ${progress.runId}` : "No scan"}
              </div>
            </div>

            {(progress.fallbackReason || message || progress.lastError) && (
              <div className="mx-4 mt-4 flex gap-3 rounded-md border border-[#ecd48f] bg-[#fff8df] p-3 text-sm text-[#69521d]">
                <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
                <div className="space-y-1">
                  {progress.fallbackReason ? <p>{progress.fallbackReason}</p> : null}
                  {message ? <p>{message}</p> : null}
                  {progress.lastError ? <p>{progress.lastError}</p> : null}
                </div>
              </div>
            )}

            <div className="grid grid-cols-3 gap-3 p-4">
              <Metric label="State" value={progress.state} />
              <Metric label="Skipped links" value={formatNumber(progress.skippedReparseCount)} />
              <Metric label="Errors" value={formatNumber(progress.errorCount)} />
            </div>

            <div className="min-h-0 flex-1 overflow-auto px-4 pb-4">
              <table className="w-full table-fixed border-collapse text-sm">
                <thead className="sticky top-0 bg-white">
                  <tr className="border-b border-[#dfe4dc] text-left text-xs uppercase text-[#697267]">
                    <th className="w-[58%] py-2 pr-3 font-semibold">Path</th>
                    <th className="w-[18%] py-2 pr-3 text-right font-semibold">Size</th>
                    <th className="w-[12%] py-2 pr-3 text-right font-semibold">Files</th>
                    <th className="w-[12%] py-2 text-right font-semibold">Folders</th>
                  </tr>
                </thead>
                <tbody>
                  {topDirectories.length === 0 ? (
                    <tr>
                      <td className="py-10 text-center text-[#697267]" colSpan={4}>
                        {isScanning ? "Scanning..." : "No directory data yet"}
                      </td>
                    </tr>
                  ) : (
                    topDirectories.map((directory) => (
                      <tr key={directory.path} className="border-b border-[#edf0ea]">
                        <td className="truncate py-3 pr-3 font-medium" title={directory.path}>
                          {directory.path}
                        </td>
                        <td className="py-3 pr-3 text-right tabular-nums">
                          {formatBytes(directory.totalSize)}
                        </td>
                        <td className="py-3 pr-3 text-right tabular-nums">
                          {formatNumber(directory.fileCount)}
                        </td>
                        <td className="py-3 text-right tabular-nums">
                          {formatNumber(directory.directoryCount)}
                        </td>
                      </tr>
                    ))
                  )}
                </tbody>
              </table>
            </div>
          </section>
        </section>
      </div>
    </main>
  )
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md border border-[#e3e7df] bg-[#fbfcf8] p-3">
      <div className="text-xs uppercase text-[#6d756c]">{label}</div>
      <div className="mt-1 truncate text-lg font-semibold tabular-nums">{value}</div>
    </div>
  )
}

function statusText(progress: ScanProgress) {
  if (progress.state === "running") {
    return `Scanning ${progress.rootPath ?? ""}`
  }
  if (progress.state === "completed") {
    return `Completed ${progress.rootPath ?? ""}`
  }
  if (progress.state === "failed") {
    return "Scan failed"
  }
  if (progress.state === "canceled") {
    return "Scan canceled"
  }
  return "Select a drive to start"
}

function formatBytes(value: number) {
  if (!Number.isFinite(value) || value <= 0) {
    return "0 B"
  }

  const units = ["B", "KB", "MB", "GB", "TB"]
  let size = value
  let unit = 0
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024
    unit += 1
  }
  return `${size >= 10 || unit === 0 ? size.toFixed(0) : size.toFixed(1)} ${units[unit]}`
}

function formatNumber(value: number) {
  return new Intl.NumberFormat().format(value)
}

function formatPercentUsed(volume: VolumeInfo) {
  if (!volume.totalSpace || volume.availableSpace == null) {
    return "unknown"
  }
  const used = Math.max(volume.totalSpace - volume.availableSpace, 0)
  return `${Math.round((used / volume.totalSpace) * 100)}% used`
}

function formatError(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export default App
