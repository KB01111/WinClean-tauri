import { useCallback, useEffect, useState } from "react"
import { Copy, Loader2, RefreshCw } from "lucide-react"
import { toast } from "sonner"

import { EmptyState, Metric, Notice, Panel } from "@/components/common"
import { McpServerCard } from "@/components/optimizer/mcp-server-card"
import { Button } from "@/components/ui/button"
import { api, formatError } from "@/lib/api"
import type { McpDiscovery } from "@/lib/optimizer"

const SERVER_TOOLS = [
  ["winclean_services", "Windows services with classification"],
  ["winclean_service_recommendations", "Safe start-type recommendations"],
  ["winclean_processes", "Top processes and EcoQoS eligibility"],
  ["winclean_battery", "Battery telemetry (real watts, health)"],
  ["winclean_hardware", "CPU topology, GPUs, NPU"],
  ["winclean_capabilities", "Windows ML execution-provider assessment"],
  ["winclean_workload", "Current workload classification"],
] as const

const CLIENT_CONFIG = JSON.stringify({ mcpServers: { winclean: { command: String.raw`C:\Program Files\WinClean\winclean.exe`, args: ["--mcp-stdio"] } } }, null, 2)

/**
 * MCP Integrations. Two directions, both read-only for WinClean's own data:
 * WinClean can discover Windows' on-device MCP servers (File Explorer), and
 * WinClean can serve its diagnostics to an agent (for example Lumen) over
 * stdio. Neither direction can change anything.
 */
export function McpView() {
  const [discovery, setDiscovery] = useState<McpDiscovery | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      setDiscovery(await api.optimizerMcpDiscover())
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

  return (
    <div className="space-y-4">
      {error ? <Notice tone="danger" text={error} /> : null}
      <div className="grid gap-3 sm:grid-cols-3">
        <Metric label="Windows MCP registry" value={!discovery ? "—" : discovery.odrAvailable ? "Available" : "Not found"} hint={discovery?.buildSupported === false ? "Needs a newer Windows build" : undefined} />
        <Metric label="Servers found" value={discovery ? String(discovery.servers.length) : "—"} />
        <Metric label="File Explorer MCP" value={!discovery ? "—" : discovery.fileExplorerFound ? "Found" : "Not found"} hint="optional" />
      </div>

      <Panel
        title="Windows on-device MCP servers"
        action={
          <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Rediscover
          </Button>
        }
      >
        <p className="mb-3 text-sm text-muted-foreground">
          WinClean keeps its own native file scanner. If Windows exposes the File Explorer MCP server, WinClean can ask it for metadata about one path — never file contents, never changes — and only for tools it recognises as read-only.
        </p>
        {loading && !discovery ? (
          <EmptyState text="Looking for the Windows MCP registry…" />
        ) : discovery && discovery.servers.length > 0 ? (
          <div className="grid gap-3">
            {discovery.servers.map((s) => (
              <McpServerCard key={s.id} server={s} />
            ))}
          </div>
        ) : (
          <EmptyState text="No Windows MCP servers were found on this PC." />
        )}
        {discovery?.notes.length ? (
          <ul className="mt-3 list-disc space-y-0.5 pl-5 text-xs text-muted-foreground">
            {discovery.notes.map((n) => (
              <li key={n}>{n}</li>
            ))}
          </ul>
        ) : null}
      </Panel>

      <Panel title="Use WinClean from another agent (for example Lumen)">
        <p className="mb-3 text-sm text-muted-foreground">
          WinClean can run as a local, read-only MCP server over stdio. An agent can read these diagnostics; it cannot change a service, process, profile or file through WinClean. Any change still needs the approval flow inside WinClean.
        </p>
        <ul className="mb-4 grid gap-1 text-sm">
          {SERVER_TOOLS.map(([name, description]) => (
            <li key={name} className="flex flex-wrap gap-x-2">
              <code className="font-medium">{name}</code>
              <span className="text-muted-foreground">{description}</span>
            </li>
          ))}
        </ul>
        <div className="relative">
          <pre className="overflow-auto rounded-md bg-muted p-3 text-xs">{CLIENT_CONFIG}</pre>
          <Button
            size="sm"
            variant="outline"
            className="absolute top-2 right-2"
            onClick={() => {
              void navigator.clipboard
                .writeText(CLIENT_CONFIG)
                .then(() => toast.success("Configuration copied. Adjust the path to where WinClean is installed."))
                .catch(() => toast.error("Could not copy to the clipboard."))
            }}
          >
            <Copy className="size-4" aria-hidden="true" />
            Copy
          </Button>
        </div>
        <p className="mt-2 text-xs text-muted-foreground">Replace the path with the real location of winclean.exe. Microsoft Learn MCP is used only while developing WinClean and is not part of the app.</p>
      </Panel>
    </div>
  )
}
