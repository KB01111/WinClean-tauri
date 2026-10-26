import { useState } from "react"
import { ArrowRight, ChevronDown, ChevronRight, ShieldCheck } from "lucide-react"

import { StatusBadge } from "@/components/common"
import { Button } from "@/components/ui/button"
import type { ValidatedProposal } from "@/lib/api"
import { formatBytes } from "@/lib/format"

const SHOWN_FILES = 8

/**
 * One backend-validated proposal. Every number shown here was recomputed by
 * WinClean from its own scan; a model's claims are only ever compared, never shown as fact.
 */
export function ProposalCard({ proposal, onApply }: { proposal: ValidatedProposal; onApply: (proposal: ValidatedProposal) => void }) {
  const [open, setOpen] = useState(false)
  const { preview } = proposal
  const empty = proposal.items.length === 0 || proposal.action === null
  return (
    <div className="rounded-lg border border-border bg-card p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium">{proposal.title || "Suggestion"}</span>
            {proposal.action ? <StatusBadge status={proposal.action} /> : null}
          </div>
          {proposal.reason ? <p className="mt-1 text-sm text-muted-foreground">{proposal.reason}</p> : null}
          <p className="mt-1 text-xs text-muted-foreground">
            {proposal.items.length} verified file(s) · {formatBytes(proposal.verifiedBytes)}
            {proposal.claimedBytes !== null && !proposal.claimMatches ? ` · the model estimated ${formatBytes(proposal.claimedBytes)}; WinClean's figure is the one to trust` : ""}
          </p>
        </div>
        {proposal.action && proposal.action !== "review" ? (
          <Button variant="outline" size="sm" disabled={empty} onClick={() => onApply(proposal)}>
            Add to selection
            <ArrowRight className="size-4" aria-hidden="true" />
          </Button>
        ) : proposal.action === "review" ? (
          <Button variant="outline" size="sm" disabled={empty} onClick={() => onApply(proposal)}>
            Review in Recover space
            <ArrowRight className="size-4" aria-hidden="true" />
          </Button>
        ) : null}
      </div>

      {!empty ? (
        <p className="mt-2 flex items-start gap-1.5 text-xs text-muted-foreground">
          <ShieldCheck className="mt-0.5 size-3.5 shrink-0 text-primary" aria-hidden="true" />
          <span>
            {preview.summary}. {preview.reversible ? "Reversible. " : ""}Nothing is permanently deleted.
          </span>
        </p>
      ) : null}

      <button type="button" onClick={() => setOpen((v) => !v)} className="mt-3 flex items-center gap-1 text-xs font-medium text-muted-foreground hover:text-foreground" aria-expanded={open}>
        {open ? <ChevronDown className="size-3.5" aria-hidden="true" /> : <ChevronRight className="size-3.5" aria-hidden="true" />}
        Files, checks{proposal.rejected.length > 0 ? ` and ${proposal.rejected.length} rejected id(s)` : ""}
      </button>
      {open ? (
        <div className="mt-2 space-y-3 text-xs">
          {proposal.items.length > 0 ? (
            <ul className="space-y-1">
              {proposal.items.slice(0, SHOWN_FILES).map((item) => (
                <li key={item.id} className="flex justify-between gap-3">
                  <span className="min-w-0 truncate" title={item.path}>
                    {item.path}
                  </span>
                  <span className="shrink-0 text-muted-foreground tabular-nums">{formatBytes(item.size)}</span>
                </li>
              ))}
              {proposal.items.length > SHOWN_FILES ? <li className="text-muted-foreground">and {proposal.items.length - SHOWN_FILES} more</li> : null}
            </ul>
          ) : null}
          {preview.steps.length > 0 ? (
            <ul className="list-disc space-y-0.5 pl-4 text-muted-foreground">
              {preview.steps.concat(preview.notes).map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          ) : null}
          {proposal.rejected.length > 0 ? (
            <div>
              <p className="font-medium">Left out by WinClean&apos;s checks</p>
              <ul className="mt-1 list-disc space-y-0.5 pl-4 text-muted-foreground">
                {proposal.rejected.slice(0, 6).map((r, index) => (
                  <li key={`${r.candidateId}-${index}`}>
                    {r.candidateId ? `${r.candidateId}: ` : ""}
                    {r.reason}
                  </li>
                ))}
                {proposal.rejected.length > 6 ? <li>and {proposal.rejected.length - 6} more</li> : null}
              </ul>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
