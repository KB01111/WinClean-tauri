import { useCallback, useEffect, useState } from "react"
import { Leaf, Loader2, RefreshCw } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { EmptyState, Notice, Panel } from "@/components/common"
import { api, formatError } from "@/lib/api"
import type { ProcessInfo } from "@/lib/optimizer"

/** Display process resource usage and EcoQoS actions, tracking changes applied during this view’s lifetime. */
export function ProcessesView() {
  const [processes, setProcesses] = useState<ProcessInfo[]>([])
  const [applied, setApplied] = useState<Set<number>>(new Set())
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<number | null>(null)

  /** Refresh process telemetry and eligibility decisions, updating loading and error state. */
  const load = useCallback(async () => {
    setLoading(true)
    try {
      setProcesses(await api.optimizerListProcesses())
      setError(null)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  /** Apply or restore EcoQoS for a process and track the PID locally after a successful request. */
  async function toggle(proc: ProcessInfo) {
    const enable = !applied.has(proc.pid)
    setBusy(proc.pid)
    try {
      await api.optimizerSetProcessEcoQos(proc.pid, proc.name, enable)
      setApplied((cur) => {
        const next = new Set(cur)
        if (enable) next.add(proc.pid)
        else next.delete(proc.pid)
        return next
      })
      toast.success(`EcoQoS ${enable ? "enabled" : "restored"} for ${proc.name}. Fully reversible.`)
    } catch (e) {
      toast.error(formatError(e))
    } finally {
      setBusy(null)
    }
  }

  /** Attempt EcoQoS for each eligible process not already tracked as applied, continuing after individual failures. */
  async function applyEligible() {
    const eligible = processes.filter((p) => p.ecoDecision === "eligible" && !applied.has(p.pid))
    for (const proc of eligible) {
      try {
        await api.optimizerSetProcessEcoQos(proc.pid, proc.name, true)
        setApplied((cur) => new Set(cur).add(proc.pid))
      } catch {
        // Per-process failures are audited in the backend; continue with the rest.
      }
    }
    toast.success(`EcoQoS applied to eligible background processes (${eligible.length} candidates). Editors, compilers, games and inference were excluded.`)
  }

  return (
    <Panel
      title="Processes & EcoQoS (SetProcessInformation)"
      action={
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={() => void applyEligible()} disabled={loading}>
            <Leaf className="size-4" aria-hidden="true" /> Apply to eligible
          </Button>
          <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Refresh
          </Button>
        </div>
      }
    >
      <p className="mb-3 text-sm text-muted-foreground">
        WinClean never terminates processes. EcoQoS only lowers efficiency-mode priority for clearly eligible background work (updaters, indexers, telemetry). VS Code, Cursor, Codex, Claude Code, OpenCode, compilers, games and AI inference are protected, as is the foreground app.
      </p>
      {error ? <Notice tone="danger" text={error} /> : null}
      {loading ? (
        <EmptyState text="Sampling processes (two 250 ms CPU samples)…" />
      ) : (
        <div className="max-h-[60vh] overflow-auto">
          {processes.map((proc) => (
            <div key={proc.pid} className="flex items-center justify-between gap-3 border-b border-border/60 py-2 text-sm">
              <div className="min-w-0">
                <span className="font-medium">{proc.name}</span>{" "}
                <span className="text-xs text-muted-foreground">
                  PID {proc.pid} · {proc.workingSetMb.toFixed(1)} MB · CPU {proc.cpuPercentRecent.toFixed(1)}%
                  {proc.isForeground ? " · foreground" : ""} · {proc.ecoDecision}
                </span>
              </div>
              <Button
                size="sm"
                variant={applied.has(proc.pid) ? "default" : "outline"}
                disabled={proc.ecoDecision === "protected" || proc.ecoDecision === "foreground" || busy === proc.pid}
                onClick={() => void toggle({ ...proc, ecoQosApplied: applied.has(proc.pid) })}
              >
                {applied.has(proc.pid) ? "Restore" : "EcoQoS"}
              </Button>
            </div>
          ))}
        </div>
      )}
    </Panel>
  )
}
