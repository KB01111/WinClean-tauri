import { Badge, type BadgeTone } from "@/components/ui/badge"
import { EmptyState, StatusBadge } from "@/components/common"
import {
  START_TYPES,
  startTypeLabel,
  type ServiceInfo,
  type ServiceRisk,
  type StartType,
} from "@/lib/optimizer"

const RISK_TONE: Record<ServiceRisk, BadgeTone> = { safe: "good", caution: "warn", protected: "bad" }

/** Start types the row's select offers; "disabled" only where the backend would allow it. */
function offeredTypes(svc: ServiceInfo): string[] {
  const base = START_TYPES.filter((t) => t !== "disabled" || svc.classification.canDisable || svc.startType === "disabled")
  return base.includes(svc.startType as StartType) ? base : [...base, svc.startType]
}

/**
 * Service rows. The select only *requests* a change: it opens the review
 * dialog, the select itself never applies anything.
 */
export function ServiceTable({
  services,
  busy,
  onRequestChange,
}: {
  services: ServiceInfo[]
  busy: boolean
  onRequestChange: (svc: ServiceInfo, startType: StartType) => void
}) {
  if (services.length === 0) return <EmptyState text="No services match the filters" />
  return (
    <ul className="max-h-[60vh] overflow-auto" aria-label="Windows services">
      {services.map((svc) => (
        <li key={svc.name} className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 py-2.5 text-sm">
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-medium">{svc.displayName}</span>
              <StatusBadge status={svc.state} />
              <Badge tone={RISK_TONE[svc.classification.risk]}>{svc.classification.risk}</Badge>
              {svc.triggerStart ? <Badge tone="info">trigger-start</Badge> : null}
              {svc.memoryMb !== null && svc.state === "running" ? <Badge>{svc.memoryMb.toFixed(1)} MB</Badge> : null}
            </div>
            <div className="mt-0.5 text-xs text-muted-foreground">
              <code>{svc.name}</code> · {svc.classification.reason}
              {svc.dependents.length ? ` · ${svc.dependents.length} dependent service${svc.dependents.length === 1 ? "" : "s"}` : ""}
            </div>
          </div>
          <select
            aria-label={`Start type for ${svc.displayName}`}
            value={svc.startType}
            disabled={!svc.canChange || busy}
            onChange={(e) => onRequestChange(svc, e.target.value as StartType)}
            className="h-9 rounded-md border border-input bg-card px-2 text-sm disabled:opacity-50"
          >
            {offeredTypes(svc).map((t) => (
              <option key={t} value={t}>
                {startTypeLabel(t)}
              </option>
            ))}
          </select>
        </li>
      ))}
    </ul>
  )
}
