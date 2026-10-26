import { useState } from "react"
import { ListChecks, Loader2, ShieldAlert, Sparkles, Target } from "lucide-react"
import { toast } from "sonner"

import { ProposalCard } from "@/components/ai/proposal-card"
import { EmptyState, Metric, Notice, Panel, TextInput } from "@/components/common"
import { Button } from "@/components/ui/button"
import { askModel } from "@/lib/ai-provider"
import { getStoredAiModel } from "@/lib/ai-models"
import { describeEndpoint, sendsDataOffDevice, type AiEndpoint } from "@/lib/ai-config"
import { api, formatError, type ProposalInput, type ScanRunRecord, type StorageEvaluation, type ValidatedProposal } from "@/lib/api"
import { formatBytes, formatDateTime } from "@/lib/format"
import type { RecoverMode } from "@/lib/recover-space"
import { STORAGE_SYSTEM_PROMPT, buildStorageBrief, goalProposals, groupedProposals, parseGoalGb, parseStorageReply } from "@/lib/storage-ai"

type Result = {
  runId: number
  evaluation: StorageEvaluation
  source: "ai" | "rules"
  summary: string
  /** Why the model's answer was replaced by WinClean's own grouping, when it was. */
  fallbackNote: string | null
  goalPlan: ValidatedProposal[]
  proposals: ValidatedProposal[]
}

/**
 * Storage analysis. WinClean's own rules decide what may be cleaned; a model (or
 * WinClean's plain grouping, with no model) only arranges those candidates into
 * suggestions. Every suggestion is re-resolved and re-checked by the backend
 * before it is shown, and acting on it always goes through the Recover space review.
 */
export function StorageAnalysisPanel({
  scanRuns,
  activeRun,
  endpoint,
  aiReady,
  aiUnavailableReason,
  onApplySuggestion,
  onGoToSettings,
}: {
  scanRuns: ScanRunRecord[]
  activeRun: ScanRunRecord | null
  endpoint: AiEndpoint
  aiReady: boolean
  aiUnavailableReason: string | null
  onApplySuggestion: (runId: number, mode: RecoverMode, candidateIds: string[]) => void
  onGoToSettings: () => void
}) {
  const completedRuns = scanRuns.filter((run) => run.status === "completed")
  const [selectedRunId, setSelectedRunId] = useState<number | null>(null)
  const [goalText, setGoalText] = useState("")
  const [result, setResult] = useState<Result | null>(null)
  const [busy, setBusy] = useState<"ai" | "rules" | null>(null)
  const [error, setError] = useState<string | null>(null)

  const effectiveRunId = selectedRunId ?? (activeRun?.status === "completed" ? activeRun.id : null) ?? completedRuns[0]?.id ?? null
  const goalBytes = parseGoalGb(goalText)

  async function analyze(mode: "ai" | "rules") {
    if (!effectiveRunId || busy) return
    setBusy(mode)
    setError(null)
    setResult(null)
    try {
      const evaluation = await api.storageEvaluate(effectiveRunId, { goalBytes: goalBytes ?? undefined, limit: 250 })
      if (!evaluation) throw new Error("That scan is no longer available.")

      let summary = ""
      let proposals: ProposalInput[] = groupedProposals(evaluation)
      let source: Result["source"] = "rules"
      let fallbackNote: string | null = null
      if (mode === "ai") {
        try {
          const text = await askModel({
            endpoint,
            openaiModel: getStoredAiModel(),
            system: STORAGE_SYSTEM_PROMPT,
            user: buildStorageBrief(evaluation, goalBytes),
          })
          const parsed = parseStorageReply(text)
          if (!parsed.ok) throw new Error(parsed.error)
          summary = parsed.reply.summary
          proposals = parsed.reply.proposals
          source = "ai"
        } catch (aiError) {
          fallbackNote = `${formatError(aiError)} Showing WinClean's own grouping instead.`
        }
      }
      const [validated, goalPlan] = await Promise.all([
        proposals.length ? api.storageValidateProposals(effectiveRunId, proposals) : Promise.resolve([]),
        goalProposals(evaluation).length ? api.storageValidateProposals(effectiveRunId, goalProposals(evaluation)) : Promise.resolve([]),
      ])
      setResult({ runId: effectiveRunId, evaluation, source, summary, fallbackNote, goalPlan, proposals: validated })
      if (source === "ai") toast.success("AI analysis complete.")
    } catch (analysisError) {
      setError(formatError(analysisError))
    } finally {
      setBusy(null)
    }
  }

  function apply(proposal: ValidatedProposal) {
    if (!result || !proposal.action) return
    onApplySuggestion(result.runId, proposal.action, proposal.items.map((item) => item.id))
  }

  const evaluation = result?.evaluation
  const visible = result?.proposals.filter((p) => p.items.length > 0 || p.rejected.length > 0) ?? []

  return (
    <div className="space-y-4">
      <Panel title="Find what to clean">
        {completedRuns.length === 0 ? (
          <EmptyState text="Run a scan first — the analysis needs scan data" />
        ) : (
          <div className="space-y-3">
            <div className="flex flex-wrap items-end gap-3">
              <label className="flex flex-col gap-1 text-sm">
                <span className="text-muted-foreground">Scan</span>
                <select
                  value={effectiveRunId ?? ""}
                  disabled={busy !== null}
                  onChange={(event) => {
                    setSelectedRunId(Number(event.target.value))
                    setResult(null)
                    setError(null)
                  }}
                  className="h-9 max-w-96 rounded-md border border-input bg-card px-2 text-sm outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/40"
                >
                  {completedRuns.map((run) => (
                    <option key={run.id} value={run.id}>
                      {run.rootPath} · {formatBytes(run.bytesIndexed)} · {formatDateTime(run.finishedAtMs)}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1 text-sm">
                <span className="flex items-center gap-1 text-muted-foreground">
                  <Target className="size-3.5" aria-hidden="true" /> Goal (GB, optional)
                </span>
                <TextInput value={goalText} onChange={(event) => setGoalText(event.target.value)} inputMode="decimal" placeholder="e.g. 20" className="w-32" aria-label="Space to free in gigabytes" />
              </label>
              <Button onClick={() => void analyze("ai")} disabled={!aiReady || busy !== null}>
                {busy === "ai" ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Sparkles className="size-4" aria-hidden="true" />}
                {busy === "ai" ? "Analyzing" : "Analyze with AI"}
              </Button>
              <Button variant="outline" onClick={() => void analyze("rules")} disabled={busy !== null}>
                {busy === "rules" ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <ListChecks className="size-4" aria-hidden="true" />}
                Quick suggestions (no AI)
              </Button>
            </div>
            {!aiReady ? (
              <p className="text-xs text-muted-foreground">
                {aiUnavailableReason}{" "}
                <button type="button" className="font-medium text-primary underline-offset-2 hover:underline" onClick={onGoToSettings}>
                  Open AI settings
                </button>
              </p>
            ) : (
              <p className="text-xs text-muted-foreground">
                {sendsDataOffDevice(endpoint) ? `Only file metadata (paths, sizes, dates) goes to ${describeEndpoint(endpoint)} — never file contents.` : "Your local model runs on this PC; nothing is sent over the internet."} The model can only group candidates WinClean already approved; every suggestion is re-checked before you see it.
              </p>
            )}
          </div>
        )}
        {error ? <Notice tone="danger" text={error} /> : null}
        {busy ? (
          <div className="mt-4 space-y-2">
            <div className="h-4 w-2/3 animate-pulse rounded bg-muted" />
            <div className="h-4 w-1/2 animate-pulse rounded bg-muted" />
            <p className="pt-1 text-xs text-muted-foreground">{busy === "ai" ? "The assistant is weighing what is safe to remove — free models can take a minute…" : "Grouping WinClean's safe candidates…"}</p>
          </div>
        ) : null}
      </Panel>

      {result && evaluation && !busy ? (
        <>
          {result.fallbackNote ? <Notice tone="warning" text={result.fallbackNote} /> : null}
          {evaluation.warnings.map((warning) => (
            <Notice key={warning} tone="info" text={warning} />
          ))}
          <div className="grid gap-3 md:grid-cols-4">
            <Metric label="Safe to clean" value={formatBytes(evaluation.cleanBytes)} hint="Recycle Bin" />
            <Metric label="Offload to Drive" value={formatBytes(evaluation.offloadBytes)} hint="verified upload first" />
            <Metric label="Needs your judgment" value={formatBytes(evaluation.reviewBytes)} hint="never automatic" />
            <Metric label="Kept out" value={String(evaluation.blockedTotal)} hint="protected files" />
          </div>

          {result.summary ? (
            <Panel title="Summary">
              <p className="text-sm leading-relaxed">{result.summary}</p>
            </Panel>
          ) : null}

          {evaluation.goal ? (
            <Panel title={`Goal: free ${formatBytes(evaluation.goal.goalBytes)}`}>
              <p className="mb-3 text-sm text-muted-foreground">{evaluation.goal.note}</p>
              <div className="space-y-3">
                {result.goalPlan.map((proposal, index) => (
                  <ProposalCard key={`goal-${index}`} proposal={proposal} onApply={apply} />
                ))}
                {result.goalPlan.length === 0 ? <EmptyState text="Nothing safe is available toward this goal" /> : null}
              </div>
            </Panel>
          ) : null}

          <Panel title={result.source === "ai" ? `AI suggestions (${visible.length})` : `Suggestions (${visible.length})`}>
            {visible.length === 0 ? (
              <EmptyState text="Nothing worth acting on was found" />
            ) : (
              <div className="space-y-3">
                {visible.map((proposal, index) => (
                  <ProposalCard key={`${proposal.title}-${index}`} proposal={proposal} onApply={apply} />
                ))}
              </div>
            )}
          </Panel>

          {evaluation.blocked.length > 0 ? (
            <Panel title="Why some large files are not suggested">
              <p className="mb-2 flex items-start gap-1.5 text-xs text-muted-foreground">
                <ShieldAlert className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
                These match your scan but WinClean keeps them out of every suggestion, whatever a model says.
              </p>
              <ul className="space-y-1 text-xs">
                {evaluation.blocked.slice(0, 8).map((item) => (
                  <li key={item.path} className="flex justify-between gap-3">
                    <span className="min-w-0 truncate" title={`${item.path} — ${item.detail}`}>
                      {item.path} <span className="text-muted-foreground">— {item.detail}</span>
                    </span>
                    <span className="shrink-0 text-muted-foreground tabular-nums">{formatBytes(item.size)}</span>
                  </li>
                ))}
                {evaluation.blockedTotal > 8 ? <li className="text-muted-foreground">and {evaluation.blockedTotal - 8} more</li> : null}
              </ul>
            </Panel>
          ) : null}
        </>
      ) : null}

      {!result && !busy && completedRuns.length > 0 ? (
        <Panel title="How it works">
          <ol className="list-decimal space-y-1.5 pl-5 text-sm text-muted-foreground">
            <li>WinClean&apos;s own rules decide what may be cleaned, offloaded or left for you — projects, system files, documents and cloud-synced folders are protected.</li>
            <li>The assistant groups those candidates and explains them. Without AI, WinClean groups them by rule.</li>
            <li>Every suggestion is re-resolved against your scan and re-checked on disk, then you review it in Recover space. Files go to the Recycle Bin — nothing is permanently deleted.</li>
          </ol>
        </Panel>
      ) : null}
    </div>
  )
}
