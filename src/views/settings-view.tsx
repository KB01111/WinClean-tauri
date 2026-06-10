import { useEffect, useState } from "react"
import {
  Cloud,
  Database,
  Download,
  KeyRound,
  Loader2,
  Monitor,
  Moon,
  ShieldCheck,
  Sun,
  Trash2,
} from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Panel, Posture, SummaryList, TextInput } from "@/components/common"
import { useTheme } from "@/components/theme-provider"
import { api, formatError, type AppHealth, type GoogleDriveStatus } from "@/lib/api"
import { formatNumber } from "@/lib/format"
import { AI_MODELS, getStoredAiModel, setStoredAiModel } from "@/lib/ai-agent"
import { cn } from "@/lib/utils"

export function SettingsView({
  health,
  driveStatus,
  onConnectDrive,
  onDisconnectDrive,
  onExportDiagnostics,
  diagnosticsPending,
  aiKeyConfigured,
  onAiKeyChanged,
}: {
  health: AppHealth | null
  driveStatus: GoogleDriveStatus | null
  onConnectDrive: () => void
  onDisconnectDrive: () => void
  onExportDiagnostics: () => void
  diagnosticsPending: boolean
  aiKeyConfigured: boolean
  onAiKeyChanged: (configured: boolean) => void
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
          configured={aiKeyConfigured}
          onChanged={onAiKeyChanged}
        />

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
            ["Active scan", health?.activeScanState ?? "unknown"],
            [
              "Latest run",
              health?.latestCompletedRunId
                ? String(health.latestCompletedRunId)
                : "none",
            ],
            ["Pending actions", formatNumber(health?.pendingActionCount ?? 0)],
            [
              "Pending offloads",
              formatNumber(health?.pendingOffloadCount ?? 0),
            ],
            ["Database", health?.databasePath ?? "unknown"],
          ]}
        />
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

function AiSettingsPanel({
  configured,
  onChanged,
}: {
  configured: boolean
  onChanged: (configured: boolean) => void
}) {
  const [keyInput, setKeyInput] = useState("")
  const [pending, setPending] = useState(false)
  const [model, setModel] = useState(getStoredAiModel)

  useEffect(() => {
    setStoredAiModel(model)
  }, [model])

  async function saveKey() {
    if (!keyInput.trim() || pending) {
      return
    }
    setPending(true)
    try {
      const status = await api.setAiApiKey(keyInput.trim())
      onChanged(status.configured)
      setKeyInput("")
      toast.success("AI API key saved to Windows Credential Manager.")
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setPending(false)
    }
  }

  async function clearKey() {
    if (pending) {
      return
    }
    setPending(true)
    try {
      const status = await api.clearAiApiKey()
      onChanged(status.configured)
      toast.success("AI API key removed.")
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setPending(false)
    }
  }

  return (
    <Panel title="AI assistant">
      <div className="space-y-3">
        <div className="flex items-start gap-2 text-sm text-muted-foreground">
          <KeyRound className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <p>
            Add an OpenAI API key to let the AI assistant analyze scan results.
            The key is stored in Windows Credential Manager and only file
            metadata (paths, sizes, dates) is sent to OpenAI — never file
            contents.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <TextInput
            type="password"
            value={keyInput}
            onChange={(event) => setKeyInput(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && keyInput.trim() && !pending) {
                void saveKey()
              }
            }}
            placeholder={configured ? "Key configured — enter to replace" : "sk-..."}
            className="max-w-80 flex-1"
            autoComplete="off"
          />
          <Button onClick={saveKey} disabled={!keyInput.trim() || pending}>
            {pending ? (
              <Loader2 className="size-4 animate-spin" aria-hidden="true" />
            ) : null}
            Save key
          </Button>
          {configured ? (
            <Button variant="outline" onClick={clearKey} disabled={pending}>
              Remove key
            </Button>
          ) : null}
        </div>
        <label className="flex items-center gap-2 text-sm">
          <span className="text-muted-foreground">Model</span>
          <select
            value={model}
            onChange={(event) => setModel(event.target.value)}
            className="h-9 rounded-md border border-input bg-card px-2 text-sm outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/40"
          >
            {AI_MODELS.map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </select>
        </label>
      </div>
    </Panel>
  )
}
