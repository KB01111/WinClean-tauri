import { AlertTriangle } from "lucide-react"

import { Badge, type BadgeTone } from "@/components/ui/badge"
import type { McpRegisteredServer, McpServerIdentity, ToolPolicy } from "@/lib/optimizer"

const POLICY: Record<ToolPolicy, { label: string; tone: BadgeTone }> = {
  metadataOnly: { label: "Metadata only", tone: "good" },
  consentedSearch: { label: "Search, with consent", tone: "info" },
  contentRead: { label: "Blocked: reads content", tone: "warn" },
  destructive: { label: "Blocked: can change data", tone: "bad" },
  unknown: { label: "Blocked: unclassified", tone: "warn" },
}

const IDENTITY: Record<McpServerIdentity, { label: string; tone: BadgeTone }> = {
  microsoftFileExplorer: { label: "Microsoft File Explorer", tone: "good" },
  microsoftPublished: { label: "Microsoft", tone: "info" },
  thirdParty: { label: "Third party", tone: "warn" },
  unverified: { label: "Publisher unverified", tone: "warn" },
}

/**
 * One server from the Windows on-device MCP registry. This is discovery only:
 * WinClean never starts a server because it appears here, and every tool it
 * declares is classified by WinClean's own policy.
 */
export function McpServerCard({ server }: { server: McpRegisteredServer }) {
  const identity = IDENTITY[server.identity]
  return (
    <section className="rounded-lg border border-border bg-card p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="min-w-0">
          <h4 className="font-medium">
            {server.name}
            {server.version ? <span className="ml-2 text-xs font-normal text-muted-foreground">{server.version}</span> : null}
          </h4>
          {server.description ? <p className="text-xs text-muted-foreground">{server.description}</p> : null}
        </div>
        <Badge tone={identity.tone}>{identity.label}</Badge>
      </div>
      {server.impersonationWarning ? (
        <p className="mt-2 flex items-start gap-1.5 text-xs text-destructive">
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          Named like File Explorer but not proven to be Microsoft&apos;s connector, so WinClean will not use it.
        </p>
      ) : null}
      {server.declaredTools.length > 0 ? (
        <ul className="mt-3 grid gap-1">
          {server.declaredTools.map((tool) => {
            const p = POLICY[tool.policy]
            return (
              <li key={tool.name} className="flex flex-wrap items-center justify-between gap-2 text-sm">
                <span className="min-w-0">
                  <code className="font-medium">{tool.name}</code>
                  {tool.description ? <span className="ml-2 text-xs text-muted-foreground">{tool.description}</span> : null}
                </span>
                <Badge tone={p.tone}>{p.label}</Badge>
              </li>
            )
          })}
        </ul>
      ) : (
        <p className="mt-2 text-xs text-muted-foreground">This server declares no tools in the registry.</p>
      )}
    </section>
  )
}
