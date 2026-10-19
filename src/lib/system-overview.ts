// Deterministic "what is worth doing next" for the System Overview. No AI and
// no scoring: each rule is a plain condition on facts the app already has, so
// the reason for every suggestion can be stated exactly.

import { formatBytes } from "@/lib/format"
import type { DgpuPower, WorkloadKind } from "@/lib/optimizer"
import type { View } from "@/lib/views"

export type NextAction = {
  id: string
  title: string
  detail: string
  view: View
  tone: "info" | "warn"
}

export type OverviewFacts = {
  serviceRecommendations: number
  serviceRecommendationMb: number
  battery: { onAc: boolean; acKnown: boolean; percent: number | null } | null
  activeProfileId: string | null
  autoSwitchEnabled: boolean
  recoverableBytes: number | null
  workload: WorkloadKind | null
  dgpu: DgpuPower | null
  interruptedChanges: number
}

export function nextActions(f: OverviewFacts): NextAction[] {
  const out: NextAction[] = []
  const onBattery = !!f.battery && f.battery.acKnown && !f.battery.onAc

  if (f.interruptedChanges > 0) {
    out.push({
      id: "interrupted",
      title: "Review interrupted changes",
      detail: `${f.interruptedChanges} change${f.interruptedChanges === 1 ? " was" : "s were"} cut off before finishing. Check what state they left behind.`,
      view: "optimizerHistory",
      tone: "warn",
    })
  }
  if (onBattery && f.battery?.percent != null && f.battery.percent <= 20 && f.activeProfileId !== "batterySaver" && !f.autoSwitchEnabled) {
    out.push({
      id: "low-battery",
      title: "Battery is low",
      detail: `${f.battery.percent}% left and the ${f.activeProfileId ?? "current"} profile is active. Battery Saver stretches what remains.`,
      view: "profiles",
      tone: "warn",
    })
  }
  if (onBattery && f.dgpu === "awake" && f.workload !== "gaming" && f.workload !== "aiInference") {
    out.push({
      id: "dgpu-awake",
      title: "The RTX 4070 is awake on battery",
      detail: "Windows reports the discrete GPU as powered while nothing here needs it. Find the app holding it (Task Manager › GPU) and close it to save power.",
      view: "hardware",
      tone: "warn",
    })
  }
  if (f.serviceRecommendations > 0) {
    const mem = f.serviceRecommendationMb > 0 ? ` and release about ${Math.round(f.serviceRecommendationMb)} MB` : ""
    out.push({
      id: "services",
      title: `${f.serviceRecommendations} service${f.serviceRecommendations === 1 ? "" : "s"} could start on demand`,
      detail: `Safe start-type changes that trim boot work${mem}. You review everything before it is applied.`,
      view: "services",
      tone: "info",
    })
  }
  if (f.recoverableBytes != null && f.recoverableBytes > 0) {
    out.push({
      id: "storage",
      title: `${formatBytes(f.recoverableBytes)} can be cleaned`,
      detail: "From your latest scan, sent to the Recycle Bin only after you confirm.",
      view: "cleanup",
      tone: "info",
    })
  }
  if (!f.autoSwitchEnabled) {
    out.push({
      id: "auto-switch",
      title: "Automatic profile switching is off",
      detail: "Turn it on to move between plugged-in, battery, gaming and coding profiles on its own. It is off by default and never overrides a manual choice.",
      view: "profiles",
      tone: "info",
    })
  }
  const rank = (a: NextAction) => (a.tone === "warn" ? 0 : 1)
  return out.sort((a, b) => rank(a) - rank(b)).slice(0, 5)
}
