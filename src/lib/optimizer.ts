// WinClean 2.0 optimizer: typed mirror of the Rust optimizer modules.
// Pure helpers here mirror src-tauri/src/optimizer/policy.rs exactly so the
// AI layer and UI can validate without a backend round-trip.

export type ServiceRisk = "protected" | "caution" | "safe"
export type StartType = "automatic" | "automaticDelayed" | "manual" | "disabled"

export type ServiceInfo = {
  name: string
  displayName: string
  state: string
  startType: string
  delayedAutoStart: boolean
  pid: number | null
  dependencies: string[]
  canChange: boolean
  classification: { risk: ServiceRisk; known: boolean; reason: string; recommendation: string }
}

export type ProcessInfo = {
  pid: number
  name: string
  workingSetMb: number
  cpuPercentRecent: number
  isForeground: boolean
  ecoDecision: "protected" | "foreground" | "eligible" | "unknown"
  ecoQosApplied: boolean
}

export type BatteryStatus = {
  present: boolean
  onAc: boolean
  percent: number | null
  secondsRemaining: number | null
  acKnown: boolean
  activeSchemeGuid: string | null
  activeSchemeName: string | null
}

export type PowerSample = { atMs: number; percent: number; onAc: boolean }

export type PowerProfile = {
  id: string
  label: string
  schemeGuid: string | null
  ecoQosForBackground: boolean
  description: string
}

export type HardwareInfo = {
  cpuName: string
  cpuIsRyzenAi9Hx370: boolean
  cpuArchitectureNote: string
  logicalProcessors: number
  ramTotalMb: number | null
  adapters: { name: string; driverVersion: string | null; kind: string }[]
  npu: {
    present: boolean
    name: string | null
    inferenceVerified: boolean
    note: string
  }
  ghelperDetected: boolean
  dgpuPolicy: string
}

export type CapabilityReport = {
  windowsBuild: number | null
  /** null = unverified: registry-only discovery cannot measure the 40+ TOPS requirement. */
  copilotPlusCapable: boolean | null
  mcpAgentRegistryFound: boolean
  mcpRegistryPaths: string[]
  fileExplorerMcp: string
  executionProviders: { name: string; status: string; detail: string }[]
  inferenceRoute: string[]
  npuVerifiedForInference: boolean
  notes: string[]
}

export type OptimizationAudit = {
  id: number
  kind: string
  target: string
  action: string
  status: string
  detail: string | null
  snapshotId: number | null
  createdAtMs: number
  completedAtMs: number | null
}

export type OptimizationSnapshot = {
  id: number
  kind: string
  target: string
  payloadJson: string
  createdAtMs: number
  rolledBackAtMs: number | null
}

export type OptimizerHistory = {
  audit: OptimizationAudit[]
  snapshots: OptimizationSnapshot[]
  profiles: PowerProfile[]
}

const START_TYPES: StartType[] = [
  "automatic",
  "automaticDelayed",
  "manual",
  "disabled",
]

/** Deterministic gate for AI service suggestions. Nothing reaches the
 * backend unless this accepts it, and the backend re-checks anyway. */
export function validateAiServiceSuggestions(
  suggestions: { serviceName: string; startType: string }[],
  services: ServiceInfo[]
): { accepted: typeof suggestions; rejected: string[] } {
  const accepted: typeof suggestions = []
  const rejected: string[] = []
  for (const s of suggestions) {
    const svc = services.find((x) => x.name === s.serviceName)
    if (!svc) {
      rejected.push(`${s.serviceName}: unknown service`)
    } else if (svc.classification.risk === "protected" || !svc.canChange) {
      rejected.push(`${s.serviceName}: protected service, never modified`)
    } else if (!START_TYPES.includes(s.startType as StartType)) {
      rejected.push(`${s.serviceName}: invalid start type '${s.startType}'`)
    } else {
      accepted.push(s)
    }
  }
  return { accepted, rejected }
}

/** Return the PIDs marked as having EcoQoS applied so their settings can be restored. */
export function ecoQosRestorePids(
  processes: { pid: number; ecoQosApplied: boolean }[]
): number[] {
  return processes.filter((p) => p.ecoQosApplied).map((p) => p.pid)
}

/** Suggest gaming when requested, battery saver at 20% or less on battery, or balanced otherwise. */
export function suggestProfile(input: {
  onAc: boolean
  percent: number | null
  gaming?: boolean
}): string {
  if (input.gaming) return "gaming"
  if (!input.onAc && input.percent !== null && input.percent <= 20)
    return "batterySaver"
  return "balanced"
}

/** Mirrors optimizer/power.rs: watts only from a real discharge drop. */
export function dischargeWatts(
  prev: PowerSample,
  next: PowerSample,
  designCapacityMwh: number | null
): number | null {
  if (prev.onAc || next.onAc || next.atMs <= prev.atMs) return null
  if (next.percent >= prev.percent || designCapacityMwh === null) return null
  const hours = (next.atMs - prev.atMs) / 3_600_000
  const w = ((prev.percent - next.percent) / 100) * designCapacityMwh / hours / 1000
  return Number.isFinite(w) && w > 0 ? Math.round(w * 10) / 10 : null
}
