import { describe, expect, test } from "bun:test"

import { selectDefaultScanRoot } from "./volume-selection"

describe("volume selection", () => {
  test("defaults to the first ready local scan volume", () => {
    expect(
      selectDefaultScanRoot(
        [
          { rootPath: "A:\\", driveType: "removable", isReady: false },
          { rootPath: "D:\\", driveType: "cdrom", isReady: false },
          { rootPath: "C:\\", driveType: "fixed", isReady: true },
        ],
        ""
      )
    ).toBe("C:\\")
  })

  test("keeps the current target when it is still present", () => {
    expect(
      selectDefaultScanRoot(
        [
          { rootPath: "C:\\", driveType: "fixed", isReady: true },
          { rootPath: "E:\\", driveType: "removable", isReady: true },
        ],
        "E:\\"
      )
    ).toBe("E:\\")
  })

  test("falls back when the current target is not supported", () => {
    expect(
      selectDefaultScanRoot(
        [
          { rootPath: "Z:\\", driveType: "remote", isReady: true },
          { rootPath: "C:\\", driveType: "fixed", isReady: true },
        ],
        "Z:\\"
      )
    ).toBe("C:\\")
  })
})
