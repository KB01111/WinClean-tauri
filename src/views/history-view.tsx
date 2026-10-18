import { useState } from "react"
import { ChevronDown, ChevronRight, FolderOpen, Loader2, Trash2 } from "lucide-react"

import { Button } from "@/components/ui/button"
import { EmptyState, Panel, ProgressBar, StatusBadge } from "@/components/common"
import { formatBytes, formatDateTime } from "@/lib/format"
import { getOffloadProgress } from "@/lib/recover-space"
import { OffloadItemRow } from "@/views/recover-space-view"
import type {
  CleanupActionRecord,
  OffloadJobStatus,
  ScanRunRecord,
} from "@/lib/api"

const ACTION_LABELS: Record<string, string> = {
  recycle_bin: "Recycle Bin",
  google_drive_offload: "Drive offload",
  drive_restore: "Drive restore",
}

export function HistoryView({
  cleanupHistory,
  offloadJobs,
  scanRuns,
  historyPending,
  restorePendingItemId,
  onOpenRecycleBin,
  onDeleteRun,
  onClearScanHistory,
  onRestoreItem,
}: {
  cleanupHistory: CleanupActionRecord[]
  offloadJobs: OffloadJobStatus[]
  scanRuns: ScanRunRecord[]
  historyPending: boolean
  restorePendingItemId: number | null
  onOpenRecycleBin: () => void
  onDeleteRun: (runId: number) => void
  onClearScanHistory: () => void
  onRestoreItem: (itemId: number) => void
}) {
  const clearableScanCount = scanRuns.filter(
    (run) => run.status !== "running"
  ).length

  return (
    <div className="grid gap-4 xl:grid-cols-2 xl:items-start">
      <div className="space-y-4">
        <Panel title="Offload batches">
          {offloadJobs.length === 0 ? (
            <EmptyState text="No Google Drive offloads recorded yet" />
          ) : (
            <div className="space-y-2">
              {offloadJobs.map((job) => (
                <OffloadHistoryCard
                  key={job.id}
                  job={job}
                  restorePendingItemId={restorePendingItemId}
                  onRestoreItem={onRestoreItem}
                />
              ))}
            </div>
          )}
        </Panel>

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
                className="flex items-center justify-between gap-3 border-b border-border/60 py-3 text-sm"
              >
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="truncate font-medium">
                      {action.targetPath}
                    </span>
                  </div>
                  <div className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
                    <span>
                      {ACTION_LABELS[action.actionType] ?? action.actionType}
                    </span>
                    <StatusBadge status={action.status} />
                    <span>{formatDateTime(action.createdAtMs)}</span>
                  </div>
                  {action.explanation ? (
                    <div
                      className="mt-0.5 truncate text-xs text-muted-foreground"
                      title={action.explanation}
                    >
                      {action.explanation}
                    </div>
                  ) : null}
                </div>
              </div>
            ))
          )}
        </Panel>
      </div>

      <Panel
        title="Completed scans"
        action={
          <Button
            variant="outline"
            size="sm"
            aria-label="Clear completed scans"
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
                className="flex items-center justify-between gap-3 border-b border-border/60 py-3 text-sm"
              >
                <div className="min-w-0">
                  <div className="truncate font-medium">{run.rootPath}</div>
                  <div className="mt-0.5 flex items-center gap-2 text-xs text-muted-foreground">
                    <StatusBadge status={run.status} />
                    <span>
                      {formatBytes(run.bytesIndexed)} ·{" "}
                      {formatDateTime(run.finishedAtMs)}
                    </span>
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

function OffloadHistoryCard({
  job,
  restorePendingItemId,
  onRestoreItem,
}: {
  job: OffloadJobStatus
  restorePendingItemId: number | null
  onRestoreItem: (itemId: number) => void
}) {
  const [expanded, setExpanded] = useState(false)
  const progress = getOffloadProgress(job)
  const running = job.status === "queued" || job.status === "running"
  return (
    <div className="rounded-lg border border-border bg-card">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={() => setExpanded((value) => !value)}
        className="flex w-full items-center justify-between gap-3 p-3 text-left text-sm"
      >
        <span className="flex min-w-0 items-center gap-2">
          {expanded ? (
            <ChevronDown className="size-4 shrink-0" aria-hidden="true" />
          ) : (
            <ChevronRight className="size-4 shrink-0" aria-hidden="true" />
          )}
          <span className="truncate font-medium">
            Batch {job.id} · {job.destinationLabel}
          </span>
          <StatusBadge status={job.status} />
          <span className="inline-flex shrink-0 items-center rounded-full bg-muted px-2 py-0.5 text-xs font-medium text-muted-foreground tabular-nums">
            {progress.percent}%
          </span>
        </span>
        <span className="shrink-0 text-xs text-muted-foreground tabular-nums">
          {job.completedItems}/{job.totalItems} ·{" "}
          {formatBytes(job.uploadedBytes)}
        </span>
      </button>
      {running ? (
        <div className="px-3 pb-2">
          <ProgressBar percent={progress.percent} />
        </div>
      ) : null}
      {expanded ? (
        <div className="space-y-1.5 border-t border-border/60 p-3">
          <div className="text-xs text-muted-foreground">
            Started {formatDateTime(job.startedAtMs)} · Finished{" "}
            {formatDateTime(job.completedAtMs)}
          </div>
          {job.lastError ? (
            <p className="text-xs text-destructive">{job.lastError}</p>
          ) : null}
          {job.items.map((item) => (
            <OffloadItemRow
              key={item.id}
              item={item}
              onRestore={onRestoreItem}
              restorePending={restorePendingItemId === item.id}
            />
          ))}
        </div>
      ) : null}
    </div>
  )
}
