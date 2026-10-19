import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"

import { api, formatError } from "@/lib/api"
import type { HelperStatus } from "@/lib/optimizer"

/**
 * State and actions for the administrator helper. The app itself stays
 * unprivileged; `connect()` shows one UAC prompt for the narrow helper process.
 */
export function useHelper() {
  const [status, setStatus] = useState<HelperStatus | null>(null)
  const [busy, setBusy] = useState(false)

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.optimizerHelperStatus())
    } catch {
      // The status call has no failure mode worth surfacing; keep the last value.
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const connect = useCallback(
    async (allowUntrustedLocation = false): Promise<boolean> => {
      setBusy(true)
      try {
        const next = await api.optimizerHelperConnect(allowUntrustedLocation)
        setStatus(next)
        toast.success("Administrator helper is running. Changes you approve can now be applied.")
        return next.connected
      } catch (error) {
        toast.error(formatError(error))
        await refresh()
        return false
      } finally {
        setBusy(false)
      }
    },
    [refresh]
  )

  const disconnect = useCallback(async () => {
    setBusy(true)
    try {
      setStatus(await api.optimizerHelperDisconnect())
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setBusy(false)
    }
  }, [])

  /** True when a privileged write could run right now. */
  const ready = status ? status.connected || status.processElevated : false

  return { status, busy, ready, refresh, connect, disconnect }
}

export type HelperController = ReturnType<typeof useHelper>
