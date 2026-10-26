import { useEffect, useState } from "react"

import { api, type NativeInfo } from "@/lib/api"

let cached: Promise<NativeInfo | null> | null = null

/** Native window facts are fixed for the process lifetime, so they are fetched once. */
function loadNativeInfo(): Promise<NativeInfo | null> {
  cached ??= api.nativeInfo().catch(() => null)
  return cached
}

/**
 * Mirrors the native window material onto <html data-mica>. When the backend
 * managed to apply Mica, the CSS lets the system backdrop show through the
 * sidebar and page background; otherwise (older Windows, Linux, failure) the
 * opaque theme colours stay, so the app is always readable.
 */
export function useNativeWindow() {
  const [info, setInfo] = useState<NativeInfo | null>(null)
  useEffect(() => {
    let live = true
    void loadNativeInfo().then((value) => {
      if (live) setInfo(value)
    })
    return () => {
      live = false
    }
  }, [])
  useEffect(() => {
    document.documentElement.dataset.mica = info?.mica ? "true" : "false"
  }, [info])
  return info
}
