import { useCallback, useEffect, useState } from "react"

import { useOptimizerEvents } from "@/hooks/use-optimizer-events"
import { usePolling } from "@/hooks/use-polling"
import { api } from "@/lib/api"
import type {
  HardwareInfo,
  HelperStatus,
  ProfilesView,
  RecoveryReport,
  ServiceRecommendation,
  TelemetryView,
  WorkloadSnapshot,
} from "@/lib/optimizer"

export type SystemSnapshot = {
  battery: TelemetryView | null
  workload: WorkloadSnapshot | null
  profiles: ProfilesView | null
  hardware: HardwareInfo | null
  recommendations: ServiceRecommendation[] | null
  helper: HelperStatus | null
  recovery: RecoveryReport | null
}

const EMPTY: SystemSnapshot = {
  battery: null,
  workload: null,
  profiles: null,
  hardware: null,
  recommendations: null,
  helper: null,
  recovery: null,
}

const value = <T,>(r: PromiseSettledResult<T>): T | null => (r.status === "fulfilled" ? r.value : null)

/**
 * Facts for the System Overview. Each source loads independently, so one
 * failing (for example no battery on a desktop) never blanks the page. Battery
 * and workload refresh while the window is visible; everything else (services,
 * hardware, profiles) loads once and then follows events or a manual refresh.
 */
export function useSystemSnapshot() {
  const [snapshot, setSnapshot] = useState<SystemSnapshot>(EMPTY)
  const [loading, setLoading] = useState(true)

  const loadFast = useCallback(async () => {
    const [battery, workload] = await Promise.allSettled([api.optimizerBatteryTelemetry(), api.optimizerWorkload()])
    setSnapshot((s) => ({ ...s, battery: value(battery), workload: value(workload) }))
  }, [])

  const loadSlow = useCallback(async () => {
    const [profiles, hardware, recommendations, helper, recovery] = await Promise.allSettled([
      api.optimizerListProfiles(),
      api.optimizerHardwareInfo(false),
      api.optimizerServiceRecommendations(),
      api.optimizerHelperStatus(),
      api.optimizerRecoveryReport(),
    ])
    setSnapshot((s) => ({
      ...s,
      profiles: value(profiles),
      hardware: value(hardware),
      recommendations: value(recommendations),
      helper: value(helper),
      recovery: value(recovery),
    }))
  }, [])

  const reload = useCallback(async () => {
    setLoading(true)
    await Promise.all([loadFast(), loadSlow()])
    setLoading(false)
  }, [loadFast, loadSlow])

  useEffect(() => {
    void reload()
  }, [reload])
  usePolling(loadFast, 15_000)
  // Profiles change on user action or an automatic switch, both announced as events.
  useOptimizerEvents((event) => {
    if (event.kind === "workload") setSnapshot((s) => ({ ...s, workload: event.data }))
    if (event.kind === "profileChanged") {
      void api
        .optimizerListProfiles()
        .then((profiles) => setSnapshot((s) => ({ ...s, profiles })))
        .catch(() => {})
    }
  })

  return { snapshot, loading, reload }
}
