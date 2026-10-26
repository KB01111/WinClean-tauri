import {
  Cloud,
  Database,
  Download,
  FolderOpen,
  Loader2,
  Monitor,
  Moon,
  ShieldCheck,
  Sun,
  Trash2,
} from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { AiSettingsPanel } from "@/components/ai/ai-settings-panel"
import { ProtectedFoldersPanel } from "@/components/protected-folders-panel"
import { ElevationBanner } from "@/components/optimizer/elevation-banner"
import { Panel, Posture, SummaryList } from "@/components/common"
import { useTheme } from "@/components/theme-provider"
import { api, formatError, type AiKeyMap, type AiKeyProvider, type AppHealth, type GoogleDriveStatus } from "@/lib/api"
import { formatNumber } from "@/lib/format"
import { useHelper } from "@/hooks/use-helper"
import { cn } from "@/lib/utils"

const SCAN_STATE_LABELS: Record<string, string> = {
  idle: "Idle",
  running: "Scanning…",
  completed: "Completed",
  failed: "Failed",
  canceled: "Canceled",
}

/** Return the containing folder name for a database path, falling back to the original path. */
function databaseFolderName(path: string) {
  const segments = path.split(/[\\/]/).filter(Boolean)
  segments.pop()
  return segments[segments.length - 1] ?? path
}

/** Return a database path’s parent using Windows path separators. */
function databaseParentPath(path: string) {
  return path.split(/[\\/]/).slice(0, -1).join("\\")
}

/** Render appearance, Drive, AI, diagnostics, and database health settings and actions. */
export function SettingsView({
  health,
  driveStatus,
  onConnectDrive,
  onDisconnectDrive,
  onExportDiagnostics,
  diagnosticsPending,
  aiKeys,
  onAiKeyChanged,
}: {
  health: AppHealth | null
  driveStatus: GoogleDriveStatus | null
  onConnectDrive: () => void
  onDisconnectDrive: () => void
  onExportDiagnostics: () => void
  diagnosticsPending: boolean
  aiKeys: AiKeyMap
  onAiKeyChanged: (provider: AiKeyProvider, configured: boolean) => void
}) {
  return (
    <div className="grid gap-4 xl:grid-cols-[1fr_380px]">
      <div className="space-y-4">
        <Panel title="Appearance">
          <ThemeToggle />
        </Panel>

        <Panel title="Google Drive">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="min-w-0 text-sm">
              <p className="font-medium">
                {driveStatus?.connected
                  ? (driveStatus.accountEmail ?? "Connected account")
                  : driveStatus?.configured
                    ? "Not connected"
                    : "Not configured"}
              </p>
              <p className="mt-0.5 text-muted-foreground">
                {driveStatus?.connected
                  ? "Offloaded files are uploaded to the WinClean Offloads folder and verified before the local copy is recycled."
                  : driveStatus?.configured
                    ? "Sign in to enable verified offload batches."
                    : "Set WINCLEAN_GOOGLE_CLIENT_ID to enable Google Drive offload."}
              </p>
            </div>
            <Button
              variant="outline"
              disabled={!driveStatus?.configured}
              onClick={
                driveStatus?.connected ? onDisconnectDrive : onConnectDrive
              }
            >
              <Cloud className="size-4" aria-hidden="true" />
              {driveStatus?.connected ? "Disconnect" : "Connect"}
            </Button>
          </div>
        </Panel>

        <AiSettingsPanel
          keys={aiKeys}
          onChanged={onAiKeyChanged}
        />

        <ProtectedFoldersPanel />

        <HelperPanel />

        <Panel
          title="Production posture"
          action={
            <Button
              variant="outline"
              onClick={onExportDiagnostics}
              disabled={diagnosticsPending}
            >
              {diagnosticsPending ? (
                <Loader2 className="size-4 animate-spin" aria-hidden="true" />
              ) : (
                <Download className="size-4" aria-hidden="true" />
              )}
              {diagnosticsPending ? "Exporting" : "Export diagnostics"}
            </Button>
          }
        >
          <div className="grid gap-3 md:grid-cols-2">
            <Posture
              icon={ShieldCheck}
              title="Privacy"
              text="Scan data stays local; selected files leave the device only when offloaded or analyzed with your AI key."
            />
            <Posture
              icon={Trash2}
              title="Cleanup"
              text="Cleanup uses Windows Recycle Bin; permanent delete is not exposed."
            />
            <Posture
              icon={Database}
              title="Storage"
              text={`SQLite schema ${health?.schemaVersion ?? "unknown"}`}
            />
            <Posture
              icon={Cloud}
              title="Google Drive"
              text={
                health?.googleDriveConnected
                  ? "Connected for verified offload batches."
                  : health?.googleDriveConfigured
                    ? "Configured, waiting for sign-in."
                    : "Set WINCLEAN_GOOGLE_CLIENT_ID to enable offload."
              }
            />
          </div>
        </Panel>
      </div>

      <Panel title="App health">
        <SummaryList
          rows={[
            ["Version", health?.appVersion ?? "unknown"],
            [
              "Active scan",
              health
                ? (SCAN_STATE_LABELS[health.activeScanState] ??
                  health.activeScanState)
                : "unknown",
            ],
            [
              "Latest run",
              health?.latestCompletedRunId
                ? `Scan #${health.latestCompletedRunId}`
                : "none",
            ],
            ["Pending actions", formatNumber(health?.pendingActionCount ?? 0)],
            [
              "Pending offloads",
              formatNumber(health?.pendingOffloadCount ?? 0),
            ],
            [
              "Data folder",
              health?.databasePath
                ? databaseFolderName(health.databasePath)
                : "unknown",
            ],
          ]}
        />
        <Button
          variant="outline"
          size="sm"
          className="mt-3"
          disabled={!health?.databasePath}
          onClick={() => {
            const databasePath = health?.databasePath
            if (!databasePath) {
              return
            }
            api.revealInExplorer(databaseParentPath(databasePath)).catch(
              (error) => {
                toast.error(formatError(error))
              }
            )
          }}
        >
          <FolderOpen className="size-4" aria-hidden="true" />
          Open data folder
        </Button>
      </Panel>
    </div>
  )
}

function ThemeToggle() {
  const { theme, setTheme } = useTheme()
  const options = [
    { value: "light" as const, label: "Light", icon: Sun },
    { value: "dark" as const, label: "Dark", icon: Moon },
    { value: "system" as const, label: "System", icon: Monitor },
  ]
  return (
    <div className="flex flex-wrap items-center justify-between gap-3">
      <p className="text-sm text-muted-foreground">
        Choose how WinClean looks. Press{" "}
        <kbd className="rounded border border-border bg-muted px-1.5 py-0.5 text-xs font-medium">
          D
        </kbd>{" "}
        anywhere to toggle.
      </p>
      <div className="flex gap-1 rounded-lg border border-border bg-muted/50 p-1">
        {options.map((option) => (
          <button
            key={option.value}
            type="button"
            onClick={() => setTheme(option.value)}
            className={cn(
              "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium transition-colors",
              theme === option.value
                ? "bg-card text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground"
            )}
          >
            <option.icon className="size-4" aria-hidden="true" />
            {option.label}
          </button>
        ))}
      </div>
    </div>
  )
}

/** Administrator helper: WinClean stays unprivileged and elevates one narrow process on demand. */
function HelperPanel() {
  const helper = useHelper()
  return (
    <Panel title="Administrator helper">
      <ElevationBanner helper={helper} />
      <ul className="list-disc space-y-1 pl-5 text-xs text-muted-foreground">
        <li>The app itself never runs as administrator. One UAC prompt starts a separate helper from the same signed executable.</li>
        <li>The helper accepts a single kind of request — change the start type of a service — and checks every request against the protected-service rules again before acting.</li>
        <li>It talks to WinClean over a private pipe that only your account can open, rejects any other client, and exits when WinClean closes or after 10 idle minutes.</li>
      </ul>
    </Panel>
  )
}
