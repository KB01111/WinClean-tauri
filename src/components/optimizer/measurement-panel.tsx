import { useState } from "react"
import { Loader2 } from "lucide-react"
import { toast } from "sonner"

import { EmptyState, Field, SelectInput, TextInput } from "@/components/common"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { usePolling } from "@/hooks/use-polling"
import { api, formatError } from "@/lib/api"
import { formatDateTime } from "@/lib/format"
import {
  describeMeasurement,
  formatWatts,
  profileLabel,
  type MeasurementView,
  type PowerProfile,
  type WattStats,
} from "@/lib/optimizer"

function minutesSince(ms: number, now: number): string {
  const m = Math.max(0, Math.floor((now - ms) / 60_000))
  return m < 1 ? "less than a minute" : `${m} min`
}

function Stats({ title, stats }: { title: string; stats: WattStats | null }) {
  return (
    <div className="rounded-lg border border-border bg-card p-3">
      <div className="text-xs font-medium tracking-wide text-muted-foreground uppercase">{title}</div>
      <div className="mt-1 text-lg font-semibold tabular-nums">{stats ? formatWatts(stats.meanW) : "—"}</div>
      <div className="text-xs text-muted-foreground">{stats ? `± ${stats.stdevW.toFixed(1)} W · ${stats.samples} readings` : "no readings"}</div>
    </div>
  )
}

/**
 * Honest before/after power measurement. WinClean records the battery's real
 * discharge rate for a baseline window, applies the profile, records again and
 * compares the two with a confidence interval. It says "improved" only when
 * the difference is larger than the noise — otherwise it says so.
 */
export function MeasurementPanel({
  measurements,
  profiles,
  onChanged,
}: {
  measurements: MeasurementView[]
  profiles: PowerProfile[]
  onChanged: () => Promise<void> | void
}) {
  const open = measurements.find((m) => m.record.endedMs === null) ?? null
  const finished = measurements.filter((m) => m.record.endedMs !== null)
  const [label, setLabel] = useState("")
  const [profileId, setProfileId] = useState(profiles.find((p) => p.id === "batterySaver")?.id ?? profiles[0]?.id ?? "")
  const [busy, setBusy] = useState(false)
  const [now, setNow] = useState(() => Date.now())
  usePolling(() => setNow(Date.now()), open ? 15_000 : null)

  async function run(action: () => Promise<unknown>, success?: string) {
    setBusy(true)
    try {
      await action()
      if (success) toast.success(success)
      await onChanged()
    } catch (e) {
      toast.error(formatError(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-4">
      {open ? (
        <div className="rounded-lg border border-primary/40 bg-accent/50 p-4 text-sm">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium">{open.record.label}</span>
            <Badge tone="info">{open.record.switchMs === null ? "Recording baseline" : `Recording with ${profileLabel(profiles, open.record.profileId)}`}</Badge>
          </div>
          <p className="mt-1 text-muted-foreground">
            {open.record.switchMs === null
              ? `Baseline running for ${minutesSince(open.record.baselineStartMs, now)}. Stay unplugged and work as usual for at least 5 minutes, then switch.`
              : `Running with the new profile for ${minutesSince(open.record.switchMs, now)}. Keep doing the same kind of work for at least 5 minutes, then finish.`}
          </p>
          <div className="mt-3 flex flex-wrap gap-2">
            {open.record.switchMs === null ? (
              <Button disabled={busy} onClick={() => void run(() => api.optimizerMeasurementSwitch(open.record.id), `Switched to ${profileLabel(profiles, open.record.profileId)}.`)}>
                {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                Switch to {profileLabel(profiles, open.record.profileId)}
              </Button>
            ) : (
              <Button disabled={busy} onClick={() => void run(() => api.optimizerMeasurementFinish(open.record.id), "Measurement finished.")}>
                {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                Finish and compare
              </Button>
            )}
          </div>
        </div>
      ) : (
        <div className="grid gap-3 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
          <Field label="Name this measurement">
            <TextInput value={label} maxLength={60} onChange={(e) => setLabel(e.target.value)} placeholder="Battery Saver while browsing" />
          </Field>
          <Field label="Profile to compare against the current state">
            <SelectInput value={profileId} onChange={(e) => setProfileId(e.target.value)}>
              {profiles.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </SelectInput>
          </Field>
          <Button disabled={busy || !label.trim() || !profileId} onClick={() => void run(() => api.optimizerMeasurementStart(label.trim(), profileId), "Baseline started. Unplug the charger if it is connected.")}>
            {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
            Start baseline
          </Button>
        </div>
      )}

      {finished.length === 0 ? (
        <EmptyState text="No finished measurements yet." />
      ) : (
        <ul className="grid gap-3">
          {finished.map(({ record, result }) => {
            const summary = describeMeasurement(result)
            return (
              <li key={record.id} className="rounded-lg border border-border bg-card p-4 text-sm">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span className="font-medium">{record.label}</span>
                  <span className="text-xs text-muted-foreground">
                    {profileLabel(profiles, record.profileId)} · {formatDateTime(record.createdAtMs)}
                  </span>
                </div>
                <p className={`mt-1 ${summary.tone === "good" ? "text-emerald-700 dark:text-emerald-300" : summary.tone === "warn" ? "text-amber-700 dark:text-amber-300" : "text-muted-foreground"}`}>{summary.headline}</p>
                {result ? (
                  <div className="mt-3 grid gap-3 sm:grid-cols-3">
                    <Stats title="Before" stats={result.baseline} />
                    <Stats title="After" stats={result.after} />
                    <div className="rounded-lg border border-border bg-card p-3">
                      <div className="text-xs font-medium tracking-wide text-muted-foreground uppercase">Difference</div>
                      <div className="mt-1 text-lg font-semibold tabular-nums">{result.deltaW != null ? `${result.deltaW > 0 ? "+" : ""}${result.deltaW.toFixed(1)} W` : "—"}</div>
                      <div className="text-xs text-muted-foreground">{result.ci95W != null ? `95% CI ± ${result.ci95W.toFixed(1)} W` : "no interval"}</div>
                    </div>
                  </div>
                ) : null}
                {result?.notes.length ? (
                  <ul className="mt-2 list-disc pl-5 text-xs text-muted-foreground">
                    {result.notes.map((n) => (
                      <li key={n}>{n}</li>
                    ))}
                  </ul>
                ) : null}
              </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}
