import { ArrowRight } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import { EmptyState } from "@/components/common"
import { startTypeLabel, type ServiceRecommendation } from "@/lib/optimizer"

/** Deterministic recommendations with a checkbox each; nothing is applied from here. */
export function RecommendationList({
  recommendations,
  selected,
  onToggle,
}: {
  recommendations: ServiceRecommendation[]
  selected: Set<string>
  onToggle: (serviceName: string) => void
}) {
  if (recommendations.length === 0) {
    return <EmptyState text="No safe start-type changes to recommend right now." />
  }
  return (
    <ul className="grid gap-2">
      {recommendations.map((r) => {
        const id = `rec-${r.serviceName}`
        return (
          <li key={r.serviceName} className="rounded-lg border border-border bg-card p-3">
            <div className="flex items-start gap-3">
              <input
                id={id}
                type="checkbox"
                className="mt-1 size-4 accent-[var(--primary)]"
                checked={selected.has(r.serviceName)}
                onChange={() => onToggle(r.serviceName)}
              />
              <label htmlFor={id} className="min-w-0 flex-1 cursor-pointer text-sm">
                <span className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{r.displayName}</span>
                  <Badge tone={r.confidence === "high" ? "good" : "info"}>{r.confidence} confidence</Badge>
                  {r.running ? <Badge tone="neutral">running{r.memoryMb !== null ? ` · ${r.memoryMb.toFixed(1)} MB` : ""}</Badge> : null}
                </span>
                <span className="mt-1 flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
                  <code>{r.serviceName}</code>
                  <span aria-hidden="true">·</span>
                  {startTypeLabel(r.currentStartType)}
                  <ArrowRight className="size-3" aria-hidden="true" />
                  <span className="font-medium text-foreground">{startTypeLabel(r.suggestedStartType)}</span>
                </span>
                <span className="mt-1.5 block text-xs text-muted-foreground">{r.rationale}</span>
              </label>
            </div>
          </li>
        )
      })}
    </ul>
  )
}
