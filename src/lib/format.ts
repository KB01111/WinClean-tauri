export type ScanState = "idle" | "running" | "completed" | "failed" | "canceled"

export type VolumeCapacity = {
  availableSpace?: number | null
  totalSpace?: number | null
}

export type ScanStatusLike = {
  state: ScanState
  rootPath?: string | null
}

export function formatBytes(value: number) {
  if (!Number.isFinite(value) || value <= 0) {
    return "0 B"
  }

  const units = ["B", "KB", "MB", "GB", "TB"]
  let size = value
  let unit = 0
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024
    unit += 1
  }
  return `${size >= 10 || unit === 0 ? size.toFixed(0) : size.toFixed(1)} ${units[unit]}`
}

export function formatNumber(value: number) {
  return new Intl.NumberFormat().format(value)
}

export function formatPercentUsed(volume: VolumeCapacity) {
  if (!volume.totalSpace || volume.availableSpace == null) {
    return "unknown"
  }
  const used = Math.max(volume.totalSpace - volume.availableSpace, 0)
  return `${Math.round((used / volume.totalSpace) * 100)}% used`
}

export function statusText(progress: ScanStatusLike) {
  if (progress.state === "running") {
    return `Scanning ${progress.rootPath ?? ""}`
  }
  if (progress.state === "completed") {
    return `Completed ${progress.rootPath ?? ""}`
  }
  if (progress.state === "failed") {
    return "Scan failed"
  }
  if (progress.state === "canceled") {
    return "Scan canceled"
  }
  return "Select a drive to start"
}

export function formatDateTime(value?: number | null) {
  if (!value) {
    return "Not recorded"
  }
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value))
}
