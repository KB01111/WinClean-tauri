import { useCallback, useEffect, useState } from "react"
import { Loader2, RefreshCw, ShieldCheck } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { EmptyState, Notice, Panel, StatusBadge } from "@/components/common"
import { api, formatError } from "@/lib/api"
import type { ServiceInfo, StartType } from "@/lib/optimizer"

const START_TYPES: StartType[] = ["automatic", "automaticDelayed", "manual", "disabled"]

/** Display filterable Windows services with policy classifications and guarded start-type controls. */
export function ServicesView() {
  const [services, setServices] = useState<ServiceInfo[]>([])
  const [query, setQuery] = useState("")
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<Set<string>>(new Set())
  const [loading, setLoading] = useState(true)

  /** Refresh native service configurations and policy classifications, updating loading and error state. */
  const load = useCallback(async () => {
    setLoading(true)
    try {
      setServices(await api.optimizerListServices())
      setError(null)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  /** Request a service start-type change, replace its row on success, and report the outcome. */
  async function change(svc: ServiceInfo, startType: string) {
    setBusy((cur) => new Set(cur).add(svc.name))
    try {
      const updated = await api.optimizerSetServiceStartType(svc.name, svc.displayName, startType)
      setServices((cur) => cur.map((s) => (s.name === updated.name ? updated : s)))
      toast.success(`${svc.displayName}: start type set to ${startType}. Snapshot saved for rollback.`)
    } catch (e) {
      toast.error(formatError(e))
    } finally {
      setBusy((cur) => { const next = new Set(cur); next.delete(svc.name); return next })
    }
  }

  const visible = services.filter((s) =>
    `${s.name} ${s.displayName}`.toLowerCase().includes(query.toLowerCase())
  )

  return (
    <div className="space-y-4">
      <Panel
        title="Windows services (Service Control Manager)"
        action={
          <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Refresh
          </Button>
        }
      >
        <div className="mb-3 flex items-center gap-2 text-sm text-muted-foreground">
          <ShieldCheck className="size-4 text-primary" aria-hidden="true" />
          Protected services (Defender, Windows Update, Search, drivers, networking, Copilot+) can never be changed. Every change snapshots the previous start type first; roll back in Optimization History.
        </div>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Filter services…"
          aria-label="Filter services"
          className="mb-3 h-9 w-full max-w-md rounded-md border border-input bg-card px-3 text-sm outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/40"
        />
        {error ? <Notice tone="danger" text={error} /> : null}
        {loading ? (
          <EmptyState text="Reading services from the Service Control Manager…" />
        ) : visible.length === 0 ? (
          <EmptyState text="No services match the filter" />
        ) : (
          <div className="max-h-[60vh] overflow-auto">
            {visible.map((svc) => (
              <div key={svc.name} className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 py-2.5 text-sm">
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium">{svc.displayName}</span>
                    <StatusBadge status={svc.state} />
                    <span className="rounded-full bg-muted px-2 py-0.5 text-xs">{svc.classification.risk}</span>
                  </div>
                  <div className="mt-0.5 truncate text-xs text-muted-foreground" title={svc.classification.recommendation}>
                    {svc.name} · start: {svc.startType}{svc.delayedAutoStart ? " (delayed)" : ""}
                    {svc.dependencies.length ? ` · depends on: ${svc.dependencies.join(", ")}` : ""} · {svc.classification.recommendation}
                  </div>
                </div>
                <select
                  aria-label={`Start type for ${svc.displayName}`}
                  value={svc.startType}
                  disabled={!svc.canChange || busy.has(svc.name)}
                  onChange={(e) => void change(svc, e.target.value)}
                  className="h-9 rounded-md border border-input bg-card px-2 text-sm disabled:opacity-50"
                >
                  {/* Unknown services cannot be newly disabled, but an already-disabled one must still show its real state. */}
                  {START_TYPES.filter((t) => t !== "disabled" || svc.classification.known || svc.startType === "disabled").map((t) => (
                    <option key={t} value={t}>{t}</option>
                  ))}
                  {!START_TYPES.includes(svc.startType as StartType) ? (
                    <option value={svc.startType}>{svc.startType}</option>
                  ) : null}
                </select>
              </div>
            ))}
          </div>
        )}
      </Panel>
    </div>
  )
}
