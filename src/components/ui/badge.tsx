import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"

import { cn } from "@/lib/utils"

const badgeVariants = cva("inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-medium whitespace-nowrap", {
  variants: {
    tone: {
      neutral: "border-border bg-muted text-muted-foreground",
      good: "border-emerald-600/30 bg-emerald-500/10 text-emerald-800 dark:text-emerald-300",
      warn: "border-amber-600/40 bg-amber-500/10 text-amber-800 dark:text-amber-300",
      bad: "border-destructive/40 bg-destructive/10 text-destructive",
      info: "border-primary/30 bg-accent text-accent-foreground",
    },
  },
  defaultVariants: { tone: "neutral" },
})

export type BadgeTone = NonNullable<VariantProps<typeof badgeVariants>["tone"]>

function Badge({ className, tone, ...props }: React.ComponentProps<"span"> & VariantProps<typeof badgeVariants>) {
  return <span data-slot="badge" className={cn(badgeVariants({ tone }), className)} {...props} />
}

export { Badge }
