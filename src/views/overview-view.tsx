import { Download, Flame, ListChecks, Loader2, Sparkles } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  CapacityBar,
  DataTable,
  DistributionBar,
  EmptyState,
  Metric,
  Panel,
  StatusBadge,
  SummaryList,
} from "@/components/common"
import { formatBytes, formatDateTime, formatNumber } from "@/lib/format"
import { diskUsage, reclaimGroups, volumeForRoot } from "@/lib/reclaim"
import type { View } from "@/lib/views"
import type {
  CategorySummary,
  ExtensionSummary,
  FileSearchResult,
  RecommendationPlan,
  ScanProgress,
  ScanRunRecord,
  TopDirectory,
  VolumeInfo,
} from "@/lib/api"

/** Display scan totals, large files and directories, scan history, and file-type distributions. */
export function OverviewView({
  progress,
  scanRuns,
  activeRun,
  topDirectories,
  topFiles,
  staleFiles,
  categories,
  extensions,
  recommendationPlan,
  volumes,
  onNavigate,
  onSelectRun,
  onOpenDirectory,
  onExportReport,
  reportPending,
}: {
  progress: ScanProgress
  scanRuns: ScanRunRecord[]
  activeRun: ScanRunRecord | null
  topDirectories: TopDirectory[]
  topFiles: FileSearchResult[]
  staleFiles: FileSearchResult[]
  categories: CategorySummary[]
  extensions: ExtensionSummary[]
  recommendationPlan: RecommendationPlan | null
  volumes: VolumeInfo[]
  onNavigate: (view: View) => void
  onSelectRun: (run: ScanRunRecord) => void
  onOpenDirectory: (path: string) => void
  onExportReport: () => void
  reportPending: boolean
}) {
  // Show live counts while a scan runs (even when they are legitimately 0)
  // or when the progress snapshot belongs to the selected run; otherwise show
  // the selected run's stored totals.
  const showLiveProgress =
    progress.state === "running" ||
    (progress.state !== "idle" &&
      progress.runId != null &&
      progress.runId === activeRun?.id)
  const indexedBytes = showLiveProgress
    ? progress.bytesIndexed
    : (activeRun?.bytesIndexed ?? 0)
  return (
    <div className="grid gap-4 xl:grid-cols-[1fr_360px]">
      <div className="space-y-4">
        <div className="grid gap-3 md:grid-cols-3 xl:grid-cols-5">
          <Metric
            label="Files"
            value={formatNumber(
              showLiveProgress
                ? progress.filesIndexed
                : (activeRun?.filesIndexed ?? 0)
            )}
          />
          <Metric
            label="Folders"
            value={formatNumber(
              showLiveProgress
                ? progress.directoriesIndexed
                : (activeRun?.directoriesIndexed ?? 0)
            )}
          />
          <Metric label="Indexed" value={formatBytes(indexedBytes)} />
          <Metric
            label="Recoverable"
            value={formatBytes(
              (recommendationPlan?.cleanBytes ?? 0) +
                (recommendationPlan?.offloadBytes ?? 0)
            )}
          />
          <Metric
            label="Denied"
            value={formatNumber(
              showLiveProgress
                ? progress.accessDeniedCount
                : (activeRun?.accessDeniedCount ?? 0)
            )}
            hint="access denied paths"
          />
        </div>

        <DiskUsagePanel volumes={volumes} activeRun={activeRun} />

        <ReclaimPanel plan={recommendationPlan} onNavigate={onNavigate} />

        <Panel
          title="Storage hogs"
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
          {topDirectories.length === 0 ? (
            <EmptyState text="Run a scan to find what is eating your disk" />
          ) : (
            <StorageHogBars
              directories={topDirectories.slice(0, 8)}
              bytesIndexed={activeRun?.bytesIndexed ?? 0}
              onOpenDirectory={onOpenDirectory}
            />
          )}
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

        <Panel title="Large files untouched for 6+ months">
          <DataTable
            headers={["File", "Size", "Last modified"]}
            empty="No stale large files found"
            rows={staleFiles
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
                    "w-full rounded-lg border p-3 text-left transition-colors",
                    activeRun?.id === run.id
                      ? "border-primary/60 bg-accent"
                      : "border-border bg-card hover:border-ring/50",
                  ].join(" ")}
                >
                  <div className="flex items-center justify-between gap-3">
                    <span className="truncate text-sm font-semibold">
                      {run.rootPath}
                    </span>
                    <StatusBadge status={run.status} />
                  </div>
                  <div className="mt-1 text-xs text-muted-foreground">
                    {formatBytes(run.bytesIndexed)} ·{" "}
                    {formatDateTime(run.finishedAtMs)}
                  </div>
                </button>
              ))
            )}
          </div>
        </Panel>

        <Panel title="File mix">
          <DistributionBar segments={fileMixSegments(categories)} />
          {categories.length > 0 ? (
            <div className="mt-4">
              <SummaryList
                rows={categories.map((row) => [
                  row.category,
                  formatBytes(row.totalSize),
                ])}
              />
            </div>
          ) : null}
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

const CHART_SEGMENT_CLASSES = [
  "bg-chart-1",
  "bg-chart-2",
  "bg-chart-3",
  "bg-chart-4",
  "bg-chart-5",
]

/** Build chart segments for the five largest categories, grouping any remaining bytes as Other. */
function fileMixSegments(categories: CategorySummary[]) {
  const sorted = [...categories].sort((a, b) => b.totalSize - a.totalSize)
  const segments = sorted.slice(0, 5).map((row, index) => ({
    label: row.category,
    value: row.totalSize,
    className: CHART_SEGMENT_CLASSES[index],
  }))
  const remainder = sorted
    .slice(5)
    .reduce((sum, row) => sum + row.totalSize, 0)
  if (remainder > 0) {
    segments.push({
      label: "Other",
      value: remainder,
      className: "bg-muted-foreground/40",
    })
  }
  return segments
}

/** Render directory-size bars relative to the first directory, with navigation and indexed-byte shares. */
function StorageHogBars({
  directories,
  bytesIndexed,
  onOpenDirectory,
}: {
  directories: TopDirectory[]
  bytesIndexed: number
  onOpenDirectory: (path: string) => void
}) {
  const maxSize = directories[0]?.totalSize || 1
  return (
    <div className="space-y-2">
      {directories.map((directory) => (
        <button
          key={directory.path}
          type="button"
          onClick={() => onOpenDirectory(directory.path)}
          className="group block w-full text-left"
          title={`Open ${directory.path} in Explorer`}
        >
          <div className="flex items-center justify-between gap-3 text-sm">
            <span className="flex min-w-0 items-center gap-1.5">
              <Flame
                className="size-3.5 shrink-0 text-chart-1/70"
                aria-hidden="true"
              />
              <span className="truncate font-medium group-hover:text-primary">
                {directory.path}
              </span>
            </span>
            <span className="flex shrink-0 items-baseline gap-1.5">
              {bytesIndexed > 0 ? (
                <span className="text-xs text-muted-foreground tabular-nums">
                  {Math.min(100, Math.round((directory.totalSize / bytesIndexed) * 100))}% of
                  indexed
                </span>
              ) : null}
              <span className="tabular-nums">
                {formatBytes(directory.totalSize)}
              </span>
            </span>
          </div>
          <div className="mt-1 h-1.5 overflow-hidden rounded-full bg-muted">
            <div
              className="h-full rounded-full bg-chart-1 transition-colors group-hover:bg-chart-2"
              style={{
                width: `${Math.max(2, Math.round((directory.totalSize / maxSize) * 100))}%`,
              }}
            />
          </div>
        </button>
      ))}
    </div>
  )
}

/** Used and free space from Windows, and how much of the used space the scan could see. */
function DiskUsagePanel({ volumes, activeRun }: { volumes: VolumeInfo[]; activeRun: ScanRunRecord | null }) {
  // Both the volume and the scanned total come from the selected run, so they always describe the same disk.
  const volume = volumeForRoot(volumes, activeRun?.rootPath)
  const usage = diskUsage(volume, activeRun?.bytesIndexed ?? 0)
  if (!volume || !usage) return null
  return (
    <Panel title={`Disk ${volume.rootPath}`}>
      <CapacityBar used={usage.used} total={usage.total} label={`Space used on ${volume.rootPath}`} />
      <div className="mt-2 flex flex-wrap justify-between gap-x-4 text-sm">
        <span>
          {formatBytes(usage.used)} used of {formatBytes(usage.total)}
        </span>
        <span className="text-muted-foreground">{formatBytes(usage.free)} free</span>
      </div>
      {usage.unaccounted > usage.total * 0.02 ? (
        <p className="mt-2 text-xs text-muted-foreground">
          The scan accounts for {formatBytes(usage.scanned)}. About {formatBytes(usage.unaccounted)} is used by files WinClean cannot list — system files (pagefile, hibernation, Windows Update store), snapshots, or folders that denied access.
        </p>
      ) : null}
    </Panel>
  )
}

const KIND_LABEL = { clean: "Recycle Bin", offload: "Google Drive", review: "Your judgment" } as const

/** Where recoverable space comes from, by the rule that found it. */
function ReclaimPanel({ plan, onNavigate }: { plan: RecommendationPlan | null; onNavigate: (view: View) => void }) {
  const groups = reclaimGroups(plan?.candidates ?? [])
  const largest = Math.max(1, ...groups.map((g) => g.bytes))
  return (
    <Panel
      title="What you can reclaim"
      action={
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={() => onNavigate("cleanup")}>
            <ListChecks className="size-4" aria-hidden="true" />
            Recover space
          </Button>
          <Button variant="outline" size="sm" onClick={() => onNavigate("insights")}>
            <Sparkles className="size-4" aria-hidden="true" />
            Ask AI
          </Button>
        </div>
      }
    >
      {groups.length === 0 ? (
        <EmptyState text={plan ? "Nothing worth cleaning was found" : "Run a scan to see what can be reclaimed"} />
      ) : (
        <div className="space-y-2">
          {groups.map((group) => (
            <div key={`${group.kind}-${group.reason}`}>
              <div className="flex items-center justify-between gap-3 text-sm">
                <span className="flex min-w-0 items-center gap-2">
                  <span className="truncate font-medium">{group.reason}</span>
                  <StatusBadge status={group.kind} />
                </span>
                <span className="shrink-0 tabular-nums">
                  {formatBytes(group.bytes)} <span className="text-xs text-muted-foreground">· {formatNumber(group.count)} · {KIND_LABEL[group.kind]}</span>
                </span>
              </div>
              <div className="mt-1 h-1.5 overflow-hidden rounded-full bg-muted">
                <div className={group.kind === "clean" ? "h-full rounded-full bg-chart-2" : group.kind === "offload" ? "h-full rounded-full bg-chart-1" : "h-full rounded-full bg-muted-foreground/50"} style={{ width: `${Math.max(2, Math.round((group.bytes / largest) * 100))}%` }} />
              </div>
            </div>
          ))}
        </div>
      )}
    </Panel>
  )
}
