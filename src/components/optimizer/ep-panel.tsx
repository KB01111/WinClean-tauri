import { ArrowRight } from "lucide-react"

import { CheckList } from "@/components/optimizer/check-list"
import { Badge, type BadgeTone } from "@/components/ui/badge"
import type { CapabilityReport, EpAssessment, RouteEntry } from "@/lib/optimizer"

const READINESS: Record<EpAssessment["readiness"], { label: string; tone: BadgeTone }> = {
  included: { label: "Candidate (not session-tested)", tone: "info" },
  candidate: { label: "Candidate", tone: "info" },
  unavailable: { label: "Unavailable", tone: "neutral" },
}

const ROUTE_TONE: Record<RouteEntry["status"], BadgeTone> = { candidate: "good", fallback: "neutral", excluded: "warn" }

function Route({ title, route }: { title: string; route: RouteEntry[] }) {
  return (
    <div>
      <h4 className="mb-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">{title}</h4>
      <ol className="grid gap-1.5 text-sm">
        {route.map((r, i) => (
          <li key={`${r.ep}-${i}`} className="flex flex-wrap items-start gap-2">
            <span className="flex items-center gap-1.5">
              {i > 0 ? <ArrowRight className="size-3 text-muted-foreground" aria-hidden="true" /> : null}
              <span className="font-medium">{r.ep.replace("ExecutionProvider", "")}</span>
              <Badge tone={ROUTE_TONE[r.status]}>{r.status}</Badge>
            </span>
            <span className="min-w-0 text-xs text-muted-foreground">{r.reason}</span>
          </li>
        ))}
      </ol>
    </div>
  )
}

/**
 * Windows ML readiness per execution provider, and the routing WinClean would
 * prefer on battery and on AC. Everything here is an assessment of what is
 * installed and declared — no inference session is created, so nothing is
 * ever labelled "verified".
 */
export function EpPanel({ report }: { report: CapabilityReport }) {
  return (
    <div className="space-y-5">
      <div className="grid gap-4 md:grid-cols-2">
        <Route title="Preferred on battery" route={report.routeOnBattery} />
        <Route title="Preferred on AC" route={report.routeOnAc} />
      </div>
      <div className="grid gap-3 lg:grid-cols-2">
        {report.providers.map((ep) => (
          <section key={ep.name} className="rounded-lg border border-border bg-card p-3">
            <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
              <h4 className="text-sm font-medium">
                {ep.name.replace("ExecutionProvider", "")} <span className="font-normal text-muted-foreground">· {ep.device.toUpperCase()} · {ep.vendor}</span>
              </h4>
              <Badge tone={READINESS[ep.readiness].tone}>{READINESS[ep.readiness].label}</Badge>
            </div>
            <p className="mb-2 text-xs text-muted-foreground">{ep.summary}</p>
            <CheckList checks={ep.checks} />
            {ep.wakesDgpu ? <p className="mt-2 text-xs text-amber-700 dark:text-amber-300">Would use the RTX 4070; WinClean only includes it when it is already awake or you consent.</p> : null}
          </section>
        ))}
      </div>
    </div>
  )
}
