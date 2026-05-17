import { invoke } from "@tauri-apps/api/core"

export const wincleanCommands = [
  "app_health",
  "begin_google_drive_auth",
  "cancel_offload",
  "cancel_scan",
  "category_summary",
  "cleanup_history",
  "cleanup_plan",
  "clear_scan_history",
  "delete_scan_run",
  "directory_children",
  "disconnect_google_drive",
  "execute_cleanup",
  "export_diagnostics",
  "export_report",
  "extension_summary",
  "file_search",
  "google_drive_status",
  "list_volumes",
  "offload_status",
  "open_recycle_bin",
  "recommendations",
  "scan_runs",
  "scan_status",
  "start_offload",
  "start_scan",
  "top_directories",
] as const

export type WinCleanCommand = (typeof wincleanCommands)[number]

export function invokeWinClean<T>(
  command: WinCleanCommand,
  args?: Record<string, unknown>
) {
  return invoke<T>(command, args)
}
