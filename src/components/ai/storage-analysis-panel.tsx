import { useState } from "react"
import { ArrowRight, Loader2, Sparkles } from "lucide-react"
import { toast } from "sonner"

import { EmptyState, Notice, Panel, StatusBadge } from "@/components/common"
import { Button } from "@/components/ui/button"
import { analyzeScanWithAi, type AiReport } from "@/lib/ai-agent"
import { getStoredAiModel } from "@/lib/ai-models"
import type { AiEndpoint } from "@/lib/ai-config"
import { api, formatError, type ScanRunRecord } from "@/lib/api"
import { formatBytes, formatDateTime } from "@/lib/format"
import type { RecoverMode } from "@/lib/recover-space"

/** Scan analysis: the assistant reads scan metadata and suggests what to clean or offload. */
export function StorageAnalysisPanel({
  scanRuns,
  activeRun,
  endpoint,
  onApplySuggestion,
}: {
  scanRuns: ScanRunRecord[]
  activeRun: ScanRunRecord | null
  endpoint: AiEndpoint
  onApplySuggestion: (runId: number, mode: RecoverMode, candidateIds: string[]) => void
}) {
  const completedRuns = scanRuns.filter((run) => run.status === "completed")
  const [selectedRunId, setSelectedRunId] = useState<number | null>(null)
  const [report, setReport] = useState<AiReport | null>(null)
  const [analyzedRunId, setAnalyzedRunId] = useState<number | null>(null)
  const [analyzing, setAnalyzing] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const effectiveRunId = selectedRunId ?? (activeRun?.status === "completed" ? activeRun.id : null) ?? completedRuns[0]?.id ?? null

  async function analyze() {
    if (!effectiveRunId || analyzing) return
    setAnalyzing(true)
    setError(null)
    try {
      const apiKey = endpoint.kind === "local" ? "" : await api.getAiApiKey()
      if (endpoint.kind !== "local" && !apiKey) throw new Error("Add your OpenAI API key in Settings first.")
      const nextReport = await analyzeScanWithAi({ apiKey: apiKey ?? "", model: getStoredAiModel(), runId: effectiveRunId, endpoint })
      setReport(nextReport)
      setAnalyzedRunId(effectiveRunId)
      toast.success("AI analysis complete.")
    } catch (analysisError) {
      setError(formatError(analysisError))
    } finally {
      setAnalyzing(false)
    }
  }

  return (
    <div className="space-y-4">
      <Panel
        title="Analyze a scan"
        action={
          <Button onClick={analyze} disabled={!effectiveRunId || analyzing}>
            {analyzing ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Sparkles className="size-4" aria-hidden="true" />}
            {analyzing ? "Analyzing" : "Analyze with AI"}
          </Button>
        }
      >
        {completedRuns.length === 0 ? (
          <EmptyState text="Run a scan first — the assistant needs scan data to analyze" />
        ) : (
          <div className="flex flex-wrap items-center gap-3">
            <label className="flex items-center gap-2 text-sm">
              <span className="text-muted-foreground">Scan run</span>
              <select
                value={effectiveRunId ?? ""}
                disabled={analyzing}
                onChange={(event) => {
                  setSelectedRunId(Number(event.target.value))
                  setReport(null)
                  setAnalyzedRunId(null)
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
            <p className="text-xs text-muted-foreground">
              {endpoint.kind === "local" ? "Your local model runs on this PC; nothing is sent over the internet. " : "Only file metadata (paths, sizes, dates) is sent to OpenAI — never file contents. "}
              Smart suggestions can only reference items WinClean already considers actionable.
            </p>
          </div>
        )}
        {error ? <Notice tone="danger" text={error} /> : null}
        {analyzing ? (
          <div className="mt-4 space-y-2">
            <div className="h-4 w-2/3 animate-pulse rounded bg-muted" />
            <div className="h-4 w-1/2 animate-pulse rounded bg-muted" />
            <p className="pt-1 text-xs text-muted-foreground">The assistant is reading your scan data and weighing what is safe to remove…</p>
          </div>
        ) : null}
      </Panel>

      {report && !analyzing ? (
        <>
          <Panel title="Summary">
            <p className="text-sm leading-relaxed">{report.summary}</p>
          </Panel>
          <Panel title={`Suggestions (${report.suggestions.length})`}>
            {report.suggestions.length === 0 ? (
              <EmptyState text="The assistant found nothing worth acting on" />
            ) : (
              <div className="space-y-3">
                {report.suggestions.map((suggestion, index) => (
                  <div key={`${suggestion.title}-${index}`} className="rounded-lg border border-border bg-card p-4">
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className="flex items-center gap-2">
                          <span className="font-medium">{suggestion.title}</span>
                          <StatusBadge status={suggestion.action} />
                        </div>
                        <p className="mt-1 text-sm text-muted-foreground">{suggestion.reason}</p>
                        <p className="mt-1 text-xs text-muted-foreground">
                          {suggestion.candidateIds.length} item(s) · about {formatBytes(suggestion.estimatedBytes)}
                        </p>
                      </div>
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={suggestion.candidateIds.length === 0 || analyzedRunId === null}
                        onClick={() => {
                          if (analyzedRunId !== null) onApplySuggestion(analyzedRunId, suggestion.action, suggestion.candidateIds)
                        }}
                      >
                        Add to selection
                        <ArrowRight className="size-4" aria-hidden="true" />
                      </Button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </Panel>
        </>
      ) : null}

      {!report && !analyzing && completedRuns.length > 0 ? (
        <Panel title="How it works">
          <ol className="list-decimal space-y-1.5 pl-5 text-sm text-muted-foreground">
            <li>The assistant reads your scan summary and largest folders.</li>
            <li>It cross-checks WinClean&apos;s own safe recommendations — it can only suggest files WinClean already considers actionable.</li>
            <li>You review each suggestion and apply it to the Recover Space selection. Nothing is deleted or uploaded without your click.</li>
          </ol>
        </Panel>
      ) : null}
    </div>
  )
}
