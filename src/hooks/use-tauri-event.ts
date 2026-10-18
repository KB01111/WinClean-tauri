import { useEffect, useRef } from "react"
import { listen } from "@tauri-apps/api/event"

/**
 * Subscribe to a Tauri event with the latest handler and release the listener on cleanup.
 * Ignore subscription failures so the hook can also render in a plain browser.
 */
export function useTauriEvent<T>(
  event: string,
  handler: (payload: T) => void
) {
  const handlerRef = useRef(handler)
  useEffect(() => {
    handlerRef.current = handler
  }, [handler])

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | null = null
    listen<T>(event, (tauriEvent) => {
      handlerRef.current(tauriEvent.payload)
    })
      .then((fn) => {
        if (disposed) {
          fn()
          return
        }
        unlisten = fn
      })
      .catch(() => {
        // Subscriptions only resolve inside the Tauri runtime; ignore
        // failures so plain-browser dev renders keep working.
      })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [event])
}
