import {
  BarChart3,
  FolderTree,
  HardDrive,
  History,
  Loader2,
  Settings,
  Sparkles,
  Trash2,
  BatteryCharging,
  Cpu,
  Leaf,
  Server,
  Undo2,
} from "lucide-react"

import { CapacityBar, NavButton } from "@/components/common"
import { formatBytes, formatPercentUsed } from "@/lib/format"
import type { VolumeInfo } from "@/lib/api"
import { cn } from "@/lib/utils"

export type View =
  | "overview"
  | "explorer"
  | "cleanup"
  | "services"
  | "processes"
  | "power"
  | "hardware"
  | "optimizerHistory"
  | "history"
  | "insights"
  | "settings"

const NAV_ITEMS: { view: View; label: string; icon: typeof BarChart3 }[] = [
  { view: "overview", label: "Overview", icon: BarChart3 },
  { view: "explorer", label: "Explorer", icon: FolderTree },
  { view: "cleanup", label: "Recover space", icon: Trash2 },
  { view: "services", label: "Windows Services", icon: Server },
  { view: "processes", label: "Processes & EcoQoS", icon: Leaf },
  { view: "power", label: "Battery & Power", icon: BatteryCharging },
  { view: "hardware", label: "Hardware & NPU", icon: Cpu },
  { view: "optimizerHistory", label: "Optimization History", icon: Undo2 },
  { view: "insights", label: "AI insights", icon: Sparkles },
  { view: "history", label: "History", icon: History },
  { view: "settings", label: "Settings", icon: Settings },
]

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
    <aside className="flex flex-col gap-6 border-r border-sidebar-border bg-sidebar p-4 text-sidebar-foreground">
      <div className="flex items-center gap-3">
        <div className="flex size-10 items-center justify-center rounded-lg bg-primary text-primary-foreground shadow-sm">
          <HardDrive className="size-5" aria-hidden="true" />
        </div>
        <div>
          <h1 className="text-xl font-semibold tracking-tight">WinClean</h1>
          <p className="text-xs text-muted-foreground">Local disk analyzer</p>
        </div>
      </div>

      <nav className="space-y-1">
        {NAV_ITEMS.map((item) => (
          <NavButton
            key={item.view}
            icon={item.icon}
            label={item.label}
            active={view === item.view}
            onClick={() => onViewChange(item.view)}
          />
        ))}
      </nav>

      <div className="rounded-xl border border-sidebar-border bg-card p-3 shadow-sm">
        <div className="mb-2 flex items-center justify-between">
          <span className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            Scan target
          </span>
          {loadingVolumes ? (
            <Loader2 className="size-4 animate-spin text-primary" />
          ) : null}
        </div>
        <div className="space-y-2">
          {volumes.length === 0 && !loadingVolumes ? (
            <p className="py-2 text-center text-xs text-muted-foreground">
              No volumes detected
            </p>
          ) : null}
          {volumes.map((volume) => {
            const totalSpace = volume.totalSpace
            const availableSpace = volume.availableSpace
            const hasCapacity = totalSpace != null && availableSpace != null
            return (
              <button
                type="button"
                key={volume.rootPath}
                onClick={() => onSelectRoot(volume.rootPath)}
                aria-label={`Scan drive ${volume.rootPath}, ${formatPercentUsed(volume)}`}
                className={cn(
                  "flex w-full items-center gap-2 rounded-lg border p-2 text-left transition-colors",
                  selectedRoot === volume.rootPath
                    ? "border-primary/60 bg-accent"
                    : "border-border bg-card hover:border-ring/50"
                )}
              >
                <HardDrive
                  className="size-4 shrink-0 text-primary"
                  aria-hidden="true"
                />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium">
                    {volume.rootPath}
                  </span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {volume.fileSystem ?? "unknown"} ·{" "}
                    {formatPercentUsed(volume)}
                    {volume.availableSpace != null
                      ? ` · ${formatBytes(volume.availableSpace)} free`
                      : null}
                  </span>
                  {hasCapacity ? (
                    <CapacityBar
                      className="mt-1.5"
                      used={(volume.totalSpace ?? 0) - (volume.availableSpace ?? 0)}
                      total={volume.totalSpace ?? 0}
                      label={`${volume.rootPath} capacity`}
                    />
                  ) : null}
                </span>
              </button>
            )
          })}
        </div>
      </div>
    </aside>
  )
}
