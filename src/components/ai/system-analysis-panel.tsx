import { useState } from "react"
import { Loader2, Sparkles } from "lucide-react"
import { toast } from "sonner"

import { EmptyState, Notice, Panel } from "@/components/common"
import { PlanReviewDialog } from "@/components/optimizer/plan-review-dialog"
import { Badge, type BadgeTone } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { useHelper } from "@/hooks/use-helper"
import { useServicePlan } from "@/hooks/use-service-plan"
import { analyzeSystemWithAi } from "@/lib/ai-agent"
import { getStoredAiModel } from "@/lib/ai-models"
import type { AiEndpoint } from "@/lib/ai-config"
import { normalizeProposals, type NormalizedProposals, type SystemReport } from "@/lib/ai-proposals"
import { api, formatError } from "@/lib/api"
import { startTypeLabel } from "@/lib/optimizer"

const SEVERITY: Record<SystemReport["findings"][number]["severity"], BadgeTone> = { info: "neutral", attention: "warn", important: "bad" }

type Result = { report: SystemReport; proposals: NormalizedProposals; serviceNames: Record<string, string> }

/**
 * System analysis. The assistant reads the system through read-only tools and
 * can only *propose*. Proposals are filtered here, validated again by the
 * backend's real policy, and shown in the same review dialog as manual
 * changes. Applying always needs the user's explicit click on that dialog.
 */
export function SystemAnalysisPanel({ endpoint }: { endpoint: AiEndpoint }) {
  const helper = useHelper()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<Result | null>(null)
  const [applyingProfile, setApplyingProfile] = useState(false)
  const plan = useServicePlan(() => setResult((r) => (r ? { ...r, proposals: { ...r.proposals, serviceChanges: [] } } : r)))

  async function analyze() {
    if (busy) return
    setBusy(true)
    setError(null)
    setResult(null)
    try {
      const apiKey = endpoint.kind === "local" ? "" : await api.getAiApiKey()
      if (endpoint.kind !== "local" && !apiKey) throw new Error("Add your OpenAI API key in Settings first.")
      const report = await analyzeSystemWithAi({ apiKey: apiKey ?? "", model: getStoredAiModel(), endpoint })
      const [services, profiles] = await Promise.all([api.optimizerListServices(), api.optimizerListProfiles()])
      setResult({
        report,
        proposals: normalizeProposals(report, services, profiles.profiles),
        serviceNames: Object.fromEntries(services.map((s) => [s.name, s.displayName])),
      })
    } catch (e) {
      setError(formatError(e))
    } finally {
      setBusy(false)
    }
  }

  async function applyProfile(profileId: string, label: string) {
    setApplyingProfile(true)
    try {
      const outcome = await api.optimizerApplyPowerProfile(profileId)
      toast.success(`${label}: ${outcome.detail}`)
      setResult((r) => (r ? { ...r, proposals: { ...r.proposals, profile: null } } : r))
    } catch (e) {
      toast.error(formatError(e))
    } finally {
      setApplyingProfile(false)
    }
  }

  const proposals = result?.proposals

  return (
    <div className="space-y-4">
      <Panel
        title="Analyze this PC"
        action={
          <Button onClick={() => void analyze()} disabled={busy}>
            {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Sparkles className="size-4" aria-hidden="true" />}
            {busy ? "Analyzing" : "Analyze with AI"}
          </Button>
        }
      >
        <p className="text-sm text-muted-foreground">
          The assistant looks at services, processes, battery, hardware and your profiles with read-only tools and proposes safe improvements, if any are justified. It cannot change anything.{" "}
          {endpoint.kind === "local"
            ? "Your local model runs on this PC; nothing is sent over the internet."
            : "Service names, process names with CPU/memory, battery state and hardware model names are sent to OpenAI for the analysis; no file contents are."}
        </p>
        {error ? <Notice tone="danger" text={error} /> : null}
        {busy ? <p className="mt-3 text-xs text-muted-foreground">Reading the system and thinking it through…</p> : null}
      </Panel>

      {result ? (
        <>
          <Panel title="Summary">
            <p className="text-sm leading-relaxed">{result.report.summary}</p>
          </Panel>

          <Panel title={`Findings (${result.report.findings.length})`}>
            {result.report.findings.length === 0 ? (
              <EmptyState text="Nothing notable found." />
            ) : (
              <ul className="grid gap-2">
                {result.report.findings.map((f, i) => (
                  <li key={`${f.title}-${i}`} className="rounded-lg border border-border bg-card p-3 text-sm">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-medium">{f.title}</span>
                      <Badge tone={SEVERITY[f.severity]}>{f.severity}</Badge>
                      <Badge>{f.area}</Badge>
                    </div>
                    <p className="mt-1 text-xs text-muted-foreground">{f.detail}</p>
                  </li>
                ))}
              </ul>
            )}
          </Panel>

          <Panel title="Proposed changes">
            {proposals && proposals.serviceChanges.length === 0 && !proposals.profile ? (
              <EmptyState text="The assistant proposed no changes that pass WinClean's rules." />
            ) : (
              <div className="space-y-4">
                {proposals && proposals.serviceChanges.length > 0 ? (
                  <div>
                    <ul className="grid gap-1.5 text-sm">
                      {proposals.serviceChanges.map((c) => (
                        <li key={c.serviceName} className="rounded-md border border-border bg-card px-3 py-2">
                          <span className="font-medium">{result.serviceNames[c.serviceName] ?? c.serviceName}</span> → {startTypeLabel(c.startType)}
                          <span className="block text-xs text-muted-foreground">{proposals.reasons[c.serviceName]}</span>
                        </li>
                      ))}
                    </ul>
                    <Button className="mt-3" disabled={plan.preparing} onClick={() => void plan.prepare(proposals.serviceChanges, "ai")}>
                      {plan.preparing ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                      Review {proposals.serviceChanges.length} proposed service change{proposals.serviceChanges.length === 1 ? "" : "s"}
                    </Button>
                  </div>
                ) : null}
                {proposals?.profile ? (
                  <div className="rounded-lg border border-border bg-card p-3 text-sm">
                    <div className="font-medium">Switch to {proposals.profile.label}</div>
                    <p className="text-xs text-muted-foreground">{proposals.profile.reason}</p>
                    <Button className="mt-2" size="sm" disabled={applyingProfile} onClick={() => void applyProfile(proposals.profile!.profileId, proposals.profile!.label)}>
                      {applyingProfile ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                      Apply {proposals.profile.label}
                    </Button>
                  </div>
                ) : null}
              </div>
            )}
            {proposals && proposals.dropped.length > 0 ? (
              <details className="mt-4 text-xs text-muted-foreground">
                <summary className="cursor-pointer">Not shown ({proposals.dropped.length}) — filtered out before review</summary>
                <ul className="mt-1 list-disc pl-5">
                  {proposals.dropped.map((d) => (
                    <li key={d}>{d}</li>
                  ))}
                </ul>
              </details>
            ) : null}
          </Panel>
        </>
      ) : null}

      <PlanReviewDialog
        prepared={plan.prepared}
        title="Review the assistant's proposal"
        origin={plan.origin}
        applying={plan.applying}
        helper={helper}
        onApply={(token) => {
          void plan.apply(token).then(() => helper.refresh())
        }}
        onClose={plan.close}
      />
    </div>
  )
}
