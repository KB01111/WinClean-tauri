import { Cloud, ExternalLink, FolderOpen, Loader2, Trash2, UploadCloud } from "lucide-react"
import { useId, useRef, useState, type KeyboardEvent } from "react"

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
  cleanupConfirmCopy,
  getOffloadActionState,
  getOffloadProgress,
  getReviewRevealState,
  selectedVisibleCandidateStats,
  type CleanupConfirmCopy,
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

const CANDIDATE_PAGE_SIZE = 100

/** Reveal a recovery candidate in Windows Explorer and report request failures in a toast. */
function revealCandidateInExplorer(path: string) {
  void api
    .revealInExplorer(path)
    .catch((error) => toast.error(formatError(error)))
}

/** Render the cleanup confirmation dialog with keyboard cancellation and button focus cycling. */
function CleanupConfirmDialog({
  labelledById,
  copy,
  onCancel,
  onConfirm,
}: {
  labelledById: string
  copy: CleanupConfirmCopy
  onCancel: () => void
  onConfirm: () => void
}) {
  const panelRef = useRef<HTMLDivElement>(null)

  /** Cancel on Escape and cycle focus between dialog buttons on Tab or Shift+Tab. */
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.stopPropagation()
      onCancel()
      return
    }
    if (event.key === "Tab") {
      event.preventDefault()
      const buttons = panelRef.current
        ? Array.from(
            panelRef.current.querySelectorAll<HTMLButtonElement>("button")
          )
        : []
      if (buttons.length === 0) {
        return
      }
      const activeIndex = buttons.findIndex(
        (button) => button === document.activeElement
      )
      const nextIndex = event.shiftKey
        ? (activeIndex - 1 + buttons.length) % buttons.length
        : (activeIndex + 1) % buttons.length
      buttons[nextIndex].focus()
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-4"
      onClick={onCancel}
      onKeyDown={handleKeyDown}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledById}
        className="w-full max-w-md rounded-xl border border-border bg-card p-4 text-card-foreground shadow-lg"
        onClick={(event) => event.stopPropagation()}
      >
        <h3 id={labelledById} className="text-sm font-semibold">
          {copy.title}
        </h3>
        <p className="mt-2 text-sm text-muted-foreground">{copy.body}</p>
        <div className="mt-4 flex flex-wrap justify-end gap-2">
          <Button
            variant="outline"
            autoFocus
            className="focus-visible:ring-2 focus-visible:ring-ring/40"
            onClick={onCancel}
          >
            {copy.cancelLabel}
          </Button>
          <Button
            variant="destructive"
            className="focus-visible:ring-2 focus-visible:ring-ring/40"
            onClick={onConfirm}
          >
            <Trash2 className="size-4" aria-hidden="true" />
            {copy.confirmLabel}
          </Button>
        </div>
      </div>
    </div>
  )
}

/** Render recovery filters, candidate selection, cleanup confirmation, and Google Drive offload controls. */
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
  const confirmTitleId = useId()
  const candidateListId = useId()
  const [confirmOpen, setConfirmOpen] = useState(false)
  const candidateListKey = `${recoverMode}:${recommendationPlan?.scanRunId ?? "none"}`
  const [rowLimit, setRowLimit] = useState<{ key: string; count: number }>({
    key: candidateListKey,
    count: CANDIDATE_PAGE_SIZE,
  })
  const renderedCandidateCount =
    rowLimit.key === candidateListKey ? rowLimit.count : CANDIDATE_PAGE_SIZE
  const renderedRecommendations = visibleRecommendations.slice(
    0,
    renderedCandidateCount
  )
  const remainingCandidateCount =
    visibleRecommendations.length - renderedRecommendations.length

  const selectionStats = selectedVisibleCandidateStats(
    visibleRecommendations,
    selectedCleanupIds
  )
  const selectedCount = selectionStats.count
  const firstSelectedCandidate =
    visibleRecommendations.find(
      (candidate) => selectedCleanupIds.has(candidate.id)
    ) ?? null
  const offloadActionState = getOffloadActionState({
    selectedCount,
    driveStatus,
    running: offloadRunning,
  })
  const allVisibleSelected =
    visibleRecommendations.length > 0 &&
    selectedCount === visibleRecommendations.length
  const executeDisabled = cleanupActionDisabled({
    hasPlan: Boolean(recommendationPlan),
    selectedCount,
    pending: cleanupPending,
    scanning: isScanning,
  })
  const reviewRevealState = getReviewRevealState(selectedCount)
  const confirmCopy = cleanupConfirmCopy(selectionStats)
  const primaryAction =
    recoverMode === "clean" ? (
      <Button onClick={() => setConfirmOpen(true)} disabled={executeDisabled}>
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
    ) : (
      <div className="flex flex-wrap items-center gap-2">
        <Button
          onClick={() => {
            if (firstSelectedCandidate) {
              revealCandidateInExplorer(firstSelectedCandidate.path)
            }
          }}
          disabled={reviewRevealState.disabled}
          title={firstSelectedCandidate?.path ?? undefined}
        >
          <FolderOpen className="size-4" aria-hidden="true" />
          {reviewRevealState.label}
        </Button>
        {reviewRevealState.hint ? (
          <span className="text-xs text-muted-foreground">
            {reviewRevealState.hint}
          </span>
        ) : null}
      </div>
    )

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
          <div
            role="tablist"
            aria-label="Recover mode"
            className="flex flex-wrap gap-2"
          >
            {(["clean", "offload", "review"] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                role="tab"
                aria-selected={recoverMode === mode}
                aria-controls={candidateListId}
                onClick={() => onRecoverModeChange(mode)}
                className={cn(
                  "rounded-md border px-3 py-2 text-sm font-medium capitalize transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/40",
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

        <div id={candidateListId} className="mt-3 max-h-[560px] overflow-auto">
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
              {renderedRecommendations.map((candidate) => (
                <label
                  key={candidate.id}
                  className="grid cursor-pointer grid-cols-[24px_1fr_auto] items-center gap-3 border-b border-border/60 py-3 text-sm"
                >
                  <input
                    type="checkbox"
                    aria-label={candidate.name}
                    className="accent-primary"
                    checked={selectedCleanupIds.has(candidate.id)}
                    onChange={() => onToggleCandidate(candidate.id)}
                  />
                  <span className="min-w-0">
                    <span className="flex min-w-0 items-baseline gap-2">
                      <span className="min-w-0 truncate font-medium">
                        {candidate.name}
                      </span>
                      <span
                        className="min-w-0 flex-1 truncate text-xs text-muted-foreground"
                        title={candidate.path}
                      >
                        {candidate.path}
                      </span>
                    </span>
                    <span className="mt-0.5 flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
                      {candidate.reason ? (
                        <>
                          <span className="truncate">{candidate.reason}</span>
                          <span aria-hidden="true" className="shrink-0">
                            ·
                          </span>
                        </>
                      ) : null}
                      <span className="shrink-0 capitalize">
                        <StatusBadge status={candidate.riskLevel} />
                      </span>
                    </span>
                  </span>
                  <span className="flex shrink-0 items-center justify-end gap-2">
                    <span className="whitespace-nowrap text-right tabular-nums">
                      {formatBytes(candidate.size)}
                    </span>
                    {recoverMode === "review" ? (
                      <button
                        type="button"
                        aria-label={`Reveal ${candidate.name} in Explorer`}
                        className="rounded-sm text-primary transition-colors outline-none hover:text-primary/80 focus-visible:ring-2 focus-visible:ring-ring/40"
                        onClick={() =>
                          revealCandidateInExplorer(candidate.path)
                        }
                      >
                        <FolderOpen className="size-3.5" aria-hidden="true" />
                      </button>
                    ) : null}
                  </span>
                </label>
              ))}
              {remainingCandidateCount > 0 ? (
                <button
                  type="button"
                  className="mt-2 w-full rounded-md border border-border py-2 text-sm text-muted-foreground transition-colors outline-none hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/40"
                  onClick={() =>
                    setRowLimit({
                      key: candidateListKey,
                      count: renderedCandidateCount + CANDIDATE_PAGE_SIZE,
                    })
                  }
                >
                  Show more ({remainingCandidateCount} remaining)
                </button>
              ) : null}
            </>
          )}
        </div>
      </Panel>

      {confirmOpen ? (
        <CleanupConfirmDialog
          labelledById={confirmTitleId}
          copy={confirmCopy}
          onCancel={() => setConfirmOpen(false)}
          onConfirm={() => {
            setConfirmOpen(false)
            onClean()
          }}
        />
      ) : null}
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

/** Render an offload item’s status, upload progress, Drive link, and optional restore action. */
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
              aria-label="Open in Google Drive"
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
