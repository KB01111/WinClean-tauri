import { useCallback, useEffect, useMemo, useRef, useState } from "react"

import {
  api,
  formatError,
  type CategorySummary,
  type CleanupPlan,
  type DirectoryChild,
  type ExtensionSummary,
  type FileSearchResult,
  type RecommendationPlan,
  type ScanRunRecord,
  type TopDirectory,
} from "@/lib/api"
import {
  directoryBreadcrumbs,
  parentDirectoryPath,
  pruneCleanupSelection,
  shouldCommitRunDataResponse,
} from "@/lib/app-state"
import {
  getVisibleRecoverCandidates,
  selectedVisibleCandidateStats,
  type RecoverMode,
} from "@/lib/recover-space"
import { runDataRefreshKey } from "@/lib/scan-refresh"
import type { RecoverFilterState } from "@/views/recover-space-view"

/**
 * Owns the data of the selected scan run: the eight run-scoped collections,
 * their filters, the recover-space selection, and the debounced refresh that
 * keeps them in sync with the active run and current directory.
 */
export function useRunData({
  selectedRunId,
  scanRuns,
  isScanning,
  setMessage,
}: {
  selectedRunId: number | null
  scanRuns: ScanRunRecord[]
  isScanning: boolean
  setMessage: (message: string | null) => void
}) {
  const [topDirectories, setTopDirectories] = useState<TopDirectory[]>([])
  const [directoryChildren, setDirectoryChildren] = useState<DirectoryChild[]>(
    []
  )
  const [currentDirectory, setCurrentDirectory] = useState<string | null>(null)
  const [files, setFiles] = useState<FileSearchResult[]>([])
  const [staleFiles, setStaleFiles] = useState<FileSearchResult[]>([])
  const [fileQuery, setFileQuery] = useState("")
  const [extensionFilter, setExtensionFilter] = useState("")
  const [minSizeMbFilter, setMinSizeMbFilter] = useState("")
  const [extensions, setExtensions] = useState<ExtensionSummary[]>([])
  const [categories, setCategories] = useState<CategorySummary[]>([])
  const [cleanupPlan, setCleanupPlan] = useState<CleanupPlan | null>(null)
  const [recommendationPlan, setRecommendationPlan] =
    useState<RecommendationPlan | null>(null)
  const [recoverMode, setRecoverMode] = useState<RecoverMode>("clean")
  const [recoverFilters, setRecoverFilters] = useState<RecoverFilterState>({
    minSizeMb: "",
    minAgeDays: "",
  })
  const [selectedCleanupIds, setSelectedCleanupIds] = useState<Set<string>>(
    new Set()
  )
  const [loadingRunData, setLoadingRunData] = useState(false)
  const lastRunDataRefreshKey = useRef<string | null>(null)
  const runDataRequestId = useRef(0)
  const cleanupPlanRunId = useRef<number | null>(null)

  const activeRun = useMemo(
    () =>
      scanRuns.find((run) => run.id === selectedRunId) ??
      scanRuns.find((run) => run.status === "completed") ??
      null,
    [scanRuns, selectedRunId]
  )
  const executableCleanIds = useMemo(
    () =>
      new Set(cleanupPlan?.candidates.map((candidate) => candidate.id) ?? []),
    [cleanupPlan]
  )
  const visibleRecommendations = useMemo(() => {
    if (!recommendationPlan) {
      return []
    }
    return getVisibleRecoverCandidates(
      recommendationPlan.candidates,
      recoverMode,
      executableCleanIds
    )
  }, [executableCleanIds, recommendationPlan, recoverMode])
  const selectedVisibleStats = useMemo(
    () =>
      selectedVisibleCandidateStats(visibleRecommendations, selectedCleanupIds),
    [selectedCleanupIds, visibleRecommendations]
  )
  const explorerBreadcrumbs = useMemo(
    () => directoryBreadcrumbs(activeRun?.rootPath, currentDirectory),
    [activeRun?.rootPath, currentDirectory]
  )
  const explorerParentDirectory = useMemo(
    () => parentDirectoryPath(currentDirectory, activeRun?.rootPath),
    [activeRun?.rootPath, currentDirectory]
  )

  const refreshRunData = useCallback(
    async (
      runId?: number | null,
      parentPath?: string | null,
      options?: { clearMessage?: boolean }
    ) => {
      const requestId = ++runDataRequestId.current
      const resolvedRunId = runId ?? selectedRunId ?? null
      setLoadingRunData(true)
      if (!resolvedRunId) {
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
        setLoadingRunData(false)
        return true
      }
      const minSizeMb = Number(minSizeMbFilter)
      const minSize =
        Number.isFinite(minSizeMb) && minSizeMb > 0
          ? minSizeMb * 1024 * 1024
          : null
      const recoverMinSizeMb = Number(recoverFilters.minSizeMb)
      const recoverMinAgeDays = Number(recoverFilters.minAgeDays)
      const recommendationFilters = {
        minSize:
          Number.isFinite(recoverMinSizeMb) && recoverMinSizeMb > 0
            ? Math.round(recoverMinSizeMb * 1024 * 1024)
            : null,
        minAgeDays:
          Number.isFinite(recoverMinAgeDays) && recoverMinAgeDays > 0
            ? Math.round(recoverMinAgeDays)
            : null,
      }

      try {
        const [
          top,
          children,
          foundFiles,
          stale,
          extensionRows,
          categoryRows,
          plan,
          recommendations,
        ] = await Promise.all([
          api.topDirectories(resolvedRunId, 25),
          api.directoryChildren(resolvedRunId, parentPath ?? null, 250),
          api.fileSearch({
            runId: resolvedRunId,
            query: fileQuery || null,
            extension: extensionFilter || null,
            minSize,
            limit: 100,
          }),
          api.largestStaleFiles({ runId: resolvedRunId, limit: 25 }),
          api.extensionSummary(resolvedRunId, 20),
          api.categorySummary(resolvedRunId),
          api.cleanupPlan(resolvedRunId, 500),
          api.recommendations(resolvedRunId, recommendationFilters),
        ])
        if (
          !shouldCommitRunDataResponse({
            requestId,
            latestRequestId: runDataRequestId.current,
          })
        ) {
          return false
        }

        setTopDirectories(top)
        setDirectoryChildren(children)
        setFiles(foundFiles)
        setStaleFiles(stale)
        setExtensions(extensionRows)
        setCategories(categoryRows)
        const previousPlanRunId = cleanupPlanRunId.current
        const nextPlanRunId =
          recommendations?.scanRunId ?? plan?.scanRunId ?? null
        cleanupPlanRunId.current = nextPlanRunId
        setCleanupPlan(plan)
        setRecommendationPlan(recommendations)
        setSelectedCleanupIds((current) =>
          previousPlanRunId === nextPlanRunId
            ? pruneCleanupSelection(
                current,
                (recommendations?.candidates ?? plan?.candidates ?? []).map(
                  (candidate) => candidate.id
                )
              )
            : new Set()
        )
        if (options?.clearMessage !== false) {
          setMessage(null)
        }
        return true
      } catch (error) {
        if (
          shouldCommitRunDataResponse({
            requestId,
            latestRequestId: runDataRequestId.current,
          })
        ) {
          setMessage(formatError(error))
        }
        return false
      } finally {
        if (
          shouldCommitRunDataResponse({
            requestId,
            latestRequestId: runDataRequestId.current,
          })
        ) {
          setLoadingRunData(false)
        }
      }
    },
    [
      extensionFilter,
      fileQuery,
      minSizeMbFilter,
      recoverFilters.minAgeDays,
      recoverFilters.minSizeMb,
      selectedRunId,
      setMessage,
    ]
  )

  useEffect(() => {
    if (!activeRun || isScanning) {
      return
    }
    const key = runDataRefreshKey(activeRun.id, currentDirectory)
    if (lastRunDataRefreshKey.current === key) {
      lastRunDataRefreshKey.current = null
      return
    }
    const timeout = window.setTimeout(() => {
      void refreshRunData(activeRun.id, currentDirectory ?? activeRun.rootPath)
    }, 200)
    return () => window.clearTimeout(timeout)
  }, [
    activeRun,
    currentDirectory,
    extensionFilter,
    fileQuery,
    isScanning,
    refreshRunData,
  ])

  function clearRunData() {
    setTopDirectories([])
    setDirectoryChildren([])
    setFiles([])
    setStaleFiles([])
    setCleanupPlan(null)
    setRecommendationPlan(null)
    setSelectedCleanupIds(new Set())
  }

  function toggleCleanupCandidate(id: string) {
    setSelectedCleanupIds((current) => {
      const next = new Set(current)
      if (next.has(id)) {
        next.delete(id)
      } else {
        next.add(id)
      }
      return next
    })
  }

  function toggleAllVisible(select: boolean) {
    setSelectedCleanupIds((current) => {
      const next = new Set(current)
      for (const candidate of visibleRecommendations) {
        if (select) {
          next.add(candidate.id)
        } else {
          next.delete(candidate.id)
        }
      }
      return next
    })
  }

  return {
    topDirectories,
    directoryChildren,
    files,
    staleFiles,
    extensions,
    categories,
    cleanupPlan,
    recommendationPlan,
    currentDirectory,
    setCurrentDirectory,
    fileQuery,
    setFileQuery,
    extensionFilter,
    setExtensionFilter,
    minSizeMbFilter,
    setMinSizeMbFilter,
    recoverMode,
    setRecoverMode,
    recoverFilters,
    setRecoverFilters,
    selectedCleanupIds,
    setSelectedCleanupIds,
    loadingRunData,
    activeRun,
    visibleRecommendations,
    selectedVisibleStats,
    explorerBreadcrumbs,
    explorerParentDirectory,
    lastRunDataRefreshKey,
    refreshRunData,
    clearRunData,
    toggleCleanupCandidate,
    toggleAllVisible,
  }
}
