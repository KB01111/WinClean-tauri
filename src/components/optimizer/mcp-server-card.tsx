import { useState } from "react"
import { Loader2 } from "lucide-react"

import { Notice, TextInput } from "@/components/common"
import { Badge, type BadgeTone } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { api, formatError } from "@/lib/api"
import type { McpServer, McpTool, ToolPolicy } from "@/lib/optimizer"

const POLICY: Record<ToolPolicy, { label: string; tone: BadgeTone; why: string }> = {
  metadataOnly: { label: "Allowed", tone: "good", why: "Returns metadata such as names, sizes and dates. WinClean may call it." },
  contentRead: { label: "Blocked", tone: "warn", why: "Would read file contents. WinClean never reads file contents through MCP." },
  destructive: { label: "Blocked", tone: "bad", why: "Could change or delete data. WinClean never calls it." },
  unknown: { label: "Blocked", tone: "warn", why: "WinClean does not recognise this tool, so it is blocked until classified." },
}

function ToolList({ tools }: { tools: McpTool[] }) {
  return (
    <ul className="grid gap-1.5">
      {tools.map((tool) => {
        const p = POLICY[tool.policy]
        return (
          <li key={tool.name} className="flex flex-wrap items-start justify-between gap-2 rounded-md border border-border bg-card px-3 py-2 text-sm">
            <span className="min-w-0">
              <code className="font-medium">{tool.name}</code>
              {tool.description ? <span className="block text-xs text-muted-foreground">{tool.description}</span> : null}
              <span className="block text-xs text-muted-foreground">{p.why}</span>
            </span>
            <Badge tone={p.tone}>{p.label}</Badge>
          </li>
        )
      })}
    </ul>
  )
}

/**
 * One Windows MCP server from the on-device registry. Inspecting it only lists
 * its tools; WinClean's single call is File Explorer metadata for one path,
 * and only through the permission-aware policy in the backend.
 */
export function McpServerCard({ server }: { server: McpServer }) {
  const [tools, setTools] = useState<McpTool[] | null>(server.tools.length ? server.tools : null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [path, setPath] = useState("")
  const [details, setDetails] = useState<unknown>(null)

  async function inspect() {
    setBusy(true)
    setError(null)
    try {
      setTools((await api.optimizerMcpProbe(server.id)).tools)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setBusy(false)
    }
  }

  async function lookup() {
    if (!path.trim()) return
    setBusy(true)
    setError(null)
    try {
      setDetails(await api.optimizerMcpFileDetails(server.id, path.trim().replace(/^"(.*)"$/, "$1")))
    } catch (e) {
      setError(formatError(e))
      setDetails(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="rounded-lg border border-border bg-card p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="min-w-0">
          <h4 className="font-medium">{server.name}</h4>
          {server.description ? <p className="text-xs text-muted-foreground">{server.description}</p> : null}
        </div>
        <div className="flex items-center gap-2">
          {server.isFileExplorer ? <Badge tone="info">File Explorer</Badge> : null}
          <Button size="sm" variant="outline" onClick={() => void inspect()} disabled={busy}>
            {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
            {tools ? "Re-inspect tools" : "Inspect tools"}
          </Button>
        </div>
      </div>
      {error ? (
        <div className="mt-3">
          <Notice tone="warning" text={error} />
        </div>
      ) : null}
      {tools ? (
        <div className="mt-3">
          <ToolList tools={tools} />
        </div>
      ) : null}
      {server.isFileExplorer && tools ? (
        <div className="mt-4 space-y-2">
          <h5 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">File details (metadata only)</h5>
          <div className="flex flex-wrap gap-2">
            <TextInput value={path} onChange={(e) => setPath(e.target.value)} onKeyDown={(e) => e.key === "Enter" && void lookup()} placeholder={String.raw`C:\Users\you\Downloads\file.zip`} aria-label="Path to look up" className="max-w-md flex-1" />
            <Button onClick={() => void lookup()} disabled={busy || !path.trim()}>
              Get details
            </Button>
          </div>
          {details != null ? <pre className="max-h-64 overflow-auto rounded-md bg-muted p-3 text-xs">{JSON.stringify(details, null, 2)}</pre> : null}
        </div>
      ) : null}
    </section>
  )
}
