import { ArrowRight, Loader2, ShieldCheck, ShieldX } from "lucide-react"

import { Badge, type BadgeTone } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import type { HelperController } from "@/hooks/use-helper"
import { startTypeLabel, type PreparedPlanView, type ServiceRisk } from "@/lib/optimizer"

const RISK_TONE: Record<ServiceRisk, BadgeTone> = { safe: "good", caution: "warn", protected: "bad" }

/**
 * The approval step. It shows exactly what the backend validated (not what was
 * requested), why each change is allowed, and what was refused. "Apply" sends
 * only the single-use token, so what runs is what was reviewed here.
 */
export function PlanReviewDialog({
  prepared,
  title = "Review service changes",
  origin = "user",
  applying,
  helper,
  onApply,
  onClose,
}: {
  prepared: PreparedPlanView | null
  title?: string
  origin?: "user" | "ai"
  applying: boolean
  helper: HelperController
  onApply: (token: string) => void
  onClose: () => void
}) {
  const plan = prepared?.plan
  const canApply = !!prepared?.token && (plan?.items.length ?? 0) > 0
  const needsHelper = canApply && prepared?.writeCapability === "unavailable" && !helper.ready

  return (
    <Dialog open={prepared !== null} onOpenChange={(open) => (!open && !applying ? onClose() : undefined)}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            {origin === "ai"
              ? "The AI assistant proposed these changes. WinClean re-checked every one against its own safety rules; only the checked items below can run, and only if you approve."
              : "WinClean re-checked every change against its safety rules. Nothing happens until you approve."}
          </DialogDescription>
        </DialogHeader>

        {plan && plan.items.length > 0 ? (
          <ul className="grid gap-2" aria-label="Changes that will be applied">
            {plan.items.map((item) => (
              <li key={item.name} className="rounded-lg border border-border bg-card p-3 text-sm">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span className="font-medium">{item.displayName}</span>
                  <Badge tone={RISK_TONE[item.risk]}>{item.risk}</Badge>
                </div>
                <div className="mt-1 flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
                  <code>{item.name}</code>
                  <span aria-hidden="true">·</span>
                  <span>{startTypeLabel(item.from)}</span>
                  <ArrowRight className="size-3" aria-hidden="true" />
                  <span className="font-medium text-foreground">{startTypeLabel(item.to)}</span>
                </div>
                <p className="mt-1.5 text-xs text-muted-foreground">{item.reason}</p>
              </li>
            ))}
          </ul>
        ) : (
          <p className="rounded-lg border border-border bg-muted/40 p-3 text-sm text-muted-foreground">
            None of the requested changes passed WinClean's safety rules, so there is nothing to apply.
          </p>
        )}

        {plan && plan.rejected.length > 0 ? (
          <div>
            <h4 className="mb-1.5 flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
              <ShieldX className="size-3.5" aria-hidden="true" /> Refused ({plan.rejected.length})
            </h4>
            <ul className="grid gap-1 text-xs text-muted-foreground">
              {plan.rejected.map((r) => (
                <li key={r.serviceName}>
                  <code className="text-foreground">{r.serviceName}</code> — {r.reason}
                </li>
              ))}
            </ul>
          </div>
        ) : null}

        {canApply ? (
          <p className="flex items-start gap-2 text-xs text-muted-foreground">
            <ShieldCheck className="mt-0.5 size-3.5 shrink-0 text-primary" aria-hidden="true" />
            The previous start types are saved first, the result is verified by reading the services back, and the whole batch is undone automatically if any step fails. You can also roll it back later in History &amp; Rollback.
          </p>
        ) : null}

        <DialogFooter>
          {needsHelper ? (
            <Button variant="outline" onClick={() => void helper.connect(false)} disabled={helper.busy}>
              {helper.busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <ShieldCheck className="size-4" aria-hidden="true" />}
              Enable administrator helper first
            </Button>
          ) : null}
          <Button variant="outline" onClick={onClose} disabled={applying}>
            {canApply ? "Cancel" : "Close"}
          </Button>
          {canApply && prepared?.token ? (
            <Button onClick={() => onApply(prepared.token as string)} disabled={applying || needsHelper}>
              {applying ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
              Apply {plan?.items.length} change{plan?.items.length === 1 ? "" : "s"}
            </Button>
          ) : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
