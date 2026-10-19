import { useEffect, useRef } from "react"
import { listen } from "@tauri-apps/api/event"

import { parseOptimizerEvent, type OptimizerEvent } from "@/lib/optimizer"

/**
 * Subscribes to the backend runtime's `optimizer-event` stream (automatic
 * profile switches, workload changes, live telemetry). Event-driven: the UI
 * refreshes when something happened instead of polling for it.
 */
export function useOptimizerEvents(handler: (event: OptimizerEvent) => void) {
  const latest = useRef(handler)
  useEffect(() => {
    latest.current = handler
  })
  useEffect(() => {
    let unlisten: (() => void) | undefined
    let cancelled = false
    listen<unknown>("optimizer-event", (e) => {
      const parsed = parseOptimizerEvent(e.payload)
      if (parsed) latest.current(parsed)
    })
      .then((fn) => {
        if (cancelled) {
          try {
            fn()
          } catch {
            // see cleanup below
          }
        } else {
          unlisten = fn
        }
      })
      .catch(() => {})
    return () => {
      cancelled = true
      try {
        unlisten?.()
      } catch {
        // The window may already be closing; there is nothing left to unsubscribe from.
      }
    }
  }, [])
}
