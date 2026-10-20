import { useCallback, useEffect, useState } from "react"
import { Loader2, RefreshCw } from "lucide-react"
import { toast } from "sonner"

import { EmptyState, Metric, Notice, Panel } from "@/components/common"
import { PowerChart } from "@/components/optimizer/power-chart"
import { ProfileButtons } from "@/components/optimizer/profile-buttons"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { useOptimizerEvents } from "@/hooks/use-optimizer-events"
import { usePolling } from "@/hooks/use-polling"
import { useTelemetryLease } from "@/hooks/use-telemetry-lease"
import { api, formatError } from "@/lib/api"
import { appendSample } from "@/lib/telemetry-chart"
import {
  formatMwh,
  formatRuntime,
  formatWatts,
  healthLabel,
  suggestProfile,
  type PowerProfile,
  type ProfilesView,
  type TelemetrySample,
  type TelemetryView,
} from "@/lib/optimizer"

const HISTORY_LIMIT = 240

/**
 * Battery & Power. Watts come from the battery's own reported discharge rate
 * (IOCTL_BATTERY_QUERY_STATUS); when a battery does not report it the value is
 * shown as "—" — WinClean never estimates power it did not measure. Fast
 * sampling runs only while this view is visible (a lease the backend lets
 * expire on its own).
 */
export function PowerView() {
  useTelemetryLease(true)
  const [view, setView] = useState<TelemetryView | null>(null)
  const [samples, setSamples] = useState<TelemetrySample[]>([])
  const [profiles, setProfiles] = useState<ProfilesView | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  // Full load: history and profiles change rarely, so they are fetched on open,
  // on a manual refresh and when a profile changes — not on every live tick.
  const refresh = useCallback(async () => {
    try {
      const [v, history, p] = await Promise.all([
        api.optimizerBatteryTelemetry(),
        api.optimizerTelemetryHistory(HISTORY_LIMIT),
        api.optimizerListProfiles(),
      ])
      setView(v)
      setSamples(appendSample(history, v.lastSample, HISTORY_LIMIT))
      setProfiles(p)
      setError(null)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }, [])

  // Live tick: one cheap call; the newest sample extends the chart locally.
  const refreshLive = useCallback(async () => {
    try {
      const v = await api.optimizerBatteryTelemetry()
      setView(v)
      setSamples((current) => appendSample(current, v.lastSample, HISTORY_LIMIT))
      setError(null)
    } catch (e) {
      setError(formatError(e))
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])
  usePolling(refreshLive, 5_000)
  useOptimizerEvents((event) => {
    if (event.kind === "profileChanged") void refresh()
  })

  async function apply(profile: PowerProfile) {
    setBusyId(profile.id)
    try {
      const outcome = await api.optimizerApplyPowerProfile(profile.id)
      toast.success(`${profile.label}: ${outcome.detail}`)
      await refresh()
    } catch (e) {
      toast.error(formatError(e))
    } finally {
      setBusyId(null)
    }
  }

  const t = view?.telemetry
  const status = view?.status ?? null
  const health = healthLabel(t?.info?.healthPercent)
  const runtime = t?.estimatedRuntimeMinutes ?? (status?.secondsRemaining && status.secondsRemaining > 0 ? status.secondsRemaining / 60 : null)
  const runtimeSource = t?.estimatedRuntimeMinutes != null ? "From measured draw" : runtime != null ? "Windows estimate" : "Not available"
  const acKnown = status?.acKnown ?? false
  const suggested = status && acKnown ? suggestProfile({ onAc: status.onAc, percent: status.percent }) : null
  const draw = t?.dischargeWatts ?? (t?.chargeWatts != null ? t.chargeWatts : null)

  return (
    <div className="space-y-4">
      {error ? <Notice tone="danger" text={error} /> : null}
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-5">
        <Metric
          label="Battery"
          value={!status ? "—" : status.present ? `${status.percent ?? "—"}%` : "No battery"}
          hint={!status ? "" : !acKnown ? "AC status unknown" : status.onAc ? "Plugged in" : "On battery"}
        />
        <Metric
          label={t?.chargeWatts != null && t.dischargeWatts == null ? "Charging at" : "Power draw"}
          value={formatWatts(draw)}
          hint={t?.averageDischargeWatts != null ? `avg ${formatWatts(t.averageDischargeWatts)}` : "reported by the battery"}
        />
        <Metric label="Estimated runtime" value={formatRuntime(runtime)} hint={runtimeSource} />
        <Metric
          label="Battery health"
          value={t?.info?.healthPercent != null ? `${t.info.healthPercent.toFixed(0)}%` : "—"}
          hint={t?.info?.cycleCount != null ? `${health.label} · ${t.info.cycleCount} cycles` : health.label}
        />
        <Metric
          label="Full-charge capacity"
          value={formatMwh(t?.info?.fullChargeCapacityMwh)}
          hint={t?.info?.designCapacityMwh != null ? `design ${formatMwh(t.info.designCapacityMwh)}` : undefined}
        />
      </div>

      <Panel
        title="Power draw"
        action={
          <Button variant="outline" size="sm" onClick={() => void refresh()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Refresh
          </Button>
        }
      >
        {loading ? <EmptyState text="Reading the battery…" /> : <PowerChart samples={samples} />}
        {t && t.energyCrossCheckWatts != null && t.dischargeWatts != null ? (
          <p className="mt-2 text-xs text-muted-foreground">
            Cross-check from the change in stored energy: {formatWatts(t.energyCrossCheckWatts)}. The two should agree within a few watts; a large gap means the battery reports its rate coarsely.
          </p>
        ) : null}
        {t?.notes.length ? (
          <ul className="mt-2 list-disc space-y-0.5 pl-5 text-xs text-muted-foreground">
            {t.notes.map((n) => (
              <li key={n}>{n}</li>
            ))}
          </ul>
        ) : null}
      </Panel>

      <Panel
        title="Power profiles"
        action={status?.activeSchemeName ? <Badge tone="neutral">Windows plan: {status.activeSchemeName}</Badge> : undefined}
      >
        {profiles ? (
          <ProfileButtons profiles={profiles.profiles} activeId={profiles.active?.profileId ?? null} busyId={busyId} onApply={(p) => void apply(p)} suggestedId={suggested} />
        ) : (
          <EmptyState text="Loading profiles…" />
        )}
        <p className="mt-3 text-xs text-muted-foreground">
          Profiles switch the Windows power plan and the background-throttling mode. Every switch is snapshotted and can be rolled back in History &amp; Rollback. Automatic switching and measurements are in Optimization Profiles.
        </p>
      </Panel>
    </div>
  )
}
