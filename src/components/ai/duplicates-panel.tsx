import { useCallback, useEffect, useState } from "react"
import { Copy, FolderOpen, Loader2, Square } from "lucide-react"
import { toast } from "sonner"

import { EmptyState, Metric, Notice, Panel } from "@/components/common"
import { Button } from "@/components/ui/button"
import { usePolling } from "@/hooks/use-polling"
import { api, formatError, type DuplicateGroup, type DuplicateJobStatus, type DuplicateReport, type ScanRunRecord } from "@/lib/api"
import { formatBytes } from "@/lib/format"

const SHOWN_GROUPS = 12

function GroupRow({ group }: { group: DuplicateGroup }) {
  return (
    <div className="rounded-lg border border-border bg-card p-3">
      <div className="flex flex-wrap items-center justify-between gap-2 text-sm">
        <span className="font-medium">
          {group.files.length} copies · {formatBytes(group.size)} each
        </span>
        <span className="text-xs text-muted-foreground">
          {formatBytes(group.reclaimableBytes)} reclaimable{group.fingerprint ? ` · fingerprint ${group.fingerprint}` : ""}
        </span>
      </div>
      <ul className="mt-2 space-y-1">
        {group.files.map((file) => (
          <li key={file.entryId} className="flex items-center justify-between gap-2 text-xs">
            <span className="min-w-0 truncate" title={file.path}>
              {file.path}
            </span>
            <Button variant="ghost" size="sm" onClick={() => void api.revealInExplorer(file.path).catch((e) => toast.error(formatError(e)))} aria-label={`Show ${file.name} in Explorer`}>
              <FolderOpen className="size-3.5" aria-hidden="true" />
            </Button>
          </li>
        ))}
      </ul>
    </div>
  )
}

/**
 * Duplicate finder. Same size is free from the index; contents are only read when you
 * click, within a budget, at background priority, and never while on battery.
 * WinClean never deletes duplicates itself: you decide which copy to keep.
 */
export function DuplicatesPanel({ scanRuns, activeRun }: { scanRuns: ScanRunRecord[]; activeRun: ScanRunRecord | null }) {
  const completed = scanRuns.filter((run) => run.status === "completed")
  const runId = (activeRun?.status === "completed" ? activeRun.id : null) ?? completed[0]?.id ?? null
  const [report, setReport] = useState<DuplicateReport | null>(null)
  const [job, setJob] = useState<DuplicateJobStatus | null>(null)
  const [error, setError] = useState<string | null>(null)

  const loadReport = useCallback(async () => {
    if (runId === null) return
    try {
      setReport(await api.duplicateReport(runId))
    } catch (e) {
      setError(formatError(e))
    }
  }, [runId])

  useEffect(() => {
    setReport(null)
    setError(null)
    void loadReport()
    api.duplicatesStatus().then(setJob).catch(() => {})
  }, [loadReport])

  const running = job?.state === "running"
  usePolling(async () => {
    const next = await api.duplicatesStatus()
    setJob(next)
    if (next.state !== "running") await loadReport()
  }, running ? 700 : null)

  async function start() {
    setError(null)
    try {
      setJob(await api.duplicatesStart(runId))
    } catch (e) {
      setError(formatError(e))
    }
  }

  if (runId === null) {
    return (
      <Panel title="Duplicate files">
        <EmptyState text="Run a scan first" />
      </Panel>
    )
  }

  return (
    <div className="space-y-4">
      <Panel
        title="Duplicate files"
        action={
          running ? (
            <Button variant="outline" onClick={() => void api.duplicatesCancel().then(setJob)}>
              <Square className="size-4" aria-hidden="true" />
              Stop
            </Button>
          ) : (
            <Button onClick={() => void start()}>
              <Copy className="size-4" aria-hidden="true" />
              Compare file contents
            </Button>
          )
        }
      >
        <p className="text-sm text-muted-foreground">
          Files of identical size are listed from your scan for free. Comparing contents reads files, so it only runs when you ask, reads at most 4 GB, runs at background priority and stops if the PC goes onto battery. Protected locations and excluded folders are never read.
        </p>
        {running ? (
          <p className="mt-3 flex items-center gap-2 text-sm">
            <Loader2 className="size-4 animate-spin" aria-hidden="true" />
            Compared {job?.filesHashed ?? 0} file(s), {formatBytes(job?.bytesRead ?? 0)} read…
          </p>
        ) : null}
        {job?.state === "completed" && job.summary ? (
          <p className="mt-3 text-xs text-muted-foreground">
            Last check: {job.summary.filesHashed} file(s) compared across {job.summary.groupsExamined} group(s)
            {job.summary.stopped ? ` — ${job.summary.stopped}` : job.summary.budgetExhausted ? " — read budget used up; run again to continue" : ""}.
          </p>
        ) : null}
        {job?.state === "failed" ? <Notice tone="danger" text={job.error ?? "The duplicate check failed."} /> : null}
        {error ? <Notice tone="danger" text={error} /> : null}
      </Panel>

      {report ? (
        <>
          <div className="grid gap-3 md:grid-cols-3">
            <Metric label="Identical copies" value={formatBytes(report.confirmedReclaimableBytes)} hint="contents verified" />
            <Metric label="Possibly identical" value={formatBytes(report.possibleReclaimableBytes)} hint="same size only" />
            <Metric label="Protected" value={String(report.protectedFiles)} hint="never listed" />
          </div>
          <Panel title={`Verified duplicates (${report.confirmed.length})`}>
            {report.confirmed.length === 0 ? (
              <EmptyState text="No verified duplicates yet — compare file contents to find them" />
            ) : (
              <div className="space-y-2">
                {report.confirmed.slice(0, SHOWN_GROUPS).map((group, index) => (
                  <GroupRow key={`c-${group.fingerprint}-${index}`} group={group} />
                ))}
              </div>
            )}
          </Panel>
          {report.possible.length > 0 ? (
            <Panel title={`Same size, not compared (${report.possible.length})`}>
              <div className="space-y-2">
                {report.possible.slice(0, SHOWN_GROUPS).map((group, index) => (
                  <GroupRow key={`p-${index}`} group={group} />
                ))}
              </div>
            </Panel>
          ) : null}
          <ul className="list-disc space-y-0.5 pl-5 text-xs text-muted-foreground">
            {report.notes.map((note) => (
              <li key={note}>{note}</li>
            ))}
          </ul>
        </>
      ) : null}
    </div>
  )
}
