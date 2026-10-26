import { useCallback, useEffect, useMemo, useState } from "react"
import { Loader2, RefreshCw, ShieldCheck } from "lucide-react"

import { EmptyState, Metric, Notice, Panel, TextInput } from "@/components/common"
import { ElevationBanner } from "@/components/optimizer/elevation-banner"
import { PlanReviewDialog } from "@/components/optimizer/plan-review-dialog"
import { RecommendationList } from "@/components/optimizer/recommendation-list"
import { ServiceTable } from "@/components/optimizer/service-table"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { useHelper } from "@/hooks/use-helper"
import { useServicePlan } from "@/hooks/use-service-plan"
import { api, formatError } from "@/lib/api"
import {
  filterServices,
  recommendationMemoryMb,
  summarizeServices,
  type ServiceInfo,
  type ServiceRecommendation,
  type ServiceRisk,
  type StartType,
} from "@/lib/optimizer"

/**
 * Windows Services: native SCM data, deterministic recommendations and guarded
 * changes. Every change — single or batch — goes through the same
 * prepare → review → approve → apply path, and protected services can never
 * be selected for change.
 */
export function ServicesView() {
  const helper = useHelper()
  const [services, setServices] = useState<ServiceInfo[]>([])
  const [recommendations, setRecommendations] = useState<ServiceRecommendation[]>([])
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState("")
  const [risk, setRisk] = useState<ServiceRisk | "all">("all")
  const [onlyRunning, setOnlyRunning] = useState(false)
  const [onlyChangeable, setOnlyChangeable] = useState(false)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [list, recs] = await Promise.all([api.optimizerListServices(), api.optimizerServiceRecommendations()])
      setServices(list)
      setRecommendations(recs)
      setSelected(new Set(recs.map((r) => r.serviceName)))
      setError(null)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const plan = useServicePlan(() => {
    void load()
  })

  const visible = useMemo(
    () => filterServices(services, { query, risk, onlyRunning, onlyChangeable }),
    [services, query, risk, onlyRunning, onlyChangeable]
  )
  const summary = useMemo(() => summarizeServices(services), [services])
  const chosen = recommendations.filter((r) => selected.has(r.serviceName))

  function toggle(name: string) {
    setSelected((cur) => {
      const next = new Set(cur)
      if (!next.delete(name)) next.add(name)
      return next
    })
  }

  function requestSingle(svc: ServiceInfo, startType: StartType) {
    void plan.prepare([{ serviceName: svc.name, startType }])
  }

  return (
    <div className="space-y-4">
      <ElevationBanner helper={helper} />

      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-5">
        <Metric label="Services" value={String(summary.total)} hint={`${summary.running} running`} />
        <Metric label="Start automatically" value={String(summary.autoStart)} />
        <Metric label="Protected" value={String(summary.protectedCount)} hint="never changed" />
        <Metric label="Recommendations" value={String(recommendations.length)} />
        <Metric
          label="Memory in recommendations"
          value={`${recommendationMemoryMb(recommendations)} MB`}
          hint="running services only"
        />
      </div>

      <Panel
        title="Windows services (Service Control Manager)"
        action={
          <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Refresh
          </Button>
        }
      >
        <p className="mb-3 flex items-start gap-2 text-sm text-muted-foreground">
          <ShieldCheck className="mt-0.5 size-4 shrink-0 text-primary" aria-hidden="true" />
          Defender, Windows Update, Windows Search, drivers, networking, audio, ASUS/AMD/NVIDIA components and Copilot+ services can never be changed — not by you, not by the AI assistant. Changing a start type never stops a running service; it takes effect at the next start.
        </p>
        {error ? <Notice tone="danger" text={error} /> : null}

        <Tabs defaultValue="recommended">
          <TabsList>
            <TabsTrigger value="recommended">Recommended ({recommendations.length})</TabsTrigger>
            <TabsTrigger value="all">All services ({services.length})</TabsTrigger>
          </TabsList>

          <TabsContent value="recommended">
            {loading ? (
              <EmptyState text="Reading services from the Service Control Manager…" />
            ) : (
              <>
                <RecommendationList recommendations={recommendations} selected={selected} onToggle={toggle} />
                {recommendations.length > 0 ? (
                  <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
                    <div className="flex gap-2">
                      <Button variant="outline" size="sm" onClick={() => setSelected(new Set(recommendations.map((r) => r.serviceName)))}>
                        Select all
                      </Button>
                      <Button variant="outline" size="sm" onClick={() => setSelected(new Set())}>
                        Select none
                      </Button>
                    </div>
                    <Button
                      disabled={chosen.length === 0 || plan.preparing}
                      onClick={() =>
                        void plan.prepare(
                          chosen.map((r) => ({ serviceName: r.serviceName, startType: r.suggestedStartType as StartType }))
                        )
                      }
                    >
                      {plan.preparing ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                      Review {chosen.length} change{chosen.length === 1 ? "" : "s"}
                    </Button>
                  </div>
                ) : null}
              </>
            )}
          </TabsContent>

          <TabsContent value="all">
            <div className="mb-3 flex flex-wrap items-center gap-3">
              <TextInput
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Filter services…"
                aria-label="Filter services"
                className="max-w-xs"
              />
              <select
                aria-label="Filter by risk"
                value={risk}
                onChange={(e) => setRisk(e.target.value as ServiceRisk | "all")}
                className="h-9 rounded-md border border-input bg-card px-2 text-sm"
              >
                <option value="all">Any risk</option>
                <option value="safe">Safe</option>
                <option value="caution">Caution</option>
                <option value="protected">Protected</option>
              </select>
              <label className="flex items-center gap-1.5 text-sm">
                <input type="checkbox" className="accent-[var(--primary)]" checked={onlyRunning} onChange={(e) => setOnlyRunning(e.target.checked)} />
                Running only
              </label>
              <label className="flex items-center gap-1.5 text-sm">
                <input type="checkbox" className="accent-[var(--primary)]" checked={onlyChangeable} onChange={(e) => setOnlyChangeable(e.target.checked)} />
                Changeable only
              </label>
            </div>
            {loading ? (
              <EmptyState text="Reading services from the Service Control Manager…" />
            ) : (
              <ServiceTable services={visible} busy={plan.preparing || plan.applying} onRequestChange={requestSingle} />
            )}
          </TabsContent>
        </Tabs>
      </Panel>

      <PlanReviewDialog
        prepared={plan.prepared}
        origin={plan.origin}
        applying={plan.applying}
        helper={helper}
        onApply={(token) => {
          void plan.apply(token).then(() => helper.refresh())
        }}
        onClose={plan.close}
      />
    </div>
  )
}
