import { useCallback, useEffect, useState } from "react"
import { BatteryCharging, Loader2, RefreshCw, Zap } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { EmptyState, Metric, Notice, Panel } from "@/components/common"
import { api, formatError } from "@/lib/api"
import type { BatteryStatus, OptimizerHistory, PowerSample } from "@/lib/optimizer"
import { suggestProfile } from "@/lib/optimizer"

/** Display battery telemetry, recorded samples, and built-in power-profile actions. */
export function PowerView() {
  const [battery, setBattery] = useState<BatteryStatus | null>(null)
  const [history, setHistory] = useState<PowerSample[]>([])
  const [profiles, setProfiles] = useState<OptimizerHistory["profiles"]>([])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  /** Refresh battery telemetry, recent samples, and available profiles, updating loading and error state. */
  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [b, h, hist] = await Promise.all([
        api.optimizerBatteryStatus(),
        api.optimizerPowerHistory(200),
        api.optimizerHistory(1),
      ])
      setBattery(b)
      setHistory(h)
      setProfiles(hist.profiles)
      setError(null)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  /** Apply a power profile through the backend, report its outcome, and reload telemetry on success. */
  async function apply(profileId: string, label: string) {
    setBusy(profileId)
    try {
      const msg = await api.optimizerApplyPowerProfile(profileId)
      toast.success(`${label}: ${msg}`)
      await load()
    } catch (e) {
      toast.error(formatError(e))
    } finally {
      setBusy(null)
    }
  }

  const acKnown = battery?.acKnown ?? false
  // No suggestion until AC status is known: nothing is marked "suggested now".
  const suggested = battery && acKnown
    ? suggestProfile({ onAc: battery.onAc, percent: battery.percent })
    : null
  const runtime =
    battery?.secondsRemaining && battery.secondsRemaining > 0
      ? `${Math.floor(battery.secondsRemaining / 3600)}h ${Math.floor((battery.secondsRemaining % 3600) / 60)}m (Windows estimate)`
      : "—"

  return (
    <div className="space-y-4">
      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <Metric label="Battery" value={!battery ? "—" : battery.present ? `${battery.percent ?? "—"}%` : "No battery"} hint={!battery ? "" : !acKnown ? "AC status unknown" : battery.onAc ? "On AC power" : "On battery"} />
        <Metric label="Active scheme" value={battery?.activeSchemeName ?? "—"} hint={battery?.activeSchemeGuid ?? ""} />
        <Metric label="Estimated runtime" value={runtime} hint="From GetSystemPowerStatus" />
        <Metric label="Suggested profile" value={suggested ?? "-"} hint={acKnown ? "Deterministic AC/DC + level policy" : "AC status unknown - no DC rules applied"} />
      </div>

      <Panel
        title="Battery & power (native telemetry)"
        action={
          <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Refresh
          </Button>
        }
      >
        {error ? <Notice tone="danger" text={error} /> : null}
        <p className="flex items-start gap-2 text-sm text-muted-foreground">
          {battery?.onAc ? <BatteryCharging className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> : <Zap className="mt-0.5 size-4 shrink-0" aria-hidden="true" />}
          Discharge watts are computed only from real sampled capacity drops stored in power history — never estimated from a model. Each refresh records one sample; leave this view open on battery to build a before/after comparison.
        </p>
        {history.length === 0 ? (
          <EmptyState text="No power samples yet — refresh on battery to start history" />
        ) : (
          <p className="mt-2 text-xs text-muted-foreground">
{history.length} sample(s) stored. Latest: {history[0]?.percent}% {history[0]?.onAc ? "(AC)" : "(DC)"}.
            Discharge watts need at least two on-battery samples plus the battery design capacity, which Windows does not expose to this build - so no watt figure is shown rather than estimated.
            Recent: {history.slice(0, 8).map((s) => `${s.percent}%${s.onAc ? " AC" : " DC"}`).join(" | ")}
          </p>
        )}
      </Panel>

      <Panel title="Optimization profiles">
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {profiles.map((profile) => (
            <div key={profile.id} className="rounded-lg border border-border bg-card p-3">
              <div className="font-medium">{profile.label}</div>
              <p className="mt-1 text-sm text-muted-foreground">{profile.description}</p>
              <p className="mt-1 text-xs text-muted-foreground">
                Scheme: {profile.schemeGuid ?? "custom"} · EcoQoS: {profile.ecoQosForBackground ? "on" : "off"}
                {suggested === profile.id ? " · suggested now" : ""}
              </p>
              <Button
                className="mt-3"
                size="sm"
                disabled={busy === profile.id || !profile.schemeGuid}
                onClick={() => void apply(profile.id, profile.label)}
              >
                {busy === profile.id ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                Apply (snapshots current scheme)
              </Button>
            </div>
          ))}
        </div>
      </Panel>
    </div>
  )
}
