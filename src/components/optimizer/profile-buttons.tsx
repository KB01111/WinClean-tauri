import { Loader2 } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ecoQosModeLabel, type PowerProfile } from "@/lib/optimizer"

/** One card per profile with an Apply button; the active profile is marked. */
export function ProfileButtons({
  profiles,
  activeId,
  busyId,
  onApply,
  suggestedId,
}: {
  profiles: PowerProfile[]
  activeId: string | null
  busyId: string | null
  onApply: (profile: PowerProfile) => void
  suggestedId?: string | null
}) {
  return (
    <ul className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
      {profiles.map((p) => {
        const active = p.id === activeId
        return (
          <li key={p.id} className={`flex flex-col justify-between rounded-lg border p-3 ${active ? "border-primary/60 bg-accent/60" : "border-border bg-card"}`}>
            <div>
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium">{p.label}</span>
                {active ? <Badge tone="good">Active</Badge> : null}
                {!active && suggestedId === p.id ? <Badge tone="info">Suggested</Badge> : null}
                {!p.builtin ? <Badge>Custom</Badge> : null}
              </div>
              <p className="mt-1 text-xs text-muted-foreground">{p.description}</p>
              <p className="mt-1 text-xs text-muted-foreground">Background throttling: {ecoQosModeLabel(p.ecoQosMode)}</p>
            </div>
            <Button className="mt-3 self-start" variant={active ? "outline" : "default"} size="sm" disabled={busyId !== null} onClick={() => onApply(p)}>
              {busyId === p.id ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
              {active ? "Re-apply" : "Apply"}
            </Button>
          </li>
        )
      })}
    </ul>
  )
}
