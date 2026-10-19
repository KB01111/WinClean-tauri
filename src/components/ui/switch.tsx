import * as React from "react"
import { Switch as SwitchPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

/** Windows-11-style toggle (Radix switch: keyboard and screen-reader friendly). */
function Switch({ className, ...props }: React.ComponentProps<typeof SwitchPrimitive.Root>) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "peer inline-flex h-5 w-10 shrink-0 items-center rounded-full border border-border bg-muted shadow-xs transition-colors outline-none focus-visible:ring-3 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:border-primary data-[state=checked]:bg-primary",
        className
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb className="pointer-events-none block size-3.5 translate-x-0.5 rounded-full bg-foreground/70 transition-transform data-[state=checked]:translate-x-[1.35rem] data-[state=checked]:bg-primary-foreground motion-reduce:transition-none" />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
