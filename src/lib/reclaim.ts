// Overview helpers: how much of a disk is in use, and where recoverable space
// comes from — straight from WinClean's own recommendation rules, no model.

import type { RecommendationCandidate, VolumeInfo } from "@/lib/api"

export type ReclaimGroup = {
  kind: "clean" | "offload" | "review"
  reason: string
  count: number
  bytes: number
}

/** Recommendation candidates grouped by the rule that picked them, biggest safe wins first. */
export function reclaimGroups(candidates: Pick<RecommendationCandidate, "kind" | "reason" | "size">[], max = 8): ReclaimGroup[] {
  const groups = new Map<string, ReclaimGroup>()
  for (const c of candidates) {
    const key = `${c.kind}|${c.reason}`
    const group = groups.get(key) ?? { kind: c.kind, reason: c.reason, count: 0, bytes: 0 }
    group.count += 1
    group.bytes += c.size
    groups.set(key, group)
  }
  const rank = { clean: 0, offload: 1, review: 2 }
  return [...groups.values()].sort((a, b) => rank[a.kind] - rank[b.kind] || b.bytes - a.bytes).slice(0, max)
}

const norm = (path: string) => path.replace(/\//g, "\\").toLowerCase().replace(/\\+$/, "")

/** The volume a scan root lives on: the longest volume root that contains it. */
export function volumeForRoot(volumes: VolumeInfo[], rootPath: string | undefined): VolumeInfo | null {
  if (!rootPath) return null
  const root = norm(rootPath)
  let best: VolumeInfo | null = null
  for (const volume of volumes) {
    const base = norm(volume.rootPath)
    if (root === base || root.startsWith(`${base}\\`)) {
      if (!best || base.length > norm(best.rootPath).length) best = volume
    }
  }
  return best
}

export type DiskUsage = { total: number; used: number; free: number; scanned: number; unaccounted: number }

/** Used/free from Windows, and how much of the used space the scan could account for. */
export function diskUsage(volume: VolumeInfo | null, scannedBytes: number): DiskUsage | null {
  if (!volume || volume.totalSpace == null || volume.availableSpace == null || volume.totalSpace <= 0) return null
  const used = Math.max(0, volume.totalSpace - volume.availableSpace)
  const scanned = Math.min(scannedBytes, used)
  return { total: volume.totalSpace, used, free: volume.availableSpace, scanned, unaccounted: used - scanned }
}
