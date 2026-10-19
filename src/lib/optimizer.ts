// WinClean 2.0 optimizer: typed mirror of the Rust optimizer commands.
// Types follow the serde camelCase output of src-tauri/src/optimizer/*.
// The helpers at the bottom are pure and unit-tested; policy decisions
// (what may be changed) are made by the backend and are never duplicated here.

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

export type ErrorKind =
  | "elevationRequired"
  | "rejected"
  | "conflict"
  | "notFound"
  | "failed"

export type CommandError = { code: ErrorKind; message: string }

function isCommandError(value: unknown): value is CommandError {
  return (
    typeof value === "object" &&
    value !== null &&
    "message" in value &&
    typeof (value as { message: unknown }).message === "string"
  )
}

/** Human-readable text for anything a command can reject with. */
export function describeError(error: unknown): string {
  if (isCommandError(error)) return error.message
  if (error instanceof Error) return error.message
  return String(error)
}

/** True when the backend refused because administrator rights are needed. */
export function isElevationRequired(error: unknown): boolean {
  return isCommandError(error) && error.code === "elevationRequired"
}

/** True when a reviewed plan no longer matches the live system. */
export function isConflict(error: unknown): boolean {
  return isCommandError(error) && error.code === "conflict"
}

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

export type ServiceRisk = "protected" | "caution" | "safe"
export type ServiceCategory =
  | "windowsCore"
  | "security"
  | "networking"
  | "driver"
  | "search"
  | "update"
  | "copilot"
  | "vendorControl"
  | "telemetry"
  | "gaming"
  | "thirdPartyUpdater"
  | "optional"
  | "unknown"
export type StartType = "automatic" | "automaticDelayed" | "manual" | "disabled"

export type ServiceClassification = {
  known: boolean
  risk: ServiceRisk
  category: ServiceCategory
  canDisable: boolean
  reason: string
  recommendation: string
}

export type ServiceInfo = {
  name: string
  displayName: string
  state: string
  startType: string
  delayedAutoStart: boolean
  pid: number | null
  dependencies: string[]
  dependents: string[]
  canChange: boolean
  ownProcess: boolean
  triggerStart: boolean
  memoryMb: number | null
  binaryPath: string | null
  classification: ServiceClassification
}

export type ServiceRecommendation = {
  serviceName: string
  displayName: string
  currentStartType: string
  suggestedStartType: string
  risk: ServiceRisk
  category: ServiceCategory
  rationale: string
  confidence: "high" | "medium"
  running: boolean
  memoryMb: number | null
  startsAtBoot: boolean
  requiresElevation: boolean
}

export type ServiceChange = { serviceName: string; startType: StartType }

export type PlanItem = {
  name: string
  displayName: string
  from: StartType
  to: StartType
  risk: ServiceRisk
  reason: string
}

export type ServicePlan = {
  items: PlanItem[]
  rejected: { serviceName: string; reason: string }[]
}

export type WriteCapability = "elevated" | "helper" | "unavailable"

export type PreparedPlanView = {
  token: string | null
  plan: ServicePlan
  fingerprint: string
  expiresAtMs: number
  writeCapability: WriteCapability
}

export type ServiceApplyOutcome = {
  snapshotId: number
  applied: { name: string; displayName: string; from: StartType; to: StartType }[]
  summary: string
}

export type HelperStatus = {
  supported: boolean
  connected: boolean
  processElevated: boolean
  locationTrusted: boolean
  protocolVersion: number
  lastError: string | null
}

// ---------------------------------------------------------------------------
// Processes and workload
// ---------------------------------------------------------------------------

export type EcoDecision = "protected" | "foreground" | "eligible" | "unknown"

export type ProcessInfo = {
  pid: number
  parentPid: number
  name: string
  workingSetMb: number
  cpuPercentRecent: number
  isForeground: boolean
  inForegroundTree: boolean
  ecoDecision: EcoDecision
  ecoQosApplied: boolean
}

export type WorkloadKind =
  | "general"
  | "coding"
  | "gaming"
  | "aiInference"
  | "aiDevelopment"

export type WorkloadSnapshot = {
  kind: WorkloadKind
  evidence: string[]
  foreground: string | null
}

// ---------------------------------------------------------------------------
// Battery, power, profiles
// ---------------------------------------------------------------------------

export type BatteryStatus = {
  present: boolean
  onAc: boolean
  acKnown: boolean
  percent: number | null
  secondsRemaining: number | null
  activeSchemeGuid: string | null
  activeSchemeName: string | null
}

export type BatteryInfo = {
  designCapacityMwh: number | null
  fullChargeCapacityMwh: number | null
  cycleCount: number | null
  chemistry: string | null
  capacityRelative: boolean
  healthPercent: number | null
}

export type BatteryReading = {
  acOnline: boolean
  charging: boolean
  discharging: boolean
  critical: boolean
  capacityMwh: number | null
  voltageMv: number | null
  rateMw: number | null
}

export type BatteryTelemetry = {
  present: boolean
  info: BatteryInfo | null
  reading: BatteryReading | null
  dischargeWatts: number | null
  chargeWatts: number | null
  averageDischargeWatts: number | null
  energyCrossCheckWatts: number | null
  estimatedRuntimeMinutes: number | null
  notes: string[]
}

export type TelemetrySample = {
  atMs: number
  percent: number | null
  onAc: boolean
  rateMw: number | null
  voltageMv: number | null
  capacityMwh: number | null
  profileId: string | null
  workload: string | null
}

export type TelemetryView = {
  telemetry: BatteryTelemetry
  status: BatteryStatus | null
  lastSample: TelemetrySample | null
}

export type PowerSample = { atMs: number; percent: number; onAc: boolean }

export type EcoQosMode = "off" | "conservative" | "battery"

export type PowerProfile = {
  id: string
  label: string
  schemeGuid: string | null
  ecoQosForBackground: boolean
  ecoQosMode: EcoQosMode
  builtin: boolean
  description: string
}

export type AutoSwitchConfig = {
  enabled: boolean
  onAcProfile: string
  onBatteryProfile: string
  lowBatteryPercent: number
  lowBatteryProfile: string
  gamingProfile: string
  aiDevelopmentProfile: string
  codingProfile: string
  respectGhelper: boolean
  minDwellSecs: number
  hysteresisPercent: number
  manualOverrideMinutes: number
}

export type ActiveProfile = {
  profileId: string
  appliedAtMs: number
  appliedBy: string
  snapshotId: number
}

export type ProfilesView = {
  profiles: PowerProfile[]
  active: ActiveProfile | null
  autoSwitch: AutoSwitchConfig
}

export type ProfileOutcome = {
  snapshotId: number
  profileId: string
  detail: string
}

export type WattStats = {
  meanW: number
  stdevW: number
  minW: number
  maxW: number
  samples: number
}

export type MeasurementVerdict =
  | "improved"
  | "worse"
  | "inconclusive"
  | "insufficientData"

export type MeasurementResult = {
  baseline: WattStats | null
  after: WattStats | null
  deltaW: number | null
  deltaPercent: number | null
  ci95W: number | null
  verdict: MeasurementVerdict
  notes: string[]
}

export type MeasurementRecord = {
  id: number
  label: string
  profileId: string
  baselineStartMs: number
  switchMs: number | null
  endedMs: number | null
  resultJson: string | null
  createdAtMs: number
}

export type MeasurementView = {
  record: MeasurementRecord
  result: MeasurementResult | null
}

// ---------------------------------------------------------------------------
// Hardware and capabilities
// ---------------------------------------------------------------------------

export type ClusterRole = "performance" | "efficiency" | "uniform"

export type CoreCluster = {
  role: ClusterRole
  label: string
  physicalCores: number
  logicalProcessors: number
  efficiencyClass: number
  l3Kb: number | null
}

export type CpuTopology = {
  physicalCores: number
  logicalProcessors: number
  smtEnabled: boolean
  hybridByEfficiencyClass: boolean
  clusters: CoreCluster[]
  l3Domains: { sizeKb: number; physicalCores: number; logicalProcessors: number }[]
  zenLayoutConfirmed: boolean
  notes: string[]
}

export type DgpuPower =
  | "awake"
  | "poweredDown"
  | "disabled"
  | "absent"
  | "unknown"

export type HardwareInfo = {
  cpuName: string
  cpuIsRyzenAi9Hx370: boolean
  cpuArchitectureNote: string
  logicalProcessors: number
  ramTotalMb: number | null
  topology: CpuTopology | null
  topologyError: string | null
  adapters: {
    name: string
    driverVersion: string | null
    driverDate: string | null
    hardwareId: string | null
    kind: string
  }[]
  dgpu: { present: boolean; power: DgpuPower; note: string }
  igpuOnly: boolean
  npu: {
    present: boolean
    name: string | null
    generation: string | null
    driverVersion: string | null
    driverDate: string | null
    hardwareId: string | null
    inferenceVerified: boolean
    note: string
  }
  ghelperDetected: boolean
  ghelper: { installed: boolean; running: boolean; coexistence: string }
  platform: {
    manufacturer: string | null
    productName: string | null
    biosVersion: string | null
    isAsus: boolean
    isZephyrusG16: boolean
  }
  dgpuPolicy: string
  sources: string[]
}

export type CheckStatus = "pass" | "fail" | "unknown" | "notRun"

export type EpCheck = {
  id: string
  label: string
  status: CheckStatus
  detail: string
}

export type EpAssessment = {
  name: string
  device: string
  vendor: string
  readiness: "unavailable" | "candidate" | "included"
  wakesDgpu: boolean
  checks: EpCheck[]
  summary: string
}

export type RouteEntry = {
  ep: string
  device: string
  status: "candidate" | "fallback" | "excluded"
  reason: string
}

export type ModelFit = {
  ep: string
  fit: "good" | "risky" | "unsupported" | "unknown"
  reasons: string[]
}

export type ModelProfile = {
  fileSizeBytes: number
  irVersion: number | null
  opset: number | null
  producer: string | null
  nodeCount: number
  operators: Record<string, number>
  initializerCount: number
  initializerTypes: Record<string, number>
  usesExternalData: boolean
  inputNames: string[]
  dynamicInputShapes: boolean
  quantized: boolean
  blockQuantized: boolean
  float16Weights: boolean
  genaiLike: boolean
}

export type CapabilityReport = {
  windowsBuild: number | null
  copilotPlus: {
    status: "bySpec" | "needsOsUpdate" | "unverified"
    note: string
  }
  providers: EpAssessment[]
  routeOnBattery: RouteEntry[]
  routeOnAc: RouteEntry[]
  dgpuPower: DgpuPower | null
  npuVerifiedForInference: boolean
  model: {
    path: string
    profile: ModelProfile | null
    fits: ModelFit[]
    error: string | null
  } | null
  notes: string[]
}

// ---------------------------------------------------------------------------
// MCP
// ---------------------------------------------------------------------------

export type ToolPolicy =
  | "metadataOnly"
  | "contentRead"
  | "destructive"
  | "unknown"

export type McpTool = {
  name: string
  description: string | null
  policy: ToolPolicy
}

export type McpServer = {
  id: string
  name: string
  description: string | null
  tools: McpTool[]
  isFileExplorer: boolean
  raw: unknown
}

export type McpDiscovery = {
  odrAvailable: boolean
  windowsBuild: number | null
  buildSupported: boolean | null
  servers: McpServer[]
  fileExplorerFound: boolean
  notes: string[]
  rawExcerpt: string | null
}

export type McpProbe = { serverInfo: unknown; tools: McpTool[] }

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

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
  activeProfile: ActiveProfile | null
}

export type RecoveryReport = {
  examined: number
  completed: number
  interrupted: number
  notes: string[]
}

/** Event emitted by the background runtime (`optimizer-event`). */
export type OptimizerEvent =
  | { kind: "profileChanged"; data: { profileId: string; reason: string; automatic: boolean; snapshotId: number } }
  | { kind: "autoSwitchError"; data: { profileId: string; message: string } }
  | { kind: "workload"; data: WorkloadSnapshot }
  | { kind: "telemetry"; data: TelemetrySample }

const EVENT_KINDS = new Set(["profileChanged", "autoSwitchError", "workload", "telemetry"])

/** Validates an `optimizer-event` payload; unknown shapes are ignored. */
export function parseOptimizerEvent(payload: unknown): OptimizerEvent | null {
  if (typeof payload !== "object" || payload === null) return null
  const { kind, data } = payload as { kind?: unknown; data?: unknown }
  if (typeof kind !== "string" || !EVENT_KINDS.has(kind)) return null
  if (typeof data !== "object" || data === null) return null
  return { kind, data } as OptimizerEvent
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

export const START_TYPES: StartType[] = ["automatic", "automaticDelayed", "manual", "disabled"]

export const START_TYPE_LABELS: Record<StartType, string> = {
  automatic: "Automatic",
  automaticDelayed: "Automatic (delayed)",
  manual: "Manual",
  disabled: "Disabled",
}

export function startTypeLabel(value: string): string {
  return START_TYPE_LABELS[value as StartType] ?? value
}

export const WORKLOAD_LABELS: Record<WorkloadKind, string> = {
  general: "General use",
  coding: "Coding",
  gaming: "Gaming",
  aiInference: "AI inference",
  aiDevelopment: "AI development",
}

/** Return the PIDs marked as having EcoQoS applied so their settings can be restored. */
export function ecoQosRestorePids(processes: { pid: number; ecoQosApplied: boolean }[]): number[] {
  return processes.filter((p) => p.ecoQosApplied).map((p) => p.pid)
}

/** Suggest gaming when requested, battery saver at 20% or less on battery, or balanced otherwise. */
export function suggestProfile(input: { onAc: boolean; percent: number | null; gaming?: boolean }): string {
  if (input.gaming) return "gaming"
  if (!input.onAc && input.percent !== null && input.percent <= 20) return "batterySaver"
  return "balanced"
}

/** "12.4 W", or an em dash when no real measurement exists. */
export function formatWatts(watts: number | null | undefined): string {
  if (watts === null || watts === undefined || !Number.isFinite(watts)) return "—"
  return `${watts.toFixed(watts >= 100 ? 0 : 1)} W`
}

/** "3 h 05 min" from minutes; null/NaN → em dash; absurd values are capped. */
export function formatRuntime(minutes: number | null | undefined): string {
  if (minutes === null || minutes === undefined || !Number.isFinite(minutes) || minutes < 0) return "—"
  const capped = Math.min(Math.round(minutes), 99 * 60)
  const h = Math.floor(capped / 60)
  const m = capped % 60
  return h > 0 ? `${h} h ${String(m).padStart(2, "0")} min` : `${m} min`
}

export function formatMwh(mwh: number | null | undefined): string {
  if (mwh === null || mwh === undefined || !Number.isFinite(mwh)) return "—"
  return `${(mwh / 1000).toFixed(1)} Wh`
}

/** Battery health wording; thresholds follow common OEM guidance, not a hidden score. */
export function healthLabel(percent: number | null | undefined): { label: string; tone: "good" | "fair" | "poor" | "unknown" } {
  if (percent === null || percent === undefined) return { label: "Unknown", tone: "unknown" }
  if (percent >= 85) return { label: "Good", tone: "good" }
  if (percent >= 70) return { label: "Fair", tone: "fair" }
  return { label: "Worn", tone: "poor" }
}

export type ServiceFilter = {
  query: string
  risk: ServiceRisk | "all"
  onlyRunning: boolean
  onlyChangeable: boolean
}

/** Filters the service list for the Services view. */
export function filterServices(services: ServiceInfo[], f: ServiceFilter): ServiceInfo[] {
  const q = f.query.trim().toLowerCase()
  return services.filter((s) => {
    if (q && !`${s.name} ${s.displayName}`.toLowerCase().includes(q)) return false
    if (f.risk !== "all" && s.classification.risk !== f.risk) return false
    if (f.onlyRunning && s.state !== "running") return false
    if (f.onlyChangeable && !s.canChange) return false
    return true
  })
}

/** Counts for the Services summary tiles. */
export function summarizeServices(services: ServiceInfo[]) {
  const running = services.filter((s) => s.state === "running").length
  const protectedCount = services.filter((s) => s.classification.risk === "protected").length
  const autoStart = services.filter((s) => s.startType === "automatic" || s.startType === "automaticDelayed").length
  const memoryMb = services.reduce((sum, s) => sum + (s.memoryMb ?? 0), 0)
  return { total: services.length, running, protectedCount, autoStart, memoryMb: Math.round(Number.isFinite(memoryMb) ? memoryMb : 0) }
}

/** Total working set the recommendations would stop holding (real values only). */
export function recommendationMemoryMb(recs: ServiceRecommendation[]): number {
  return Math.round(recs.filter((r) => r.running).reduce((sum, r) => sum + (r.memoryMb ?? 0), 0) * 10) / 10
}

/** A plan item becomes the change request sent to the backend. */
export function itemsToChanges(items: { name: string; to: StartType }[]): ServiceChange[] {
  return items.map((i) => ({ serviceName: i.name, startType: i.to }))
}

/** A snapshot can be rolled back while it has not been and is a kind the backend supports. */
export function canRollback(snapshot: OptimizationSnapshot): boolean {
  return snapshot.rolledBackAtMs === null && ["service", "serviceBatch", "powerScheme"].includes(snapshot.kind)
}

export type AuditTone = "ok" | "warn" | "bad" | "neutral"

export function auditTone(status: string): AuditTone {
  switch (status) {
    case "completed":
      return "ok"
    case "partial":
    case "interrupted":
    case "rolledBack":
    case "skipped":
      return "warn"
    case "failed":
      return "bad"
    default:
      return "neutral"
  }
}

/** Friendly name of the profile id in `profiles`, or the id itself. */
export function profileLabel(profiles: PowerProfile[], id: string | null | undefined): string {
  if (!id) return "None"
  return profiles.find((p) => p.id === id)?.label ?? id
}

export function ecoQosModeLabel(mode: EcoQosMode): string {
  return mode === "off" ? "Off" : mode === "battery" ? "Battery (widest)" : "Conservative"
}

/** Short explanation of why a model's route looks the way it does. */
export function routeHeadline(route: RouteEntry[]): string {
  const first = route.find((r) => r.status === "candidate")
  if (!first) return "CPU only"
  return `${first.ep.replace("ExecutionProvider", "")} (${first.device.toUpperCase()}) first`
}

// ---------------------------------------------------------------------------
// Custom profiles and measurements
// ---------------------------------------------------------------------------

/** Windows' own power schemes; a custom profile must name one of these (or any valid scheme GUID). */
export const POWER_SCHEMES = [
  { guid: "a1841308-3541-4fab-bc81-f71556f20b4a", label: "Power saver" },
  { guid: "381b4222-f694-41f0-9685-ff5bb260df2e", label: "Balanced" },
  { guid: "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c", label: "High performance" },
  { guid: "e9a42b02-d5df-448d-aa00-03f14749eb61", label: "Ultimate performance" },
] as const

/**
 * Builds a `custom:<slug>` id the backend accepts (letters, digits and dashes,
 * 8–48 characters in total), unique among `existing`.
 */
export function customProfileId(label: string, existing: string[]): string {
  const slug = label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 36)
    .replace(/-+$/g, "")
  const base = `custom:${slug || "profile"}`
  if (!existing.includes(base)) return base
  for (let n = 2; n < 100; n += 1) {
    const candidate = `${base}-${n}`
    if (!existing.includes(candidate)) return candidate
  }
  return `${base}-${Date.now().toString(36)}`
}

export function newCustomProfile(input: {
  label: string
  description: string
  schemeGuid: string
  ecoQosMode: EcoQosMode
  existingIds: string[]
}): PowerProfile {
  return {
    id: customProfileId(input.label, input.existingIds),
    label: input.label.trim(),
    schemeGuid: input.schemeGuid,
    ecoQosForBackground: input.ecoQosMode !== "off",
    ecoQosMode: input.ecoQosMode,
    builtin: false,
    description: input.description.trim(),
  }
}

/** Clamp a form value to an integer range; NaN falls back to `fallback`. */
export function clampInt(value: number, min: number, max: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback
  return Math.min(max, Math.max(min, Math.round(value)))
}

/** Plain-language result of an A/B measurement; never claims more than the data shows. */
export function describeMeasurement(result: MeasurementResult | null): { headline: string; tone: "good" | "warn" | "neutral" } {
  if (!result) return { headline: "Not finished yet.", tone: "neutral" }
  switch (result.verdict) {
    case "improved":
      return {
        headline: `Draw fell by ${formatWatts(Math.abs(result.deltaW ?? 0))}${result.deltaPercent != null ? ` (${Math.abs(result.deltaPercent).toFixed(0)}%)` : ""}, beyond measurement noise.`,
        tone: "good",
      }
    case "worse":
      return { headline: `Draw rose by ${formatWatts(Math.abs(result.deltaW ?? 0))}, beyond measurement noise.`, tone: "warn" }
    case "inconclusive":
      return { headline: "No difference larger than the measurement noise. Workloads varied too much to tell.", tone: "neutral" }
    default:
      return { headline: "Not enough battery readings to compare. Stay unplugged and try again for a few more minutes.", tone: "neutral" }
  }
}
