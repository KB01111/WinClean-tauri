import { useMemo } from "react"
import { ArrowRight, Sparkles } from "lucide-react"

import { StorageAnalysisPanel } from "@/components/ai/storage-analysis-panel"
import { SystemAnalysisPanel } from "@/components/ai/system-analysis-panel"
import { Panel } from "@/components/common"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { aiAvailability, describeEndpoint, loadEndpoint } from "@/lib/ai-config"
import type { ScanRunRecord } from "@/lib/api"
import type { RecoverMode } from "@/lib/recover-space"

/**
 * AI Insights: a storage analysis of a scan and a system analysis of services,
 * processes, battery and hardware. Both only propose; deterministic code and
 * the user's explicit approval decide what happens.
 */
export function AiInsightsView({
  scanRuns,
  activeRun,
  aiKeyConfigured,
  onApplySuggestion,
  onGoToSettings,
}: {
  scanRuns: ScanRunRecord[]
  activeRun: ScanRunRecord | null
  aiKeyConfigured: boolean
  onApplySuggestion: (runId: number, mode: RecoverMode, candidateIds: string[]) => void
  onGoToSettings: () => void
}) {
  const endpoint = useMemo(() => loadEndpoint(), [])
  const availability = aiAvailability(endpoint, aiKeyConfigured)

  if (!availability.ready) {
    return (
      <Panel title="AI Insights">
        <div className="flex flex-col items-center gap-3 py-12 text-center">
          <Sparkles className="size-8 text-primary" aria-hidden="true" />
          <p className="max-w-md text-sm text-muted-foreground">
            The assistant reads your scan results and system state and recommends what to clean, offload or tune. It can run on OpenAI or entirely on a local model.
          </p>
          <p className="max-w-md text-sm">{availability.reason}</p>
          <Button onClick={onGoToSettings}>
            Open AI settings
            <ArrowRight className="size-4" aria-hidden="true" />
          </Button>
        </div>
      </Panel>
    )
  }

  return (
    <div>
      <p className="mb-3 text-xs text-muted-foreground">Running on: {describeEndpoint(endpoint)}</p>
      <Tabs defaultValue="system">
        <TabsList>
          <TabsTrigger value="system">System</TabsTrigger>
          <TabsTrigger value="storage">Storage</TabsTrigger>
        </TabsList>
        <TabsContent value="system">
          <SystemAnalysisPanel endpoint={endpoint} />
        </TabsContent>
        <TabsContent value="storage">
          <StorageAnalysisPanel scanRuns={scanRuns} activeRun={activeRun} endpoint={endpoint} onApplySuggestion={onApplySuggestion} />
        </TabsContent>
      </Tabs>
    </div>
  )
}
