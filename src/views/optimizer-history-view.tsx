import { useCallback, useEffect, useState } from "react"
import { Loader2, RefreshCw, Undo2 } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { EmptyState, Notice, Panel, StatusBadge } from "@/components/common"
import { formatDateTime } from "@/lib/format"
import { api, formatError } from "@/lib/api"
import type { OptimizerHistory } from "@/lib/optimizer"

/** Display the optimizer audit trail and snapshots with rollback controls. */
export function OptimizerHistoryView() {
  const [history, setHistory] = useState<OptimizerHistory | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<number | null>(null)

  /** Refresh the latest 100 optimizer audit entries and snapshots, updating loading and error state. */
  const load = useCallback(async () => {
    setLoading(true)
    try {
      setHistory(await api.optimizerHistory(100))
      setError(null)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  /** Restore a snapshot through the backend, report its outcome, and refresh history on success. */
  async function rollback(id: number) {
    setBusy(id)
    try {
      const msg = await api.optimizerRollback(id)
      toast.success(msg)
      await load()
    } catch (e) {
      toast.error(formatError(e))
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="space-y-4">
      <Panel
        title="Optimization history & rollback"
        action={
          <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Refresh
          </Button>
        }
      >
        {error ? <Notice tone="danger" text={error} /> : null}
        {loading ? (
          <EmptyState text="Loading audit trail…" />
        ) : !history || history.audit.length === 0 ? (
          <EmptyState text="No optimization actions yet. Service, EcoQoS and power changes appear here with their snapshots." />
        ) : (
          history.audit.map((row) => (
            <div key={row.id} className="flex items-center justify-between gap-3 border-b border-border/60 py-2.5 text-sm">
              <div className="min-w-0">
                <span className="font-medium">{row.target}</span>{" "}
                <StatusBadge status={row.status} />
                <div className="text-xs text-muted-foreground">
                  {row.kind} · {row.action} · {formatDateTime(row.createdAtMs)}
                  {row.detail ? ` · ${row.detail}` : ""}
                  {row.status === "pending" ? " · interrupted or still running — check before retrying" : ""}
                </div>
              </div>
            </div>
          ))
        )}
      </Panel>

      <Panel title="Snapshots (one-click rollback)">
        {!history || history.snapshots.length === 0 ? (
          <EmptyState text="No snapshots yet" />
        ) : (
          history.snapshots.map((snap) => (
            <div key={snap.id} className="flex items-center justify-between gap-3 border-b border-border/60 py-2.5 text-sm">
              <div className="min-w-0">
                <span className="font-medium">{snap.target}</span>
                <div className="truncate text-xs text-muted-foreground" title={snap.payloadJson}>
                  {snap.kind} · {formatDateTime(snap.createdAtMs)}
                  {snap.rolledBackAtMs ? ` · rolled back ${formatDateTime(snap.rolledBackAtMs)}` : ""}
                </div>
              </div>
              <Button
                size="sm"
                variant="outline"
                disabled={snap.rolledBackAtMs !== null || busy === snap.id}
                onClick={() => void rollback(snap.id)}
              >
                {busy === snap.id ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Undo2 className="size-4" aria-hidden="true" />}
                Roll back
              </Button>
            </div>
          ))
        )}
      </Panel>
    </div>
  )
}
