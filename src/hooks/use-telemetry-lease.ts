import { useEffect } from "react"

import { api } from "@/lib/api"

/** The backend lease lasts 30 s; renewing every 20 s keeps fast sampling alive. */
const RENEW_MS = 20_000

/**
 * While `active` and the window is visible, asks the backend for fast battery
 * sampling. Releasing it (unmount, hidden, inactive) drops sampling back to
 * the slow background rate immediately.
 */
export function useTelemetryLease(active: boolean) {
  useEffect(() => {
    if (!active) return
    let timer: number | undefined
    const grant = () => void api.optimizerTelemetryLease(true).catch(() => {})
    const release = () => void api.optimizerTelemetryLease(false).catch(() => {})
    const onVisibility = () => {
      if (document.hidden) {
        if (timer !== undefined) window.clearInterval(timer)
        timer = undefined
        release()
      } else {
        grant()
        timer = window.setInterval(grant, RENEW_MS)
      }
    }
    if (!document.hidden) {
      grant()
      timer = window.setInterval(grant, RENEW_MS)
    }
    document.addEventListener("visibilitychange", onVisibility)
    return () => {
      if (timer !== undefined) window.clearInterval(timer)
      document.removeEventListener("visibilitychange", onVisibility)
      release()
    }
  }, [active])
}
