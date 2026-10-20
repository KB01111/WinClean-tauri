// Structured output of the AI optimization assistant, and the deterministic
// gate between "the model said so" and "the user is asked to approve it".
//
// The assistant can only PROPOSE. Nothing here changes anything: proposals are
// filtered against the live service list, then sent to the backend, which
// validates them again with the real policy and issues a single-use approval
// token. Only the user's explicit click applies a plan.

import { z } from "zod"

import type { PowerProfile, ServiceChange, ServiceInfo, StartType } from "@/lib/optimizer"
import { START_TYPES } from "@/lib/optimizer"

export const findingSchema = z.object({
  area: z.enum(["services", "processes", "battery", "hardware", "storage"]),
  severity: z.enum(["info", "attention", "important"]),
  title: z.string().describe("Short title"),
  detail: z.string().describe("One or two sentences with the evidence from the tools"),
})

export const systemReportSchema = z.object({
  summary: z.string().describe("A short overview of the system's efficiency situation"),
  findings: z.array(findingSchema),
  serviceProposals: z.array(
    z.object({
      serviceName: z.string().describe("Exact service name returned by get_services"),
      startType: z.enum(["automatic", "automaticDelayed", "manual", "disabled"]),
      reason: z.string(),
    })
  ),
  profileProposal: z
    .object({ profileId: z.string().describe("A profile id returned by get_profiles"), reason: z.string() })
    .nullable(),
})

export type SystemReport = z.infer<typeof systemReportSchema>

export type NormalizedProposals = {
  serviceChanges: ServiceChange[]
  reasons: Record<string, string>
  profile: { profileId: string; label: string; reason: string } | null
  /** Why each dropped proposal was dropped (shown to the user). */
  dropped: string[]
}

/**
 * First-pass filter. It removes proposals that are obviously unusable (unknown
 * service, protected service, repeated entries, invalid start type, unknown
 * profile). The backend repeats and extends these checks authoritatively, so
 * this only keeps the review list clean — it is never the security boundary.
 */
export function normalizeProposals(
  report: Pick<SystemReport, "serviceProposals" | "profileProposal">,
  services: ServiceInfo[],
  profiles: PowerProfile[]
): NormalizedProposals {
  const serviceChanges: ServiceChange[] = []
  const reasons: Record<string, string> = {}
  const dropped: string[] = []
  const seen = new Set<string>()
  for (const p of report.serviceProposals) {
    const svc = services.find((s) => s.name.toLowerCase() === p.serviceName.trim().toLowerCase())
    if (!svc) {
      dropped.push(`${p.serviceName}: not a service on this PC`)
    } else if (svc.classification.risk === "protected" || !svc.canChange) {
      dropped.push(`${svc.displayName}: protected, never changed`)
    } else if (!START_TYPES.includes(p.startType as StartType)) {
      dropped.push(`${svc.displayName}: invalid start type '${p.startType}'`)
    } else if (seen.has(svc.name.toLowerCase())) {
      dropped.push(`${svc.displayName}: proposed more than once`)
    } else if (svc.startType === p.startType) {
      dropped.push(`${svc.displayName}: already ${p.startType}`)
    } else {
      seen.add(svc.name.toLowerCase())
      serviceChanges.push({ serviceName: svc.name, startType: p.startType as StartType })
      reasons[svc.name] = p.reason
    }
  }
  let profile: NormalizedProposals["profile"] = null
  if (report.profileProposal) {
    const found = profiles.find((p) => p.id === report.profileProposal!.profileId)
    if (found) profile = { profileId: found.id, label: found.label, reason: report.profileProposal.reason }
    else dropped.push(`Profile '${report.profileProposal.profileId}': not an available profile`)
  }
  return { serviceChanges, reasons, profile, dropped }
}
