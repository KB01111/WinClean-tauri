import { Loader2, Play, RefreshCw, Square } from "lucide-react"

import { Button } from "@/components/ui/button"
import type { StorageController } from "@/hooks/use-storage"
import { formatNumber, statusText } from "@/lib/format"
import { VIEW_META, isStorageView, type View } from "@/lib/views"

/**
 * Page heading. Storage views also get the scan status line and the
 * Refresh / Scan / Cancel controls; optimizer views just show their purpose.
 */
export function PageHeader({ view, storage }: { view: View; storage: StorageController }) {
  const meta = VIEW_META[view]
  const showScan = isStorageView(view)
  const { progress, isScanning } = storage

  return (
    <header className="mb-5 flex flex-wrap items-start justify-between gap-3 border-b border-border pb-4">
      <div>
        <h2 className="text-2xl font-semibold tracking-tight">{meta.title}</h2>
        <p className="text-sm text-muted-foreground">
          {showScan
            ? `${statusText(progress)}${progress.workerCount && progress.phase ? ` · ${progress.phase} on ${formatNumber(progress.workerCount)} workers` : ""}`
            : meta.description}
        </p>
      </div>
      {showScan ? (
        <div className="flex flex-wrap items-center gap-2">
          <Button variant="outline" onClick={() => void storage.refreshDashboard()} disabled={storage.loadingData || storage.runData.loadingRunData}>
            <RefreshCw className="size-4" aria-hidden="true" />
            Refresh
          </Button>
          {isScanning ? (
            <Button variant="destructive" onClick={storage.cancelScan} disabled={storage.cancelPending}>
              {storage.cancelPending ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Square className="size-4" aria-hidden="true" />}
              {storage.cancelPending ? "Canceling" : "Cancel"}
            </Button>
          ) : (
            <Button onClick={storage.startScan} disabled={!storage.selectedVolume || !storage.selectedVolume.isReady || storage.scanPending}>
              {storage.scanPending ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Play className="size-4" aria-hidden="true" />}
              {storage.scanPending ? "Starting" : "Scan"}
            </Button>
          )}
        </div>
      ) : null}
    </header>
  )
}
