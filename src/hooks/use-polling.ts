import { useEffect, useRef } from "react"

/**
 * Runs `callback` every `intervalMs` while the window is visible. Nothing runs
 * while the window is hidden or minimised, and a refresh happens as soon as it
 * becomes visible again — polling costs nothing when nobody is looking.
 * `intervalMs = null` disables it.
 */
export function usePolling(callback: () => void | Promise<void>, intervalMs: number | null) {
  const latest = useRef(callback)
  useEffect(() => {
    latest.current = callback
  })

  useEffect(() => {
    if (intervalMs === null) return
    let timer: number | undefined
    const start = () => {
      stop()
      timer = window.setInterval(() => void latest.current(), intervalMs)
    }
    const stop = () => {
      if (timer !== undefined) window.clearInterval(timer)
      timer = undefined
    }
    const onVisibility = () => {
      if (document.hidden) {
        stop()
      } else {
        void latest.current()
        start()
      }
    }
    if (!document.hidden) start()
    document.addEventListener("visibilitychange", onVisibility)
    return () => {
      stop()
      document.removeEventListener("visibilitychange", onVisibility)
    }
  }, [intervalMs])
}
