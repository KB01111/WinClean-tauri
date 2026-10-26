import { useEffect, useState } from "react"
import { ShieldCheck, X } from "lucide-react"
import { toast } from "sonner"

import { Panel, TextInput } from "@/components/common"
import { Button } from "@/components/ui/button"
import { api, formatError } from "@/lib/api"

/**
 * Folders WinClean must never suggest, clean, offload or read for duplicates —
 * on top of the built-in protection for system folders, code projects and cloud-synced folders.
 */
export function ProtectedFoldersPanel() {
  const [folders, setFolders] = useState<string[]>([])
  const [input, setInput] = useState("")
  const [pending, setPending] = useState(false)

  useEffect(() => {
    api.storageExclusions().then(setFolders).catch(() => {})
  }, [])

  async function add() {
    if (!input.trim() || pending) return
    setPending(true)
    try {
      setFolders(await api.storageAddExclusion(input))
      setInput("")
      toast.success("Folder protected. Re-open the analysis to apply it.")
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setPending(false)
    }
  }

  async function remove(path: string) {
    try {
      setFolders(await api.storageRemoveExclusion(path))
    } catch (error) {
      toast.error(formatError(error))
    }
  }

  return (
    <Panel title="Protected folders">
      <p className="mb-3 flex items-start gap-1.5 text-sm text-muted-foreground">
        <ShieldCheck className="mt-0.5 size-4 shrink-0 text-primary" aria-hidden="true" />
        Windows folders, code projects, documents and cloud-synced folders are always protected. Add anything else you never want suggested.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <TextInput
          value={input}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void add()
          }}
          placeholder="D:\Photos\Wedding"
          className="max-w-md flex-1"
          aria-label="Folder to protect"
        />
        <Button onClick={() => void add()} disabled={!input.trim() || pending}>
          Protect folder
        </Button>
      </div>
      {folders.length > 0 ? (
        <ul className="mt-3 space-y-1">
          {folders.map((path) => (
            <li key={path} className="flex items-center justify-between gap-2 rounded-md border border-border px-3 py-1.5 text-sm">
              <span className="min-w-0 truncate" title={path}>
                {path}
              </span>
              <Button variant="ghost" size="sm" onClick={() => void remove(path)} aria-label={`Stop protecting ${path}`}>
                <X className="size-4" aria-hidden="true" />
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
    </Panel>
  )
}
