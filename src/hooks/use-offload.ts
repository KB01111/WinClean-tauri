import { useCallback, useEffect, useRef, useState } from "react"
import { toast } from "sonner"

import { api, formatError, type OffloadJobStatus } from "@/lib/api"

/**
 * Owns the lifecycle of the active Google Drive offload job: starting,
 * cancelling, polling while running, and rehydrating after an app restart.
 */
export function useOffload({ onJobSettled }: { onJobSettled: () => void }) {
  const [activeJob, setActiveJob] = useState<OffloadJobStatus | null>(null)
  const onJobSettledRef = useRef(onJobSettled)
  useEffect(() => {
    onJobSettledRef.current = onJobSettled
  }, [onJobSettled])

  const running =
    activeJob?.status === "queued" || activeJob?.status === "running"

  useEffect(() => {
    let disposed = false
    api
      .activeOffloadJob()
      .then((job) => {
        if (!disposed && job) {
          setActiveJob(job)
        }
      })
      .catch(() => {
        // Rehydration is best effort; the user can still start new jobs.
      })
    return () => {
      disposed = true
    }
  }, [])

  useEffect(() => {
    if (!activeJob || !running) {
      return
    }
    const jobId = activeJob.id
    const timer = window.setInterval(async () => {
      try {
        const nextJob = await api.offloadStatus(jobId)
        setActiveJob(nextJob)
        if (nextJob.status !== "queued" && nextJob.status !== "running") {
          if (nextJob.status === "completed") {
            toast.success(
              `Offload batch ${nextJob.id} completed (${nextJob.completedItems} item(s)).`
            )
          } else if (nextJob.status === "completed_with_errors") {
            toast.warning(
              `Offload batch ${nextJob.id} finished with ${nextJob.failedItems} item(s) needing attention.`
            )
          } else if (nextJob.status === "canceled") {
            toast.info(`Offload batch ${nextJob.id} was canceled.`)
          } else if (nextJob.status === "failed") {
            toast.error(
              nextJob.lastError ?? `Offload batch ${nextJob.id} failed.`
            )
          }
          onJobSettledRef.current()
        }
      } catch (error) {
        toast.error(formatError(error))
      }
    }, 1200)
    return () => window.clearInterval(timer)
  }, [activeJob, running])

  const start = useCallback(
    async (runId: number, candidateIds: string[]) => {
      if (candidateIds.length === 0) {
        return
      }
      try {
        const job = await api.startOffload(runId, candidateIds)
        setActiveJob(job)
        toast.success(`Offload batch ${job.id} queued for Google Drive.`)
      } catch (error) {
        toast.error(formatError(error))
      }
    },
    []
  )

  const cancel = useCallback(async () => {
    if (!activeJob) {
      return
    }
    try {
      const job = await api.cancelOffload(activeJob.id)
      setActiveJob(job)
      toast.info("Offload cancel requested.")
    } catch (error) {
      toast.error(formatError(error))
    }
  }, [activeJob])

  return { activeJob, running, start, cancel }
}
