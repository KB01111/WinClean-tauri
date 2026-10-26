import { useCallback, useEffect, useMemo, useState } from "react"
import { Leaf, Loader2, RefreshCw } from "lucide-react"
import { toast } from "sonner"

import { EmptyState, Metric, Notice, Panel, TextInput } from "@/components/common"
import { Badge, type BadgeTone } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { useOptimizerEvents } from "@/hooks/use-optimizer-events"
import { usePolling } from "@/hooks/use-polling"
import { api, formatError } from "@/lib/api"
import {
  WORKLOAD_LABELS,
  ecoQosModeLabel,
  type EcoDecision,
  type ProcessInfo,
  type ProfilesView,
  type WorkloadSnapshot,
} from "@/lib/optimizer"

const DECISION: Record<EcoDecision, { label: string; tone: BadgeTone; why: string }> = {
  protected: { label: "Protected", tone: "info", why: "Developer tools, compilers, games, AI inference and system processes are never throttled." },
  foreground: { label: "In use", tone: "good", why: "Part of the window you are working in." },
  eligible: { label: "Eligible", tone: "neutral", why: "Background process that can run on efficiency cores." },
  unknown: { label: "Unknown", tone: "warn", why: "Could not be inspected (access denied or exited)." },
}

/**
 * Processes & EcoQoS. EcoQoS asks Windows to run a process on efficient
 * cores at low frequency; it is applied per process, audited, and always
 * restores the system-managed default. The foreground window's whole process
 * tree and developer/gaming/AI processes are never throttled.
 */
export function ProcessesView() {
  const [processes, setProcesses] = useState<ProcessInfo[]>([])
  const [workload, setWorkload] = useState<WorkloadSnapshot | null>(null)
  const [profiles, setProfiles] = useState<ProfilesView | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<number | "batch" | null>(null)
  const [query, setQuery] = useState("")

  const load = useCallback(async () => {
    try {
      const [list, current] = await Promise.all([api.optimizerListProcesses(), api.optimizerWorkload()])
      setProcesses(list)
      setWorkload(current)
      setError(null)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
    api.optimizerListProfiles().then(setProfiles).catch(() => {})
  }, [load])

  // The process table is only worth refreshing while someone is looking at it.
  usePolling(load, 10_000)
  useOptimizerEvents((event) => {
    if (event.kind === "workload") setWorkload(event.data)
    if (event.kind === "profileChanged") void api.optimizerListProfiles().then(setProfiles).catch(() => {})
  })

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase()
    return processes.filter((p) => !q || p.name.toLowerCase().includes(q) || String(p.pid) === q)
  }, [processes, query])
  const eligible = processes.filter((p) => p.ecoDecision === "eligible" && !p.ecoQosApplied)
  const throttled = processes.filter((p) => p.ecoQosApplied)
  const activeProfile = profiles?.profiles.find((p) => p.id === profiles.active?.profileId)

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

  async function batch(enable: boolean) {
    const targets = enable ? eligible : throttled
    if (targets.length === 0) {
      toast.info(enable ? "No eligible background processes right now." : "Nothing is throttled.")
      return
    }
    setBusy("batch")
    let failed = 0
    for (const proc of targets) {
      try {
        await api.optimizerSetProcessEcoQos(proc.pid, proc.name, enable)
      } catch {
        failed += 1 // a process may have exited meanwhile; the rest still proceed
      }
    }
    setBusy(null)
    toast[failed ? "warning" : "success"](
      `${enable ? "Throttled" : "Restored"} ${targets.length - failed} of ${targets.length} process(es).`
    )
    await load()
  }

  return (
    <div className="space-y-4">
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <Metric label="Workload" value={workload ? WORKLOAD_LABELS[workload.kind] : "—"} hint={workload?.foreground ?? "foreground unknown"} />
        <Metric label="Active profile" value={activeProfile?.label ?? "None"} hint={activeProfile ? `EcoQoS: ${ecoQosModeLabel(activeProfile.ecoQosMode)}` : undefined} />
        <Metric label="Eligible now" value={String(eligible.length)} hint="background processes" />
        <Metric label="Throttled" value={String(throttled.length)} hint="EcoQoS applied" />
      </div>

      {workload && workload.evidence.length > 0 ? (
        <Panel title="Why WinClean thinks that">
          <ul className="list-disc space-y-1 pl-5 text-sm text-muted-foreground">
            {workload.evidence.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </Panel>
      ) : null}

      <Panel
        title="Processes"
        action={
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading || busy !== null}>
              <RefreshCw className="size-4" aria-hidden="true" />
              Refresh
            </Button>
            <Button variant="outline" size="sm" onClick={() => void batch(false)} disabled={busy !== null || throttled.length === 0}>
              Restore all ({throttled.length})
            </Button>
            <Button size="sm" onClick={() => void batch(true)} disabled={busy !== null || eligible.length === 0}>
              {busy === "batch" ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Leaf className="size-4" aria-hidden="true" />}
              Throttle eligible ({eligible.length})
            </Button>
          </div>
        }
      >
        {error ? <Notice tone="danger" text={error} /> : null}
        <TextInput value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Filter by name or PID…" aria-label="Filter processes" className="mb-3 max-w-xs" />
        {loading ? (
          <EmptyState text="Reading processes…" />
        ) : visible.length === 0 ? (
          <EmptyState text="No processes match" />
        ) : (
          <div className="max-h-[55vh] overflow-auto">
            <table className="w-full border-collapse text-sm">
              <thead className="sticky top-0 bg-card">
                <tr className="border-b border-border text-left text-xs text-muted-foreground uppercase">
                  <th className="py-2 font-semibold">Process</th>
                  <th className="py-2 text-right font-semibold">Memory</th>
                  <th className="py-2 text-right font-semibold">CPU</th>
                  <th className="py-2 pl-3 font-semibold">Status</th>
                  <th className="py-2 text-right font-semibold">EcoQoS</th>
                </tr>
              </thead>
              <tbody>
                {visible.map((p) => {
                  const d = DECISION[p.ecoDecision]
                  const canToggle = p.ecoQosApplied || p.ecoDecision === "eligible"
                  return (
                    <tr key={p.pid} className="border-b border-border/60">
                      <td className="max-w-56 truncate py-2 pr-3 font-medium" title={`${p.name} (PID ${p.pid})`}>
                        {p.name} <span className="text-xs font-normal text-muted-foreground">{p.pid}</span>
                      </td>
                      <td className="py-2 text-right tabular-nums">{p.workingSetMb.toFixed(0)} MB</td>
                      <td className="py-2 text-right tabular-nums">{p.cpuPercentRecent.toFixed(1)}%</td>
                      <td className="py-2 pl-3">
                        <Badge tone={p.ecoQosApplied ? "good" : d.tone} title={d.why}>
                          {p.ecoQosApplied ? "Throttled" : d.label}
                        </Badge>
                      </td>
                      <td className="py-2 text-right">
                        <Button
                          variant="outline"
                          size="sm"
                          disabled={!canToggle || busy !== null}
                          title={canToggle ? undefined : d.why}
                          onClick={() => void toggle(p)}
                        >
                          {busy === p.pid ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                          {p.ecoQosApplied ? "Restore" : "Throttle"}
                        </Button>
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        )}
        <p className="mt-3 text-xs text-muted-foreground">
          Throttling is per process and reversible: Restore returns control to Windows. PIDs that were reused by another program are never touched. Automatic throttling follows the active profile&apos;s EcoQoS mode (Optimization Profiles).
        </p>
      </Panel>
    </div>
  )
}
