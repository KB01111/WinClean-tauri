import { Cloud, ExternalLink, Loader2, Trash2, UploadCloud } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  EmptyState,
  Metric,
  Notice,
  Panel,
  ProgressBar,
  StatusBadge,
  TextInput,
} from "@/components/common"
import { cleanupActionDisabled } from "@/lib/app-state"
import { api, formatError } from "@/lib/api"
import { formatBytes } from "@/lib/format"
import {
  getOffloadActionState,
  getOffloadProgress,
  type RecoverMode,
} from "@/lib/recover-space"
import type {
  GoogleDriveStatus,
  OffloadJobStatus,
  RecommendationCandidate,
  RecommendationPlan,
} from "@/lib/api"
import { cn } from "@/lib/utils"
import { toast } from "sonner"

export type RecoverFilterState = {
  minSizeMb: string
  minAgeDays: string
}

export function RecoverSpaceView({
  recommendationPlan,
  cleanupWarnings,
  visibleRecommendations,
  recoverMode,
  onRecoverModeChange,
  filters,
  onFiltersChange,
  selectedCleanupIds,
  selectedRecommendationBytes,
  cleanupPending,
  isScanning,
  driveStatus,
  activeOffloadJob,
  offloadRunning,
  onToggleCandidate,
  onToggleAllVisible,
  onClean,
  onOffload,
  onConnectDrive,
  onDisconnectDrive,
  onCancelOffload,
}: {
  recommendationPlan: RecommendationPlan | null
  cleanupWarnings: string[]
  visibleRecommendations: RecommendationCandidate[]
  recoverMode: RecoverMode
  onRecoverModeChange: (mode: RecoverMode) => void
  filters: RecoverFilterState
  onFiltersChange: (filters: RecoverFilterState) => void
  selectedCleanupIds: Set<string>
  selectedRecommendationBytes: number
  cleanupPending: boolean
  isScanning: boolean
  driveStatus: GoogleDriveStatus | null
  activeOffloadJob: OffloadJobStatus | null
  offloadRunning: boolean
  onToggleCandidate: (id: string) => void
  onToggleAllVisible: (select: boolean) => void
  onClean: () => void
  onOffload: () => void
  onConnectDrive: () => void
  onDisconnectDrive: () => void
  onCancelOffload: () => void
}) {
  const offloadActionState = getOffloadActionState({
    selectedCount: visibleRecommendations.filter((candidate) =>
      selectedCleanupIds.has(candidate.id)
    ).length,
    driveStatus,
    running: offloadRunning,
  })
  const selectedCount = visibleRecommendations.filter((candidate) =>
    selectedCleanupIds.has(candidate.id)
  ).length
  const allVisibleSelected =
    visibleRecommendations.length > 0 &&
    selectedCount === visibleRecommendations.length
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
      <div className="grid gap-3 md:grid-cols-3">
        <Metric
          label="Clean"
          value={formatBytes(recommendationPlan?.cleanBytes ?? 0)}
          hint="Safe to recycle"
        />
        <Metric
          label="Offload"
          value={formatBytes(recommendationPlan?.offloadBytes ?? 0)}
          hint="Move to Google Drive"
        />
        <Metric
          label="Selected"
          value={formatBytes(selectedRecommendationBytes)}
          hint={`${selectedCount} item(s)`}
        />
      </div>

      <Panel
        title="Recover space"
        action={
          <div className="flex flex-wrap items-center gap-2">
            {recoverMode === "offload" && (
              <div className="flex items-center gap-2">
                {driveStatus?.connected && driveStatus.accountEmail ? (
                  <span
                    className="max-w-48 truncate text-xs text-muted-foreground"
                    title={driveStatus.accountEmail}
                  >
                    {driveStatus.accountEmail}
                  </span>
                ) : null}
                <Button
                  variant="outline"
                  onClick={
                    driveStatus?.connected ? onDisconnectDrive : onConnectDrive
                  }
                >
                  <Cloud className="size-4" aria-hidden="true" />
                  {driveStatus?.connected
                    ? "Disconnect Drive"
                    : "Connect Drive"}
                </Button>
              </div>
            )}
            {primaryAction}
          </div>
        }
      >
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
          <div className="flex flex-wrap gap-2">
            {(["clean", "offload", "review"] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                onClick={() => onRecoverModeChange(mode)}
                className={cn(
                  "rounded-md border px-3 py-2 text-sm font-medium capitalize transition-colors",
                  recoverMode === mode
                    ? "border-primary/60 bg-accent text-accent-foreground"
                    : "border-border bg-card text-muted-foreground hover:border-ring/60"
                )}
              >
                {mode}
              </button>
            ))}
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <label className="flex items-center gap-1.5">
              Min size (MB)
              <TextInput
                value={filters.minSizeMb}
                onChange={(event) =>
                  onFiltersChange({ ...filters, minSizeMb: event.target.value })
                }
                placeholder="0"
                inputMode="numeric"
                className="h-8 w-20"
              />
            </label>
            <label className="flex items-center gap-1.5">
              Min age (days)
              <TextInput
                value={filters.minAgeDays}
                onChange={(event) =>
                  onFiltersChange({
                    ...filters,
                    minAgeDays: event.target.value,
                  })
                }
                placeholder="0"
                inputMode="numeric"
                className="h-8 w-20"
              />
            </label>
          </div>
        </div>

        {recommendationPlan?.warnings.map((warning) => (
          <Notice key={warning} tone="info" text={warning} />
        ))}
        {recoverMode === "clean"
          ? cleanupWarnings.map((warning) => (
              <Notice key={warning} tone="info" text={warning} />
            ))
          : null}
        {recoverMode === "offload" && driveStatus?.error ? (
          <Notice tone="warning" text={driveStatus.error} />
        ) : null}
        {recoverMode === "offload" && offloadActionState.reason ? (
          <Notice tone="warning" text={offloadActionState.reason} />
        ) : null}

        {activeOffloadJob ? (
          <OffloadJobCard
            job={activeOffloadJob}
            onCancel={onCancelOffload}
          />
        ) : null}

        <div className="mt-3 max-h-[560px] overflow-auto">
          {!recommendationPlan || visibleRecommendations.length === 0 ? (
            <EmptyState text="No candidates found for this mode" />
          ) : (
            <>
              <label className="flex items-center gap-3 border-b border-border py-2 text-xs font-medium text-muted-foreground">
                <input
                  type="checkbox"
                  className="ml-[3px] accent-primary"
                  checked={allVisibleSelected}
                  onChange={(event) => onToggleAllVisible(event.target.checked)}
                />
                Select all visible ({visibleRecommendations.length})
              </label>
              {visibleRecommendations.map((candidate) => (
                <label
                  key={candidate.id}
                  className="grid cursor-pointer grid-cols-[24px_1fr_150px_90px] items-center gap-3 border-b border-border/60 py-3 text-sm"
                >
                  <input
                    type="checkbox"
                    className="accent-primary"
                    checked={selectedCleanupIds.has(candidate.id)}
                    onChange={() => onToggleCandidate(candidate.id)}
                  />
                  <span className="min-w-0">
                    <span className="block truncate font-medium">
                      {candidate.name}
                    </span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {candidate.path}
                    </span>
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {candidate.reason} · {candidate.riskLevel}
                  </span>
                  <span className="text-right tabular-nums">
                    {formatBytes(candidate.size)}
                  </span>
                </label>
              ))}
            </>
          )}
        </div>
      </Panel>
    </div>
  )
}

export function OffloadJobCard({
  job,
  onCancel,
}: {
  job: OffloadJobStatus
  onCancel?: () => void
}) {
  const progress = getOffloadProgress(job)
  const running = job.status === "queued" || job.status === "running"
  return (
    <div className="my-3 rounded-lg border border-border bg-card p-3">
      <div className="flex items-center justify-between gap-3 text-sm">
        <span className="flex min-w-0 items-center gap-2 font-medium">
          <span className="truncate">Batch {job.id}</span>
          <StatusBadge status={job.status} />
        </span>
        <span className="tabular-nums">{progress.percent}%</span>
      </div>
      <ProgressBar percent={progress.percent} className="mt-2" />
      <div className="mt-2 flex items-center justify-between gap-3 text-xs text-muted-foreground">
        <span>
          {progress.text} · {formatBytes(job.uploadedBytes)} of{" "}
          {formatBytes(job.totalBytes)}
        </span>
        {running && onCancel ? (
          <button
            type="button"
            onClick={onCancel}
            className="font-medium text-destructive hover:underline"
          >
            Cancel
          </button>
        ) : null}
      </div>
      {job.lastError ? (
        <p className="mt-2 text-xs text-destructive">{job.lastError}</p>
      ) : null}
      {job.items.length > 0 ? (
        <div className="mt-3 max-h-56 space-y-1.5 overflow-auto border-t border-border/60 pt-2">
          {job.items.map((item) => (
            <OffloadItemRow key={item.id} item={item} />
          ))}
        </div>
      ) : null}
    </div>
  )
}

export function OffloadItemRow({
  item,
  onRestore,
  restorePending,
}: {
  item: OffloadJobStatus["items"][number]
  onRestore?: (itemId: number) => void
  restorePending?: boolean
}) {
  const fileName = item.sourcePath.split(/[\\/]/).pop() ?? item.sourcePath
  const uploading = item.status === "uploading"
  const percent =
    item.size > 0 ? Math.round((item.uploadedBytes / item.size) * 100) : 0
  return (
    <div className="rounded-md px-1 py-1 text-xs">
      <div className="flex items-center justify-between gap-2">
        <span
          className="min-w-0 truncate font-medium"
          title={item.sourcePath}
        >
          {fileName}
        </span>
        <span className="flex shrink-0 items-center gap-2">
          <span className="tabular-nums text-muted-foreground">
            {formatBytes(item.size)}
          </span>
          <StatusBadge status={item.status} />
          {item.driveWebUrl ? (
            <button
              type="button"
              title="Open in Google Drive"
              className="text-primary hover:text-primary/80"
              onClick={() =>
                void api
                  .openUrl(item.driveWebUrl ?? "")
                  .catch((error) => toast.error(formatError(error)))
              }
            >
              <ExternalLink className="size-3.5" aria-hidden="true" />
            </button>
          ) : null}
          {onRestore && item.driveFileId && item.recycleStatus !== "restored" ? (
            <Button
              variant="outline"
              size="xs"
              disabled={restorePending}
              onClick={() => onRestore(item.id)}
            >
              Restore
            </Button>
          ) : null}
        </span>
      </div>
      {uploading ? <ProgressBar percent={percent} className="mt-1 h-1" /> : null}
      {item.error ? (
        <p className="mt-1 truncate text-destructive" title={item.error}>
          {item.error}
        </p>
      ) : null}
      {item.recycleStatus === "restored" ? (
        <p className="mt-1 text-muted-foreground">Restored to original location</p>
      ) : null}
    </div>
  )
}
