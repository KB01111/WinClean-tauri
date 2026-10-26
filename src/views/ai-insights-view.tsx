import { useMemo } from "react"
import { ArrowRight, Sparkles } from "lucide-react"

import { DuplicatesPanel } from "@/components/ai/duplicates-panel"
import { StorageAnalysisPanel } from "@/components/ai/storage-analysis-panel"
import { SystemAnalysisPanel } from "@/components/ai/system-analysis-panel"
import { Panel } from "@/components/common"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { aiAvailability, describeEndpoint, loadEndpoint } from "@/lib/ai-config"
import type { AiKeyMap, ScanRunRecord } from "@/lib/api"
import type { RecoverMode } from "@/lib/recover-space"

/**
 * Insights: storage analysis (works without AI too), duplicate files, and a
 * system analysis of services, processes, battery and hardware. The assistant only
 * proposes; deterministic code and the user's explicit approval decide what happens.
 */
export function AiInsightsView({
  scanRuns,
  activeRun,
  aiKeys,
  onApplySuggestion,
  onGoToSettings,
}: {
  scanRuns: ScanRunRecord[]
  activeRun: ScanRunRecord | null
  aiKeys: AiKeyMap
  onApplySuggestion: (runId: number, mode: RecoverMode, candidateIds: string[]) => void
  onGoToSettings: () => void
}) {
  const endpoint = useMemo(() => loadEndpoint(), [])
  const availability = aiAvailability(endpoint, aiKeys)
  const systemAvailability = aiAvailability(endpoint, aiKeys, "system")

  return (
    <div>
      <p className="mb-3 text-xs text-muted-foreground">
        {availability.ready ? `AI runs on: ${describeEndpoint(endpoint)}` : "AI is not set up yet — storage suggestions and duplicates work without it."}
      </p>
      <Tabs defaultValue="storage">
        <TabsList>
          <TabsTrigger value="storage">Storage</TabsTrigger>
          <TabsTrigger value="duplicates">Duplicates</TabsTrigger>
          <TabsTrigger value="system">System</TabsTrigger>
        </TabsList>
        <TabsContent value="storage">
          <StorageAnalysisPanel
            scanRuns={scanRuns}
            activeRun={activeRun}
            endpoint={endpoint}
            aiReady={availability.ready}
            aiUnavailableReason={availability.ready ? null : availability.reason}
            onApplySuggestion={onApplySuggestion}
            onGoToSettings={onGoToSettings}
          />
        </TabsContent>
        <TabsContent value="duplicates">
          <DuplicatesPanel scanRuns={scanRuns} activeRun={activeRun} />
        </TabsContent>
        <TabsContent value="system">
          {systemAvailability.ready ? (
            <SystemAnalysisPanel endpoint={endpoint} />
          ) : (
            <Panel title="System analysis">
              <div className="flex flex-col items-center gap-3 py-12 text-center">
                <Sparkles className="size-8 text-primary" aria-hidden="true" />
                <p className="max-w-md text-sm text-muted-foreground">The system analysis reads services, processes, battery and hardware state and proposes safe tuning. It needs an AI provider.</p>
                <p className="max-w-md text-sm">{systemAvailability.ready ? "" : systemAvailability.reason}</p>
                <Button onClick={onGoToSettings}>
                  Open AI settings
                  <ArrowRight className="size-4" aria-hidden="true" />
                </Button>
              </div>
            </Panel>
          )}
        </TabsContent>
      </Tabs>
    </div>
  )
}
