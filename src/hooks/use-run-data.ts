import { useCallback, useMemo, useRef, useState } from "react"

import {
  api,
  formatError,
  type CategorySummary,
  type CleanupPlan,
  type DirectoryChild,
  type ExtensionSummary,
  type FileSearchResult,
  type RecommendationPlan,
  type TopDirectory,
} from "@/lib/api"
import { pruneCleanupSelection, shouldCommitRunDataResponse } from "@/lib/app-state"
import {
  getVisibleRecoverCandidates,
  selectedVisibleCandidateStats,
  type RecoverMode,
} from "@/lib/recover-space"
import type { RecoverFilterState } from "@/views/recover-space-view"

/**
 * Everything derived from one scan run: directory sizes, search results, the
 * cleanup plan and recommendations, the user's selection and the filters that
 * shape them. Responses are guarded by a request id so a slow, stale response
 * can never overwrite a newer one.
 */
export function useRunData({
  selectedRunId,
  setMessage,
}: {
  selectedRunId: number | null
  setMessage: (message: string | null) => void
}) {
  const [topDirectories, setTopDirectories] = useState<TopDirectory[]>([])
  const [directoryChildren, setDirectoryChildren] = useState<DirectoryChild[]>([])
  const [files, setFiles] = useState<FileSearchResult[]>([])
  const [staleFiles, setStaleFiles] = useState<FileSearchResult[]>([])
  const [fileQuery, setFileQuery] = useState("")
  const [extensionFilter, setExtensionFilter] = useState("")
  const [minSizeMbFilter, setMinSizeMbFilter] = useState("")
  const [extensions, setExtensions] = useState<ExtensionSummary[]>([])
  const [categories, setCategories] = useState<CategorySummary[]>([])
  const [cleanupPlan, setCleanupPlan] = useState<CleanupPlan | null>(null)
  const [recommendationPlan, setRecommendationPlan] = useState<RecommendationPlan | null>(null)
  const [recoverMode, setRecoverMode] = useState<RecoverMode>("clean")
  const [recoverFilters, setRecoverFilters] = useState<RecoverFilterState>({ minSizeMb: "", minAgeDays: "" })
  const [selectedCleanupIds, setSelectedCleanupIds] = useState<Set<string>>(new Set())
  const [loadingRunData, setLoadingRunData] = useState(false)
  const runDataRequestId = useRef(0)
  const cleanupPlanRunId = useRef<number | null>(null)

  const executableCleanIds = useMemo(
    () => new Set(cleanupPlan?.candidates.map((candidate) => candidate.id) ?? []),
    [cleanupPlan]
  )
  const visibleRecommendations = useMemo(() => {
    if (!recommendationPlan) return []
    return getVisibleRecoverCandidates(recommendationPlan.candidates, recoverMode, executableCleanIds)
  }, [executableCleanIds, recommendationPlan, recoverMode])
  const selectedVisibleStats = useMemo(
    () => selectedVisibleCandidateStats(visibleRecommendations, selectedCleanupIds),
    [selectedCleanupIds, visibleRecommendations]
  )

  /** Drop all run-derived data (new scan starting, or no run selected). */
  const clear = useCallback(() => {
    setTopDirectories([])
    setDirectoryChildren([])
    setFiles([])
    setStaleFiles([])
    setExtensions([])
    setCategories([])
    setCleanupPlan(null)
    setRecommendationPlan(null)
    cleanupPlanRunId.current = null
    setSelectedCleanupIds(new Set())
  }, [])

  const refreshRunData = useCallback(
    async (runId?: number | null, parentPath?: string | null, options?: { clearMessage?: boolean }) => {
      const requestId = ++runDataRequestId.current
      const resolvedRunId = runId ?? selectedRunId ?? null
      const isLatest = () => shouldCommitRunDataResponse({ requestId, latestRequestId: runDataRequestId.current })
      setLoadingRunData(true)
      if (!resolvedRunId) {
        clear()
        setLoadingRunData(false)
        return true
      }
      const minSizeMb = Number(minSizeMbFilter)
      const minSize = Number.isFinite(minSizeMb) && minSizeMb > 0 ? minSizeMb * 1024 * 1024 : null
      const recoverMinSizeMb = Number(recoverFilters.minSizeMb)
      const recoverMinAgeDays = Number(recoverFilters.minAgeDays)
      const recommendationFilters = {
        minSize: Number.isFinite(recoverMinSizeMb) && recoverMinSizeMb > 0 ? Math.round(recoverMinSizeMb * 1024 * 1024) : null,
        minAgeDays: Number.isFinite(recoverMinAgeDays) && recoverMinAgeDays > 0 ? Math.round(recoverMinAgeDays) : null,
      }

      try {
        const [top, children, foundFiles, stale, extensionRows, categoryRows, plan, recommendations] = await Promise.all([
          api.topDirectories(resolvedRunId, 25),
          api.directoryChildren(resolvedRunId, parentPath ?? null, 250),
          api.fileSearch({ runId: resolvedRunId, query: fileQuery || null, extension: extensionFilter || null, minSize, limit: 100 }),
          api.largestStaleFiles({ runId: resolvedRunId, limit: 25 }),
          api.extensionSummary(resolvedRunId, 20),
          api.categorySummary(resolvedRunId),
          api.cleanupPlan(resolvedRunId, 500),
          api.recommendations(resolvedRunId, recommendationFilters),
        ])
        if (!isLatest()) return false

        setTopDirectories(top)
        setDirectoryChildren(children)
        setFiles(foundFiles)
        setStaleFiles(stale)
        setExtensions(extensionRows)
        setCategories(categoryRows)
        const previousPlanRunId = cleanupPlanRunId.current
        const nextPlanRunId = recommendations?.scanRunId ?? plan?.scanRunId ?? null
        cleanupPlanRunId.current = nextPlanRunId
        setCleanupPlan(plan)
        setRecommendationPlan(recommendations)
        setSelectedCleanupIds((current) =>
          previousPlanRunId === nextPlanRunId
            ? pruneCleanupSelection(current, (recommendations?.candidates ?? plan?.candidates ?? []).map((candidate) => candidate.id))
            : new Set()
        )
        if (options?.clearMessage !== false) setMessage(null)
        return true
      } catch (error) {
        if (isLatest()) setMessage(formatError(error))
        return false
      } finally {
        if (isLatest()) setLoadingRunData(false)
      }
    },
    [clear, extensionFilter, fileQuery, minSizeMbFilter, recoverFilters.minAgeDays, recoverFilters.minSizeMb, selectedRunId, setMessage]
  )

  const toggleCandidate = useCallback((id: string) => {
    setSelectedCleanupIds((current) => {
      const next = new Set(current)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }, [])

  const toggleAllVisible = useCallback(
    (select: boolean) => {
      setSelectedCleanupIds((current) => {
        const next = new Set(current)
        for (const candidate of visibleRecommendations) {
          if (select) next.add(candidate.id)
          else next.delete(candidate.id)
        }
        return next
      })
    },
    [visibleRecommendations]
  )

  /** Switching mode starts a fresh selection: ids from another mode do not apply. */
  const changeRecoverMode = useCallback((mode: RecoverMode) => {
    setRecoverMode(mode)
    setSelectedCleanupIds(new Set())
  }, [])

  return {
    topDirectories,
    directoryChildren,
    files,
    staleFiles,
    extensions,
    categories,
    cleanupPlan,
    recommendationPlan,
    fileQuery,
    setFileQuery,
    extensionFilter,
    setExtensionFilter,
    minSizeMbFilter,
    setMinSizeMbFilter,
    recoverMode,
    setRecoverMode,
    changeRecoverMode,
    recoverFilters,
    setRecoverFilters,
    selectedCleanupIds,
    setSelectedCleanupIds,
    visibleRecommendations,
    selectedVisibleStats,
    loadingRunData,
    refreshRunData,
    clear,
    toggleCandidate,
    toggleAllVisible,
  }
}
