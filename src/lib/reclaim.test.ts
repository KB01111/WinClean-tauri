import { describe, expect, test } from "bun:test"

import type { VolumeInfo } from "./api"
import { diskUsage, reclaimGroups, volumeForRoot } from "./reclaim"

const GIB = 1024 ** 3
const volume = (rootPath: string, total: number | null, free: number | null): VolumeInfo => ({ name: rootPath, rootPath, driveType: "fixed", isReady: true, totalSpace: total, availableSpace: free })

describe("reclaim groups", () => {
  test("sum by rule and put safe wins before offload and review", () => {
    const groups = reclaimGroups([
      { kind: "review", reason: "Possible duplicate", size: 900 },
      { kind: "clean", reason: "Temporary file", size: 10 },
      { kind: "clean", reason: "Temporary file", size: 20 },
      { kind: "offload", reason: "Large old file", size: 500 },
      { kind: "clean", reason: "Installer", size: 100 },
    ])
    expect(groups.map((g) => [g.kind, g.reason, g.count, g.bytes])).toEqual([
      ["clean", "Installer", 1, 100],
      ["clean", "Temporary file", 2, 30],
      ["offload", "Large old file", 1, 500],
      ["review", "Possible duplicate", 1, 900],
    ])
  })
})

describe("disk usage", () => {
  test("finds the volume that contains the scan root, case-insensitively", () => {
    const volumes = [volume("C:\\", 1, 1), volume("D:\\", 1, 1)]
    expect(volumeForRoot(volumes, "d:\\Projects")?.rootPath).toBe("D:\\")
    expect(volumeForRoot(volumes, "C:\\")?.rootPath).toBe("C:\\")
    expect(volumeForRoot(volumes, "E:\\")).toBeNull()
    expect(volumeForRoot(volumes, undefined)).toBeNull()
  })

  test("reports what the scan could and could not account for", () => {
    const usage = diskUsage(volume("C:\\", 500 * GIB, 100 * GIB), 350 * GIB)
    expect(usage).toEqual({ total: 500 * GIB, used: 400 * GIB, free: 100 * GIB, scanned: 350 * GIB, unaccounted: 50 * GIB })
    expect(diskUsage(volume("C:\\", 500 * GIB, 100 * GIB), 900 * GIB)?.unaccounted).toBe(0)
    expect(diskUsage(volume("C:\\", null, null), 1)).toBeNull()
    expect(diskUsage(null, 1)).toBeNull()
  })
})
