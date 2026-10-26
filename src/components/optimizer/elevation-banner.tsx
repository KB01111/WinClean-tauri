import { useState } from "react"
import { Loader2, ShieldAlert, ShieldCheck } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import type { HelperController } from "@/hooks/use-helper"

/**
 * Shows whether privileged changes are possible and offers the single UAC
 * prompt that enables them. The app stays unprivileged either way.
 */
export function ElevationBanner({ helper }: { helper: HelperController }) {
  const { status, busy, connect, disconnect } = helper
  const [confirmUntrusted, setConfirmUntrusted] = useState(false)
  if (!status) return null

  if (!status.supported) {
    return (
      <div className="mb-4 flex items-start gap-3 rounded-lg border border-border bg-muted/50 p-3 text-sm">
        <ShieldAlert className="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <p>Changing services needs Windows. You can still review recommendations.</p>
      </div>
    )
  }

  if (status.processElevated) {
    return (
      <div className="mb-4 flex items-center gap-3 rounded-lg border border-emerald-600/30 bg-emerald-500/10 p-3 text-sm">
        <ShieldCheck className="size-4 shrink-0 text-emerald-700 dark:text-emerald-300" aria-hidden="true" />
        <p>WinClean is running as administrator, so changes apply directly.</p>
      </div>
    )
  }

  if (status.connected) {
    return (
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-emerald-600/30 bg-emerald-500/10 p-3 text-sm">
        <div className="flex items-center gap-3">
          <ShieldCheck className="size-4 shrink-0 text-emerald-700 dark:text-emerald-300" aria-hidden="true" />
          <p>
            Administrator helper is running <Badge tone="good">only your approved changes</Badge>. It exits when WinClean closes or after 10 idle minutes.
          </p>
        </div>
        <Button variant="outline" size="sm" onClick={() => void disconnect()} disabled={busy}>
          Stop helper
        </Button>
      </div>
    )
  }

  return (
    <div className="mb-4 rounded-lg border border-amber-600/40 bg-amber-500/10 p-3 text-sm">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-start gap-3">
          <ShieldAlert className="mt-0.5 size-4 shrink-0 text-amber-700 dark:text-amber-300" aria-hidden="true" />
          <p className="max-w-2xl">
            Reading is free. <strong>Changing</strong> a service needs administrator rights. WinClean stays unprivileged and starts a small helper (one UAC prompt) that can only change the start type of non-protected services.
          </p>
        </div>
        <Button
          onClick={() => void connect(confirmUntrusted)}
          disabled={busy || (!status.locationTrusted && !confirmUntrusted)}
          size="sm"
        >
          {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <ShieldCheck className="size-4" aria-hidden="true" />}
          Enable administrator helper
        </Button>
      </div>
      {!status.locationTrusted ? (
        <label className="mt-3 flex items-start gap-2 text-xs">
          <input type="checkbox" className="mt-0.5 accent-[var(--primary)]" checked={confirmUntrusted} onChange={(e) => setConfirmUntrusted(e.target.checked)} />
          <span>
            WinClean is running from a folder your account can modify (a development or portable build). Elevating it there would let any program running as you replace the executable. I understand and still want to continue.
          </span>
        </label>
      ) : null}
      {status.lastError ? <p className="mt-2 text-xs text-destructive">{status.lastError}</p> : null}
    </div>
  )
}
