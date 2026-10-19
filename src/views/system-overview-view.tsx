import { ArrowRight, Loader2, RefreshCw, ShieldCheck } from "lucide-react"

import { CapacityBar, EmptyState, Metric, Panel } from "@/components/common"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { useSystemSnapshot } from "@/hooks/use-system-snapshot"
import type { VolumeInfo } from "@/lib/api"
import { formatBytes, formatPercentUsed } from "@/lib/format"
import { WORKLOAD_LABELS, formatRuntime, formatWatts, profileLabel, recommendationMemoryMb } from "@/lib/optimizer"
import { nextActions } from "@/lib/system-overview"
import { VIEW_META, type View } from "@/lib/views"

const DGPU_TEXT = {
  awake: "Awake",
  poweredDown: "Powered down",
  disabled: "Disabled",
  absent: "Not present",
  unknown: "Unknown",
} as const

/**
 * System Overview: one screen with the current power state, workload, active
 * profile and hardware, plus a short list of things worth doing. Every figure
 * is read from the system; anything unavailable shows as "—".
 */
export function SystemOverviewView({
  volumes,
  recoverableBytes,
  onNavigate,
}: {
  volumes: VolumeInfo[]
  recoverableBytes: number | null
  onNavigate: (view: View) => void
}) {
  const { snapshot, loading, reload } = useSystemSnapshot()
  const { battery, workload, profiles, hardware, recommendations, helper, recovery } = snapshot
  const t = battery?.telemetry
  const status = battery?.status ?? null
  const activeId = profiles?.active?.profileId ?? null
  const actions = nextActions({
    serviceRecommendations: recommendations?.length ?? 0,
    serviceRecommendationMb: recommendations ? recommendationMemoryMb(recommendations) : 0,
    battery: status ? { onAc: status.onAc, acKnown: status.acKnown, percent: status.percent } : null,
    activeProfileId: activeId,
    autoSwitchEnabled: profiles?.autoSwitch.enabled ?? true,
    recoverableBytes,
    workload: workload?.kind ?? null,
    dgpu: hardware?.dgpu.power ?? null,
    interruptedChanges: recovery?.interrupted ?? 0,
  })
  const draw = t?.dischargeWatts ?? t?.chargeWatts ?? null

  return (
    <div className="space-y-4">
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        <Metric
          label="Power"
          value={!status ? "—" : status.present ? `${status.percent ?? "—"}%` : "No battery"}
          hint={!status ? undefined : `${status.acKnown ? (status.onAc ? "Plugged in" : "On battery") : "AC unknown"} · ${formatWatts(draw)} · ${formatRuntime(t?.estimatedRuntimeMinutes)}`}
        />
        <Metric label="Workload" value={workload ? WORKLOAD_LABELS[workload.kind] : "—"} hint={workload?.foreground ?? undefined} />
        <Metric label="Profile" value={profiles ? profileLabel(profiles.profiles, activeId) : "—"} hint={profiles ? (profiles.autoSwitch.enabled ? "automatic switching on" : "manual") : undefined} />
        <Metric
          label="Graphics"
          value={hardware ? (hardware.igpuOnly ? "Integrated only" : `RTX: ${DGPU_TEXT[hardware.dgpu.power]}`) : "—"}
          hint={hardware?.adapters.find((a) => a.kind === "igpu")?.name}
        />
        <Metric label="NPU" value={!hardware ? "—" : hardware.npu.present ? (hardware.npu.generation ?? "Detected") : "Not detected"} hint={hardware?.npu.present ? "inference not verified" : undefined} />
        <Metric
          label="Administrator helper"
          value={!helper ? "—" : !helper.supported ? "Windows only" : helper.processElevated ? "Running as admin" : helper.connected ? "Running" : "Off"}
          hint={helper && helper.supported && !helper.connected && !helper.processElevated ? "starts when you approve a change" : undefined}
        />
      </div>

      <Panel
        title="Worth doing next"
        action={
          <Button variant="outline" size="sm" onClick={() => void reload()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Refresh
          </Button>
        }
      >
        {actions.length === 0 ? (
          <EmptyState text={loading ? "Checking this PC…" : "Nothing needs attention right now."} />
        ) : (
          <ul className="grid gap-2">
            {actions.map((a) => (
              <li key={a.id} className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-card p-3 text-sm">
                <div className="min-w-0">
                  <div className="flex items-center gap-2 font-medium">
                    {a.title}
                    {a.tone === "warn" ? <Badge tone="warn">attention</Badge> : null}
                  </div>
                  <p className="text-xs text-muted-foreground">{a.detail}</p>
                </div>
                <Button variant="outline" size="sm" onClick={() => onNavigate(a.view)}>
                  {VIEW_META[a.view].label}
                  <ArrowRight className="size-4" aria-hidden="true" />
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Panel>

      <div className="grid gap-4 xl:grid-cols-2">
        <Panel title="Storage" action={<Button variant="ghost" size="sm" onClick={() => onNavigate("overview")}>Open <ArrowRight className="size-4" aria-hidden="true" /></Button>}>
          {volumes.length === 0 ? (
            <EmptyState text="No volumes detected" />
          ) : (
            <ul className="grid gap-3">
              {volumes.map((v) => (
                <li key={v.rootPath}>
                  <div className="flex items-baseline justify-between gap-2 text-sm">
                    <span className="font-medium">{v.rootPath}</span>
                    <span className="text-xs text-muted-foreground">
                      {formatPercentUsed(v)}
                      {v.availableSpace != null ? ` · ${formatBytes(v.availableSpace)} free` : ""}
                    </span>
                  </div>
                  {v.totalSpace != null && v.availableSpace != null ? <CapacityBar className="mt-1" used={v.totalSpace - v.availableSpace} total={v.totalSpace} label={`${v.rootPath} capacity`} /> : null}
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <Panel title="What WinClean will not do">
          <ul className="grid gap-2 text-sm text-muted-foreground">
            {[
              "Change Defender, Windows Update, Windows Search, networking, audio or driver services.",
              "Write ASUS firmware, fan, GPU-mode or battery-limit settings — G-Helper keeps that job.",
              "Wake the RTX 4070 to look at it, or load CUDA/NVAPI.",
              "Change anything without a snapshot, verification and a way back.",
              "Let the AI assistant or an MCP client apply a change: they can only propose.",
            ].map((line) => (
              <li key={line} className="flex items-start gap-2">
                <ShieldCheck className="mt-0.5 size-4 shrink-0 text-primary" aria-hidden="true" />
                {line}
              </li>
            ))}
          </ul>
        </Panel>
      </div>
    </div>
  )
}
