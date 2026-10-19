import { useCallback, useEffect, useState } from "react"

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
 * failing (for example no battery on a desktop) never blanks the page. The
 * cheap, changing facts refresh while the window is visible; the expensive
 * ones (service enumeration, hardware) load once and on demand.
 */
export function useSystemSnapshot() {
  const [snapshot, setSnapshot] = useState<SystemSnapshot>(EMPTY)
  const [loading, setLoading] = useState(true)

  const loadFast = useCallback(async () => {
    const [battery, workload, profiles] = await Promise.allSettled([
      api.optimizerBatteryTelemetry(),
      api.optimizerWorkload(),
      api.optimizerListProfiles(),
    ])
    setSnapshot((s) => ({ ...s, battery: value(battery), workload: value(workload), profiles: value(profiles) }))
  }, [])

  const loadSlow = useCallback(async () => {
    const [hardware, recommendations, helper, recovery] = await Promise.allSettled([
      api.optimizerHardwareInfo(false),
      api.optimizerServiceRecommendations(),
      api.optimizerHelperStatus(),
      api.optimizerRecoveryReport(),
    ])
    setSnapshot((s) => ({
      ...s,
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

  return { snapshot, loading, reload }
}
