// Navigation model: which views exist, how they are grouped and titled, and
// which of them belong to the storage (scan) workflow. Pure data so the
// sidebar, the page header and the tests all read from one place.

export type View =
  | "system"
  | "overview"
  | "explorer"
  | "cleanup"
  | "services"
  | "processes"
  | "power"
  | "hardware"
  | "profiles"
  | "insights"
  | "mcp"
  | "optimizerHistory"
  | "history"
  | "settings"

export type ViewMeta = {
  /** Short name used in the sidebar. */
  label: string
  /** Page heading. */
  title: string
  /** One sentence under the heading when the view is not a scan view. */
  description?: string
}

export const VIEW_META: Record<View, ViewMeta> = {
  system: { label: "System Overview", title: "System Overview", description: "Power, workload, hardware and what is worth doing next." },
  overview: { label: "Disk overview", title: "Storage overview" },
  explorer: { label: "Explorer", title: "Explorer" },
  cleanup: { label: "Recover space", title: "Recover space" },
  services: { label: "Windows Services", title: "Windows Services", description: "Review and tune service start types safely, with a snapshot and one-click rollback." },
  processes: { label: "Processes & EcoQoS", title: "Processes & EcoQoS", description: "Run background work on efficient cores without touching what you are using." },
  power: { label: "Battery & Power", title: "Battery & Power", description: "Real battery draw, health and runtime from the battery itself." },
  hardware: { label: "Hardware & NPU", title: "Hardware & NPU", description: "CPU topology, GPUs, NPU and Windows ML readiness — without waking the RTX 4070." },
  profiles: { label: "Optimization Profiles", title: "Optimization Profiles", description: "Apply, automate and measure power profiles." },
  insights: { label: "AI Insights", title: "AI Insights" },
  mcp: { label: "MCP Integrations", title: "MCP Integrations", description: "Windows on-device MCP servers and WinClean's own read-only MCP server." },
  optimizerHistory: { label: "History & Rollback", title: "History & Rollback", description: "Everything WinClean changed, and how to undo it." },
  history: { label: "Storage history", title: "Storage history" },
  settings: { label: "Settings", title: "Settings" },
}

export type NavGroup = { id: string; label: string; views: View[] }

export const NAV_GROUPS: NavGroup[] = [
  { id: "system", label: "System", views: ["system"] },
  { id: "storage", label: "Storage and Cleanup", views: ["overview", "explorer", "cleanup"] },
  { id: "optimize", label: "Optimize", views: ["services", "processes", "power", "hardware", "profiles"] },
  { id: "assistant", label: "Assistant", views: ["insights", "mcp"] },
  { id: "records", label: "Records", views: ["optimizerHistory", "history", "settings"] },
]

const STORAGE_VIEWS: ReadonlySet<View> = new Set<View>(["overview", "explorer", "cleanup", "insights", "history"])

/** The scan / refresh controls only make sense on the storage workflow's views. */
export function isStorageView(view: View): boolean {
  return STORAGE_VIEWS.has(view)
}

export const DEFAULT_VIEW: View = "system"
