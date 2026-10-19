import { useCallback, useRef, useState } from "react"
import { toast } from "sonner"

import { api } from "@/lib/api"
import {
  describeError,
  isConflict,
  isElevationRequired,
  type PreparedPlanView,
  type ServiceApplyOutcome,
  type ServiceChange,
} from "@/lib/optimizer"

/**
 * The prepare → review → approve → apply flow for service changes.
 *
 * `prepare` asks the backend to validate a requested change set; it returns
 * what is actually allowed plus a single-use token. Only `apply(token)` changes
 * the system, and only for exactly the reviewed plan. If the system changed
 * between review and approval the backend answers "conflict" and the plan is
 * re-prepared so the user reviews the new state instead of an outdated one.
 */
export function useServicePlan(onApplied: (outcome: ServiceApplyOutcome) => void) {
  const [prepared, setPrepared] = useState<PreparedPlanView | null>(null)
  const [preparing, setPreparing] = useState(false)
  const [applying, setApplying] = useState(false)
  const lastRequest = useRef<{ changes: ServiceChange[]; origin: "user" | "ai" } | null>(null)
  const [origin, setOrigin] = useState<"user" | "ai">("user")

  const prepare = useCallback(async (changes: ServiceChange[], from: "user" | "ai" = "user") => {
    if (changes.length === 0) return
    lastRequest.current = { changes, origin: from }
    setOrigin(from)
    setPreparing(true)
    try {
      setPrepared(await api.optimizerPrepareServicePlan(changes, from))
    } catch (error) {
      toast.error(describeError(error))
    } finally {
      setPreparing(false)
    }
  }, [])

  const apply = useCallback(
    async (token: string) => {
      setApplying(true)
      try {
        const outcome = await api.optimizerApplyServicePlan(token)
        setPrepared(null)
        lastRequest.current = null
        toast.success(outcome.summary)
        onApplied(outcome)
      } catch (error) {
        if (isElevationRequired(error)) {
          // The plan stays open: enabling the helper and pressing Apply again uses the same token.
          toast.error("Administrator rights are needed. Enable the administrator helper, then apply again.")
        } else if (isConflict(error) && lastRequest.current) {
          toast.warning("Services changed since you reviewed this plan. Review the updated plan.")
          const { changes, origin: from } = lastRequest.current
          try {
            setPrepared(await api.optimizerPrepareServicePlan(changes, from))
          } catch (again) {
            setPrepared(null)
            toast.error(describeError(again))
          }
        } else {
          setPrepared(null)
          toast.error(describeError(error))
        }
      } finally {
        setApplying(false)
      }
    },
    [onApplied]
  )

  const close = useCallback(() => {
    setPrepared(null)
    lastRequest.current = null
  }, [])

  return { prepared, preparing, applying, origin, prepare, apply, close }
}
