import { describe, expect, test } from "bun:test"

import {
  formatBytes,
  formatNumber,
  formatPercentUsed,
  statusText,
} from "./format"

describe("format helpers", () => {
  test("formats bytes with compact binary units", () => {
    expect(formatBytes(0)).toBe("0 B")
    expect(formatBytes(1024)).toBe("1.0 KB")
    expect(formatBytes(15 * 1024 * 1024)).toBe("15 MB")
  })

  test("formats drive usage when capacity is known", () => {
    expect(formatPercentUsed({ totalSpace: 100, availableSpace: 25 })).toBe(
      "75% used"
    )
    expect(
      formatPercentUsed({ totalSpace: undefined, availableSpace: null })
    ).toBe("unknown")
  })

  test("describes scan states for user-facing status text", () => {
    expect(statusText({ state: "running", rootPath: "C:\\" })).toBe(
      "Scanning C:\\"
    )
    expect(statusText({ state: "completed", rootPath: "D:\\" })).toBe(
      "Completed D:\\"
    )
    expect(statusText({ state: "failed" })).toBe("Scan failed")
  })

  test("uses locale number formatting", () => {
    expect(formatNumber(1234)).toMatch(/1[,. \s]234/)
  })
})
