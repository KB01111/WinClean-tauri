import { useCallback, useEffect, useState } from "react"
import { Leaf, Loader2, RefreshCw } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { EmptyState, Notice, Panel } from "@/components/common"
import { api, formatError } from "@/lib/api"
import type { ProcessInfo } from "@/lib/optimizer"

export function ProcessesView() {
  const [processes, setProcesses] = useState<ProcessInfo[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<number | null>(null)

  // EcoQoS state comes from the backend on every listing
  // (GetProcessInformation), so it survives view navigation and the
  // Restore action is always available for throttled processes —
  // including ones that have since become foreground.
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

  async function toggle(proc: ProcessInfo) {
    const enable = !proc.ecoQosApplied
    setBusy(proc.pid)
    try {
      await api.optimizerSetProcessEcoQos(proc.pid, proc.name, enable)
      toast.success(`EcoQoS ${enable ? "enabled" : "restored"} for ${proc.name}. Fully reversible.`)
      await load()
    } catch (e) {
      toast.error(formatError(e))
    } finally {
      setBusy(null)
    }
  }

  async function applyEligible() {
    const eligible = processes.filter((p) => p.ecoDecision === "eligible" && !p.ecoQosApplied)
    let failed = 0
    for (const proc of eligible) {
      try {
        await api.optimizerSetProcessEcoQos(proc.pid, proc.name, true)
      } catch {
        failed += 1 // per-process failures are audited in the backend
      }
    }
    toast.success(`EcoQoS applied to eligible background processes (${eligible.length - failed}/${eligible.length}). Editors, compilers, games and inference were excluded.`)
    await load()
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
        WinClean never terminates processes. EcoQoS only lowers efficiency-mode priority for clearly eligible background work (updaters, indexers, telemetry). VS Code, Cursor, Codex, Claude Code, OpenCode, compilers, games and AI inference are protected, as is the foreground app. The backend revalidates each PID's real process before enabling, and reports live EcoQoS state on every refresh.
      </p>
      {error ? <Notice tone="danger" text={error} /> : null}
      {loading ? (
        <EmptyState text="Sampling processes (two samples over ~250 ms)…" />
      ) : (
        <div className="max-h-[60vh] overflow-auto">
          {processes.map((proc) => (
            <div key={proc.pid} className="flex items-center justify-between gap-3 border-b border-border/60 py-2 text-sm">
              <div className="min-w-0">
                <span className="font-medium">{proc.name}</span>{" "}
                <span className="text-xs text-muted-foreground">
                  PID {proc.pid} · {proc.workingSetMb.toFixed(1)} MB · CPU {proc.cpuPercentRecent < 0 ? "n/a" : `${proc.cpuPercentRecent.toFixed(1)}%`}
                  {proc.isForeground ? " · foreground" : ""} · {proc.ecoDecision}
                  {proc.ecoQosApplied ? " · EcoQoS on" : ""}
                </span>
              </div>
              <Button
                size="sm"
                variant={proc.ecoQosApplied ? "default" : "outline"}
                disabled={(!proc.ecoQosApplied && (proc.ecoDecision === "protected" || proc.ecoDecision === "foreground")) || busy === proc.pid}
                onClick={() => void toggle(proc)}
              >
                {proc.ecoQosApplied ? "Restore" : "EcoQoS"}
              </Button>
            </div>
          ))}
        </div>
      )}
    </Panel>
  )
}
