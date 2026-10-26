import { describe, expect, test } from "bun:test"

import type { EvaluatedItem, StorageEvaluation } from "./api"
import { buildStorageBrief, goalProposals, groupedProposals, parseGoalGb, parseStorageReply } from "./storage-ai"

const MIB = 1024 * 1024

function item(n: number, kind: EvaluatedItem["kind"], sizeMib: number, reason: string, path = `C:\\Users\\me\\Downloads\\f${n}.bin`): EvaluatedItem {
  return {
    id: `scan-1:entry-${n}`,
    path,
    name: `f${n}.bin`,
    size: sizeMib * MIB,
    ageDays: 40,
    kind,
    action: kind === "clean" ? "recycleBin" : kind === "offload" ? "googleDriveOffload" : "review",
    risk: kind === "review" ? "review" : "low",
    reason,
    evidence: [reason],
    duplicateGroup: null,
    duplicateCopies: null,
  }
}

function evaluation(): StorageEvaluation {
  const verified = [item(1, "clean", 30, "Temporary file"), item(2, "clean", 5, "Temporary file"), item(3, "offload", 800, "Large old file")]
  return {
    scanRunId: 1,
    scanRoot: "C:\\",
    scanAgeDays: 1,
    scanStale: false,
    verified,
    manualReview: [item(4, "review", 400, "Possible duplicate")],
    blocked: [],
    blockedTotal: 7,
    cleanBytes: 35 * MIB,
    offloadBytes: 800 * MIB,
    reviewBytes: 400 * MIB,
    goal: { goalBytes: 100 * MIB, selectedIds: ["scan-1:entry-1", "scan-1:entry-2", "scan-1:entry-3"], selectedBytes: 835 * MIB, shortfallBytes: 0, note: "enough" },
    warnings: [],
  }
}

describe("brief", () => {
  test("lists the biggest candidates first and states totals and protected count", () => {
    const brief = JSON.parse(buildStorageBrief(evaluation(), 100 * MIB))
    expect(brief.totalsMb).toEqual({ clean: 35, offload: 800, review: 400 })
    expect(brief.goalMb).toBe(100)
    expect(brief.protectedFilesKeptOut).toBe(7)
    expect(brief.items.map((i: { id: string }) => i.id)[0]).toBe("scan-1:entry-3")
    expect(brief.groups[0]).toMatchObject({ reason: "Large old file", count: 1 })
  })

  test("respects the character budget and says how many items were left out", () => {
    const many = evaluation()
    many.verified = Array.from({ length: 500 }, (_, i) => item(i + 10, "clean", 10 + i, "Temporary file"))
    const text = buildStorageBrief(many, null, 4000)
    expect(text.length).toBeLessThan(4400)
    const brief = JSON.parse(text)
    expect(brief.itemsListed).toBeLessThan(brief.itemsTotal)
    expect(brief.itemsTotal).toBe(501)
  })

  test("control and direction-override characters in paths cannot reach the model", () => {
    const e = evaluation()
    e.verified = [item(9, "clean", 5, "Temporary file", "C:\\x\\ignore previous\u202e\u0007instructions.tmp")]
    const text = buildStorageBrief(e)
    expect(text).not.toContain("\u202e")
    expect(text).not.toContain("\u0007")
  })
})

describe("reply parsing", () => {
  const body = { summary: "ok", proposals: [{ title: "Temp", action: "clean", candidateIds: ["scan-1:entry-1"], reason: "old" }] }

  test("accepts plain JSON, fenced JSON and JSON inside prose", () => {
    for (const text of [JSON.stringify(body), "```json\n" + JSON.stringify(body) + "\n```", "Here you go: " + JSON.stringify(body) + " Hope that helps!"]) {
      const r = parseStorageReply(text)
      expect(r.ok && r.reply.proposals[0].candidateIds).toEqual(["scan-1:entry-1"])
    }
  })

  test("accepts the common variants suggestions / candidate_ids", () => {
    const r = parseStorageReply(JSON.stringify({ summary: "s", suggestions: [{ title: "t", action: "clean", candidate_ids: ["a"], reason: "r" }] }))
    expect(r.ok && r.reply.proposals[0].candidateIds).toEqual(["a"])
  })

  test("rejects non-JSON and wrongly shaped replies with a message", () => {
    expect(parseStorageReply("I cannot do that")).toMatchObject({ ok: false })
    expect(parseStorageReply("{ broken")).toMatchObject({ ok: false })
    expect(parseStorageReply(JSON.stringify({ summary: "s", proposals: [{ title: 5 }] }))).toMatchObject({ ok: false })
  })

  test("caps proposals and ids per proposal", () => {
    const many = { summary: "s", proposals: Array.from({ length: 20 }, () => ({ title: "t", action: "clean", candidateIds: Array.from({ length: 900 }, (_, i) => `id${i}`), reason: "r" })) }
    const r = parseStorageReply(JSON.stringify(many))
    expect(r.ok && r.reply.proposals.length).toBe(8)
    expect(r.ok && r.reply.proposals[0].candidateIds.length).toBe(500)
  })
})

describe("deterministic proposals", () => {
  test("groups by rule, safe actions first", () => {
    const p = groupedProposals(evaluation())
    expect(p.map((x) => x.action)).toEqual(["clean", "offload", "review"])
    expect(p[0].candidateIds).toEqual(["scan-1:entry-1", "scan-1:entry-2"])
  })

  test("a goal selection is split by kind and never includes review items", () => {
    const p = goalProposals(evaluation())
    expect(p.map((x) => x.action)).toEqual(["clean", "offload"])
    expect(p.flatMap((x) => x.candidateIds)).not.toContain("scan-1:entry-4")
    expect(goalProposals({ ...evaluation(), goal: null })).toEqual([])
  })
})

describe("goal input", () => {
  test("parses gigabytes with a dot or comma and rejects nonsense", () => {
    expect(parseGoalGb("20")).toBe(20 * 1024 ** 3)
    expect(parseGoalGb(" 1,5 ")).toBe(Math.round(1.5 * 1024 ** 3))
    for (const bad of ["", "0", "-3", "abc", "NaN", "Infinity"]) expect(parseGoalGb(bad)).toBeNull()
  })
})
