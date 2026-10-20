import {
  BarChart3,
  BatteryCharging,
  Cpu,
  FolderTree,
  Gauge,
  HardDrive,
  History,
  Leaf,
  Loader2,
  Plug,
  Server,
  Settings,
  SlidersHorizontal,
  Sparkles,
  Trash2,
  Undo2,
} from "lucide-react"

import { CapacityBar, NavButton } from "@/components/common"
import { formatBytes, formatPercentUsed } from "@/lib/format"
import type { VolumeInfo } from "@/lib/api"
import { NAV_GROUPS, VIEW_META, isStorageView, type View } from "@/lib/views"
import { cn } from "@/lib/utils"

export type { View } from "@/lib/views"

const ICONS: Record<View, typeof BarChart3> = {
  system: Gauge,
  overview: BarChart3,
  explorer: FolderTree,
  cleanup: Trash2,
  services: Server,
  processes: Leaf,
  power: BatteryCharging,
  hardware: Cpu,
  profiles: SlidersHorizontal,
  insights: Sparkles,
  mcp: Plug,
  optimizerHistory: Undo2,
  history: History,
  settings: Settings,
}

/** Render grouped navigation and, on storage views, the scan target picker. */
export function Sidebar({
  view,
  onViewChange,
  volumes,
  selectedRoot,
  onSelectRoot,
  loadingVolumes,
}: {
  view: View
  onViewChange: (view: View) => void
  volumes: VolumeInfo[]
  selectedRoot: string
  onSelectRoot: (rootPath: string) => void
  loadingVolumes: boolean
}) {
  return (
    <aside className="sticky top-0 flex h-svh flex-col gap-5 overflow-y-auto border-r border-sidebar-border bg-sidebar p-4 text-sidebar-foreground">
      <div className="flex items-center gap-3">
        <div className="flex size-10 items-center justify-center rounded-lg bg-primary text-primary-foreground shadow-sm">
          <HardDrive className="size-5" aria-hidden="true" />
        </div>
        <div>
          <h1 className="text-xl font-semibold tracking-tight">WinClean</h1>
          <p className="text-xs text-muted-foreground">Windows optimization suite</p>
        </div>
      </div>

      <nav aria-label="Main" className="space-y-4">
        {NAV_GROUPS.map((group) => (
          <div key={group.id} role="group" aria-labelledby={`nav-${group.id}`}>
            <p id={`nav-${group.id}`} className="mb-1 px-3 text-[0.68rem] font-semibold tracking-wider text-muted-foreground uppercase">
              {group.label}
            </p>
            <div className="space-y-0.5">
              {group.views.map((id) => (
                <NavButton key={id} icon={ICONS[id]} label={VIEW_META[id].label} active={view === id} onClick={() => onViewChange(id)} />
              ))}
            </div>
          </div>
        ))}
      </nav>

      {isStorageView(view) ? (
        <div className="rounded-xl border border-sidebar-border bg-card p-3 shadow-sm">
          <div className="mb-2 flex items-center justify-between">
            <span className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">Scan target</span>
            {loadingVolumes ? <Loader2 className="size-4 animate-spin text-primary" /> : null}
          </div>
          <div className="space-y-2">
            {volumes.length === 0 && !loadingVolumes ? <p className="py-2 text-center text-xs text-muted-foreground">No volumes detected</p> : null}
            {volumes.map((volume) => {
              const hasCapacity = volume.totalSpace != null && volume.availableSpace != null
              return (
                <button
                  type="button"
                  key={volume.rootPath}
                  onClick={() => onSelectRoot(volume.rootPath)}
                  aria-label={`Scan drive ${volume.rootPath}, ${formatPercentUsed(volume)}`}
                  aria-pressed={selectedRoot === volume.rootPath}
                  className={cn(
                    "flex w-full items-center gap-2 rounded-lg border p-2 text-left transition-colors focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
                    selectedRoot === volume.rootPath ? "border-primary/60 bg-accent" : "border-border bg-card hover:border-ring/50"
                  )}
                >
                  <HardDrive className="size-4 shrink-0 text-primary" aria-hidden="true" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">{volume.rootPath}</span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {volume.fileSystem ?? "unknown"} · {formatPercentUsed(volume)}
                      {volume.availableSpace != null ? ` · ${formatBytes(volume.availableSpace)} free` : null}
                    </span>
                    {hasCapacity ? (
                      <CapacityBar className="mt-1.5" used={(volume.totalSpace ?? 0) - (volume.availableSpace ?? 0)} total={volume.totalSpace ?? 0} label={`${volume.rootPath} capacity`} />
                    ) : null}
                  </span>
                </button>
              )
            })}
          </div>
        </div>
      ) : null}
    </aside>
  )
}
