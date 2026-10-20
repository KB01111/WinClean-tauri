import { useCallback, useEffect, useState } from "react"
import { Loader2, RefreshCw, Undo2 } from "lucide-react"
import { toast } from "sonner"

import { EmptyState, Notice, Panel, StatusBadge } from "@/components/common"
import { Button } from "@/components/ui/button"
import { api, formatError } from "@/lib/api"
import { formatDateTime } from "@/lib/format"
import { canRollback, profileLabel, type OptimizerHistory, type RecoveryReport } from "@/lib/optimizer"

const KIND_LABEL: Record<string, string> = {
  service: "Service",
  serviceBatch: "Service batch",
  powerScheme: "Power profile",
  ecoQos: "EcoQoS",
  profile: "Profile",
}

/**
 * History & Rollback: every change WinClean made, with what it was before.
 * Snapshots can be rolled back with one click; a rollback is verified and
 * itself audited. Changes interrupted by a crash are listed, never hidden.
 */
export function OptimizerHistoryView() {
  const [history, setHistory] = useState<OptimizerHistory | null>(null)
  const [recovery, setRecovery] = useState<RecoveryReport | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<number | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [h, r] = await Promise.all([api.optimizerHistory(100), api.optimizerRecoveryReport()])
      setHistory(h)
      setRecovery(r)
      setError(null)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  async function rollback(id: number) {
    setBusy(id)
    try {
      const message = await api.optimizerRollback(id)
      // A partial rollback keeps the snapshot retryable; do not call it a success.
      if (message.includes("available to retry")) toast.warning(message)
      else toast.success(message)
      await load()
    } catch (e) {
      toast.error(formatError(e))
    } finally {
      setBusy(null)
    }
  }

  const profiles = history?.profiles ?? []
  const rollbackable = history?.snapshots.filter(canRollback) ?? []

  return (
    <div className="space-y-4">
      {error ? <Notice tone="danger" text={error} /> : null}
      {recovery && recovery.interrupted > 0 ? (
        <Notice
          tone="warning"
          text={`${recovery.interrupted} change${recovery.interrupted === 1 ? " was" : "s were"} interrupted before finishing (for example by a crash). ${recovery.notes.join(" ")}`}
        />
      ) : null}

      <Panel
        title="Restore points"
        action={
          <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Refresh
          </Button>
        }
      >
        {history?.activeProfile ? (
          <p className="mb-3 text-sm text-muted-foreground">
            Active profile: <span className="font-medium text-foreground">{profileLabel(profiles, history.activeProfile.profileId)}</span> ({history.activeProfile.appliedBy}, {formatDateTime(history.activeProfile.appliedAtMs)})
          </p>
        ) : null}
        {loading && !history ? (
          <EmptyState text="Loading…" />
        ) : rollbackable.length === 0 ? (
          <EmptyState text="Nothing to roll back. Every change WinClean makes will appear here with its previous state." />
        ) : (
          <ul className="grid gap-2">
            {rollbackable.map((s) => (
              <li key={s.id} className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-card p-3 text-sm">
                <div className="min-w-0">
                  <div className="font-medium">
                    {KIND_LABEL[s.kind] ?? s.kind}: {s.target}
                  </div>
                  <div className="text-xs text-muted-foreground">{formatDateTime(s.createdAtMs)}</div>
                </div>
                <Button variant="outline" size="sm" disabled={busy !== null} onClick={() => void rollback(s.id)}>
                  {busy === s.id ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Undo2 className="size-4" aria-hidden="true" />}
                  Roll back
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Panel>

      <Panel title="Audit trail">
        {!history || history.audit.length === 0 ? (
          <EmptyState text="No optimizer changes yet" />
        ) : (
          <div className="max-h-[50vh] overflow-auto">
            <table className="w-full border-collapse text-sm">
              <thead className="sticky top-0 bg-card">
                <tr className="border-b border-border text-left text-xs text-muted-foreground uppercase">
                  <th className="py-2 font-semibold">When</th>
                  <th className="py-2 font-semibold">Change</th>
                  <th className="py-2 text-right font-semibold">Result</th>
                </tr>
              </thead>
              <tbody>
                {history.audit.map((a) => (
                  <tr key={a.id} className="border-b border-border/60 align-top">
                    <td className="py-2 pr-3 text-xs whitespace-nowrap text-muted-foreground">{formatDateTime(a.createdAtMs)}</td>
                    <td className="py-2 pr-3">
                      <span className="font-medium">
                        {KIND_LABEL[a.kind] ?? a.kind}: {a.target}
                      </span>
                      <span className="block text-xs text-muted-foreground">{a.action}</span>
                      {a.detail ? <span className="block text-xs text-muted-foreground">{a.detail}</span> : null}
                    </td>
                    <td className="py-2 text-right">
                      <StatusBadge status={a.status} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Panel>
    </div>
  )
}
