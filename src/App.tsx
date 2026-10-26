import { lazy, Suspense, useState } from "react"

import { EmptyState, Notice } from "@/components/common"
import { PageHeader } from "@/components/layout/page-header"
import { Sidebar } from "@/components/layout/sidebar"
import { useNativeWindow } from "@/hooks/use-native-window"
import { useStorage } from "@/hooks/use-storage"
import { DEFAULT_VIEW, type View } from "@/lib/views"
import { ExplorerView } from "@/views/explorer-view"
import { HardwareView } from "@/views/hardware-view"
import { HistoryView } from "@/views/history-view"
import { McpView } from "@/views/mcp-view"
import { OptimizerHistoryView } from "@/views/optimizer-history-view"
import { OverviewView } from "@/views/overview-view"
import { PowerView } from "@/views/power-view"
import { ProcessesView } from "@/views/processes-view"
import { ProfilesView } from "@/views/profiles-view"
import { RecoverSpaceView } from "@/views/recover-space-view"
import { ServicesView } from "@/views/services-view"
import { SettingsView } from "@/views/settings-view"
import { SystemOverviewView } from "@/views/system-overview-view"

// The AI view pulls in the agents SDK; load it only when it is opened so
// startup parses (and the idle process holds) far less JavaScript.
const AiInsightsView = lazy(() => import("@/views/ai-insights-view").then((m) => ({ default: m.AiInsightsView })))

/** App shell: navigation, page header and the active view. State lives in hooks. */
function App() {
  useNativeWindow()
  const [view, setView] = useState<View>(DEFAULT_VIEW)
  const storage = useStorage({ onNavigate: setView })
  const { runData, progress } = storage

  return (
    <main className="min-h-svh bg-background text-foreground">
      <div className="grid min-h-svh grid-cols-[260px_1fr]">
        <Sidebar
          view={view}
          onViewChange={setView}
          volumes={storage.volumes}
          selectedRoot={storage.selectedRoot}
          onSelectRoot={storage.setSelectedRoot}
          loadingVolumes={storage.loadingVolumes}
        />

        <section className="min-w-0 p-5">
          <PageHeader view={view} storage={storage} />

          {(progress.fallbackReason || storage.message || progress.lastError) && (
            <Notice tone={progress.lastError ? "danger" : "warning"} text={[progress.fallbackReason, storage.message, progress.lastError].filter(Boolean).join(" ")} />
          )}

          {view === "system" && <SystemOverviewView volumes={storage.volumes} recoverableBytes={runData.recommendationPlan?.cleanBytes ?? null} onNavigate={setView} />}
          {view === "overview" && (
            <OverviewView
              progress={progress}
              scanRuns={storage.scanRuns}
              activeRun={storage.activeRun}
              topDirectories={runData.topDirectories}
              topFiles={runData.files}
              staleFiles={runData.staleFiles}
              categories={runData.categories}
              extensions={runData.extensions}
              recommendationPlan={runData.recommendationPlan}
              volumes={storage.volumes}
              onNavigate={setView}
              onSelectRun={storage.selectRun}
              onOpenDirectory={storage.openDirectory}
              onExportReport={storage.exportReport}
              reportPending={storage.reportPending}
            />
          )}
          {view === "explorer" && (
            <ExplorerView
              activeRun={storage.activeRun}
              currentDirectory={storage.currentDirectory}
              breadcrumbs={storage.explorerBreadcrumbs}
              parentDirectory={storage.explorerParentDirectory}
              directoryChildren={runData.directoryChildren}
              files={runData.files}
              fileQuery={runData.fileQuery}
              extensionFilter={runData.extensionFilter}
              loadingRunData={runData.loadingRunData}
              minSizeMbFilter={runData.minSizeMbFilter}
              onFileQueryChange={runData.setFileQuery}
              onExtensionFilterChange={runData.setExtensionFilter}
              onMinSizeMbFilterChange={runData.setMinSizeMbFilter}
              onOpenDirectory={storage.openDirectory}
            />
          )}
          {view === "cleanup" && (
            <RecoverSpaceView
              recommendationPlan={runData.recommendationPlan}
              cleanupWarnings={runData.cleanupPlan?.warnings ?? []}
              visibleRecommendations={runData.visibleRecommendations}
              recoverMode={runData.recoverMode}
              onRecoverModeChange={runData.changeRecoverMode}
              filters={runData.recoverFilters}
              onFiltersChange={runData.setRecoverFilters}
              selectedCleanupIds={runData.selectedCleanupIds}
              selectedRecommendationBytes={runData.selectedVisibleStats.bytes}
              cleanupPending={storage.cleanupPending}
              isScanning={storage.isScanning}
              driveStatus={storage.driveStatus}
              activeOffloadJob={storage.offload.activeJob}
              offloadRunning={storage.offload.running}
              onToggleCandidate={runData.toggleCandidate}
              onToggleAllVisible={runData.toggleAllVisible}
              onClean={storage.executeSelectedCleanup}
              onOffload={storage.startSelectedOffload}
              onConnectDrive={storage.connectGoogleDrive}
              onDisconnectDrive={storage.disconnectGoogleDrive}
              onCancelOffload={storage.offload.cancel}
            />
          )}
          {view === "services" && <ServicesView />}
          {view === "processes" && <ProcessesView />}
          {view === "power" && <PowerView />}
          {view === "hardware" && <HardwareView />}
          {view === "profiles" && <ProfilesView />}
          {view === "insights" && (
            <Suspense fallback={<EmptyState text="Loading the assistant…" />}>
              <AiInsightsView
                scanRuns={storage.scanRuns}
                activeRun={storage.activeRun}
                aiKeys={storage.aiKeys}
                onApplySuggestion={storage.applyAiSuggestion}
                onGoToSettings={() => setView("settings")}
              />
            </Suspense>
          )}
          {view === "mcp" && <McpView />}
          {view === "optimizerHistory" && <OptimizerHistoryView />}
          {view === "history" && (
            <HistoryView
              cleanupHistory={storage.cleanupHistory}
              offloadJobs={storage.offloadJobs}
              scanRuns={storage.scanRuns}
              historyPending={storage.historyPending}
              restorePendingItemId={storage.restorePendingItemId}
              onOpenRecycleBin={storage.openRecycleBin}
              onDeleteRun={storage.deleteRun}
              onClearScanHistory={storage.clearScanHistory}
              onRestoreItem={storage.restoreOffloadItem}
            />
          )}
          {view === "settings" && (
            <SettingsView
              health={storage.health}
              driveStatus={storage.driveStatus}
              onConnectDrive={storage.connectGoogleDrive}
              onDisconnectDrive={storage.disconnectGoogleDrive}
              onExportDiagnostics={storage.exportDiagnostics}
              diagnosticsPending={storage.diagnosticsPending}
              aiKeys={storage.aiKeys}
              onAiKeyChanged={storage.setAiKey}
            />
          )}
        </section>
      </div>
    </main>
  )
}

export default App
