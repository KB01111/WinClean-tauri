import { CircleCheck, CircleHelp, CircleMinus, CircleX } from "lucide-react"

import type { CheckStatus, EpCheck } from "@/lib/optimizer"

const ICON: Record<CheckStatus, { Icon: typeof CircleCheck; className: string; label: string }> = {
  pass: { Icon: CircleCheck, className: "text-emerald-600 dark:text-emerald-400", label: "Passed" },
  fail: { Icon: CircleX, className: "text-destructive", label: "Failed" },
  unknown: { Icon: CircleHelp, className: "text-amber-600 dark:text-amber-400", label: "Could not be determined" },
  notRun: { Icon: CircleMinus, className: "text-muted-foreground", label: "Not run" },
}

/**
 * Layered readiness checks. "Not run" is shown as such: WinClean never
 * pretends a check passed that it did not perform.
 */
export function CheckList({ checks }: { checks: EpCheck[] }) {
  return (
    <ul className="grid gap-1.5">
      {checks.map((check) => {
        const { Icon, className, label } = ICON[check.status]
        return (
          <li key={check.id} className="flex items-start gap-2 text-sm">
            <Icon className={`mt-0.5 size-4 shrink-0 ${className}`} aria-label={label} role="img" />
            <span className="min-w-0">
              <span className="font-medium">{check.label}</span>
              <span className="block text-xs text-muted-foreground">{check.detail}</span>
            </span>
          </li>
        )
      })}
    </ul>
  )
}
