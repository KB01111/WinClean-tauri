import { useState } from "react"
import { Loader2, Plug, Search, ShieldCheck, X } from "lucide-react"

import { Notice, Panel, TextInput } from "@/components/common"
import { Badge, type BadgeTone } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { api, formatError } from "@/lib/api"
import type { McpFileExplorerStatus, McpSearchOutcome, McpStatus } from "@/lib/optimizer"

const CONNECTION: Record<McpFileExplorerStatus["connection"], { label: string; tone: BadgeTone }> = {
  notAttempted: { label: "Not verified yet", tone: "neutral" },
  connected: { label: "Connected", tone: "good" },
  agentUserNeeded: { label: "Agent account needed", tone: "warn" },
  failed: { label: "Failed", tone: "bad" },
}

const PERMISSION: Record<McpFileExplorerStatus["permission"], { label: string; tone: BadgeTone }> = {
  unknown: { label: "Not asked", tone: "neutral" },
  granted: { label: "Granted", tone: "good" },
  denied: { label: "Denied", tone: "bad" },
}

/**
 * The File Explorer connector: verify it, authorise folders, then run
 * metadata-only searches. Nothing here runs until you click, a folder must be
 * authorised first, snippets are off unless you turn them on, and content-reading
 * or modifying tools are blocked by WinClean's policy.
 */
export function FileExplorerMcpPanel({ status, roots, onStatus, onRoots }: { status: McpStatus; roots: string[]; onStatus: (status: McpStatus) => void; onRoots: (roots: string[]) => void }) {
  const fe = status.fileExplorer
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [rootInput, setRootInput] = useState("")
  const [dir, setDir] = useState("")
  const [patterns, setPatterns] = useState("")
  const [snippets, setSnippets] = useState(false)
  const [outcome, setOutcome] = useState<McpSearchOutcome | null>(null)
  const [detailsPath, setDetailsPath] = useState("")
  const [details, setDetails] = useState<unknown>(null)

  async function guarded<T>(name: string, task: () => Promise<T>): Promise<T | undefined> {
    setBusy(name)
    setError(null)
    try {
      return await task()
    } catch (e) {
      setError(formatError(e))
      return undefined
    } finally {
      setBusy(null)
    }
  }

  if (!fe) {
    return (
      <Panel title="File Explorer connector">
        <p className="text-sm text-muted-foreground">The Microsoft File Explorer connector was not found or its identity could not be verified. {status.fallback}</p>
      </Panel>
    )
  }

  const connection = CONNECTION[fe.connection]
  const permission = PERMISSION[fe.permission]
  const effectiveDir = dir || roots[0] || ""
  const tools = fe.liveTools ?? fe.declaredTools

  return (
    <Panel
      title="File Explorer connector"
      action={
        busy ? (
          <Button size="sm" variant="outline" onClick={() => void api.optimizerMcpCancel()}>
            <Loader2 className="size-4 animate-spin" aria-hidden="true" />
            Cancel
          </Button>
        ) : (
          <Button size="sm" variant="outline" onClick={() => void guarded("verify", async () => onStatus(await api.optimizerMcpVerify()))}>
            <Plug className="size-4" aria-hidden="true" />
            Verify connection
          </Button>
        )
      }
    >
      {error ? <Notice tone="warning" text={error} /> : null}
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <Badge tone={connection.tone}>{connection.label}</Badge>
        <Badge tone={permission.tone}>Folder access: {permission.label}</Badge>
        {fe.verifiedExecution ? <Badge tone="good">A tool call succeeded</Badge> : null}
        {fe.toolsMatchDeclaration === false ? <Badge tone="warn">Live tools differ from the registry</Badge> : null}
      </div>
      {fe.connectionDetail ? <p className="mt-2 text-xs text-muted-foreground">{fe.connectionDetail}</p> : null}
      {fe.permissionDetail ? <p className="mt-1 text-xs text-muted-foreground">{fe.permissionDetail}</p> : null}

      <h5 className="mt-4 text-xs font-semibold tracking-wide text-muted-foreground uppercase">Authorised folders</h5>
      <p className="mb-2 text-xs text-muted-foreground">Searches only run inside folders you add here. Whole drives, system folders and links are refused.</p>
      <div className="flex flex-wrap gap-2">
        <TextInput value={rootInput} onChange={(e) => setRootInput(e.target.value)} placeholder={String.raw`C:\Users\you\Downloads`} aria-label="Folder to authorise" className="max-w-md flex-1" />
        <Button
          variant="outline"
          disabled={!rootInput.trim() || busy !== null}
          onClick={() =>
            void guarded("add", async () => {
              onRoots(await api.optimizerMcpAddSearchRoot(rootInput.trim()))
              setRootInput("")
            })
          }
        >
          Authorise
        </Button>
      </div>
      {roots.length > 0 ? (
        <ul className="mt-2 space-y-1">
          {roots.map((root) => (
            <li key={root} className="flex items-center justify-between gap-2 rounded-md border border-border px-3 py-1.5 text-sm">
              <span className="min-w-0 truncate" title={root}>
                {root}
              </span>
              <span className="flex shrink-0 gap-1">
                <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => void guarded("perm", async () => onStatus(await api.optimizerMcpVerifyPermission(root)))}>
                  <ShieldCheck className="size-4" aria-hidden="true" />
                  Check access
                </Button>
                <Button size="sm" variant="ghost" aria-label={`Remove ${root}`} onClick={() => void guarded("remove", async () => onRoots(await api.optimizerMcpRemoveSearchRoot(root)))}>
                  <X className="size-4" aria-hidden="true" />
                </Button>
              </span>
            </li>
          ))}
        </ul>
      ) : null}

      <h5 className="mt-4 text-xs font-semibold tracking-wide text-muted-foreground uppercase">Search (names and dates)</h5>
      <div className="flex flex-wrap items-center gap-2">
        <select value={effectiveDir} onChange={(e) => setDir(e.target.value)} disabled={roots.length === 0} aria-label="Folder to search" className="h-9 max-w-80 rounded-md border border-input bg-card px-2 text-sm">
          {roots.length === 0 ? <option value="">Authorise a folder first</option> : null}
          {roots.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
        <TextInput value={patterns} onChange={(e) => setPatterns(e.target.value)} placeholder="*.iso, backup" aria-label="Search patterns" className="max-w-60 flex-1" />
        <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <input type="checkbox" checked={snippets} onChange={(e) => setSnippets(e.target.checked)} />
          Include text snippets (may show private text)
        </label>
        <Button
          disabled={!effectiveDir || !patterns.trim() || busy !== null || !fe.search.available}
          onClick={() => void guarded("search", async () => setOutcome(await api.optimizerMcpSearch({ startingDir: effectiveDir, patterns: patterns.trim(), includeSnippets: snippets, limit: 100 })))}
        >
          {busy === "search" ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Search className="size-4" aria-hidden="true" />}
          Search
        </Button>
      </div>
      {!fe.search.available ? <p className="mt-1 text-xs text-muted-foreground">The connector does not offer file search. {fe.search.fallback}</p> : null}
      {outcome ? (
        <div className="mt-3 space-y-2">
          <p className="text-xs text-muted-foreground">
            {outcome.hits.length} result(s){outcome.truncated ? " (more exist)" : ""}
            {outcome.rejected ? `, ${outcome.rejected} rejected by WinClean's checks` : ""}
            {outcome.plan.unsupportedFilters.length ? `, ignored filters: ${outcome.plan.unsupportedFilters.join(", ")}` : ""}
          </p>
          <ul className="max-h-64 space-y-1 overflow-auto text-xs">
            {outcome.hits.map((hit) => (
              <li key={hit.path} className="rounded border border-border px-2 py-1">
                <button type="button" className="block max-w-full truncate text-left font-medium hover:text-primary" title="Use for file details" onClick={() => setDetailsPath(hit.path)}>
                  {hit.path}
                </button>
                {hit.snippet ? <span className="block text-muted-foreground">{hit.snippet}</span> : null}
              </li>
            ))}
          </ul>
          {outcome.notes.map((n) => (
            <p key={n} className="text-xs text-muted-foreground">
              {n}
            </p>
          ))}
        </div>
      ) : null}

      <h5 className="mt-4 text-xs font-semibold tracking-wide text-muted-foreground uppercase">File details (metadata only)</h5>
      <div className="flex flex-wrap gap-2">
        <TextInput value={detailsPath} onChange={(e) => setDetailsPath(e.target.value)} placeholder={String.raw`C:\Users\you\Downloads\file.zip`} aria-label="Path to look up" className="max-w-md flex-1" />
        <Button variant="outline" disabled={!detailsPath.trim() || busy !== null} onClick={() => void guarded("details", async () => setDetails(await api.optimizerMcpFileDetails(detailsPath.trim().replace(/^"(.*)"$/, "$1"))))}>
          Get details
        </Button>
      </div>
      {details != null ? <pre className="mt-2 max-h-64 overflow-auto rounded-md bg-muted p-3 text-xs">{JSON.stringify(details, null, 2)}</pre> : null}

      {tools.length > 0 ? (
        <details className="mt-4 text-xs">
          <summary className="cursor-pointer font-medium text-muted-foreground">Tools ({fe.liveTools ? "live" : "declared"}) and WinClean&apos;s policy</summary>
          <ul className="mt-2 space-y-1">
            {tools.map((t) => (
              <li key={t.name}>
                <code className="font-medium">{t.name}</code> — {t.policyNote}
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </Panel>
  )
}
