import { useEffect, useState } from "react"
import { KeyRound, Loader2, Server } from "lucide-react"
import { toast } from "sonner"

import { Field, Panel, SelectInput, TextInput } from "@/components/common"
import { Button } from "@/components/ui/button"
import { AI_MODELS, getStoredAiModel, setStoredAiModel } from "@/lib/ai-models"
import {
  LOCAL_PRESETS,
  describeEndpoint,
  loadEndpoint,
  saveEndpoint,
  validateEndpoint,
  type AiEndpoint,
} from "@/lib/ai-config"
import { api, formatError } from "@/lib/api"
import { cn } from "@/lib/utils"

/**
 * Where the AI assistant runs: OpenAI (API key in Windows Credential Manager)
 * or a local OpenAI-compatible server on this PC (Ollama, LM Studio, AMD
 * Lemonade, Foundry Local). Local endpoints must be on loopback, so nothing
 * leaves the machine.
 */
export function AiSettingsPanel({ configured, onChanged }: { configured: boolean; onChanged: (configured: boolean) => void }) {
  const [keyInput, setKeyInput] = useState("")
  const [pending, setPending] = useState(false)
  const [model, setModel] = useState(getStoredAiModel)
  const [endpoint, setEndpoint] = useState<AiEndpoint>(loadEndpoint)

  useEffect(() => {
    setStoredAiModel(model)
  }, [model])

  const problem = validateEndpoint(endpoint)
  useEffect(() => {
    // Only a valid choice is persisted; an invalid draft would otherwise block the assistant.
    if (problem === null) saveEndpoint(endpoint)
  }, [endpoint, problem])

  async function saveKey() {
    if (!keyInput.trim() || pending) return
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
    if (pending) return
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

  const kinds = [
    { value: "openai" as const, label: "OpenAI", icon: KeyRound },
    { value: "local" as const, label: "Local model", icon: Server },
  ]

  return (
    <Panel title="AI assistant">
      <div className="space-y-4">
        <div role="radiogroup" aria-label="AI provider" className="inline-flex gap-1 rounded-lg border border-border bg-muted/50 p-1">
          {kinds.map((k) => (
            <button
              key={k.value}
              type="button"
              role="radio"
              aria-checked={endpoint.kind === k.value}
              onClick={() => setEndpoint((e) => ({ ...e, kind: k.value }))}
              className={cn(
                "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium transition-colors focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
                endpoint.kind === k.value ? "bg-card text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground"
              )}
            >
              <k.icon className="size-4" aria-hidden="true" />
              {k.label}
            </button>
          ))}
        </div>

        {endpoint.kind === "openai" ? (
          <div className="space-y-3">
            <p className="text-sm text-muted-foreground">
              The key is stored in Windows Credential Manager. Scan analysis sends file metadata (paths, sizes, dates); system analysis sends service/process names, battery state and hardware model names. File contents are never sent.
            </p>
            <div className="flex flex-wrap items-center gap-2">
              <TextInput
                type="password"
                value={keyInput}
                onChange={(event) => setKeyInput(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && keyInput.trim() && !pending) void saveKey()
                }}
                placeholder={configured ? "Key configured — enter to replace" : "sk-..."}
                className="max-w-80 flex-1"
                autoComplete="off"
                aria-label="OpenAI API key"
              />
              <Button onClick={saveKey} disabled={!keyInput.trim() || pending}>
                {pending ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                Save key
              </Button>
              {configured ? (
                <Button variant="outline" onClick={clearKey} disabled={pending}>
                  Remove key
                </Button>
              ) : null}
            </div>
            <Field label="Model">
              <SelectInput value={model} onChange={(event) => setModel(event.target.value)} className="max-w-60">
                {AI_MODELS.map((value) => (
                  <option key={value} value={value}>
                    {value}
                  </option>
                ))}
              </SelectInput>
            </Field>
          </div>
        ) : (
          <div className="space-y-3">
            <p className="text-sm text-muted-foreground">
              Use a model served on this PC through an OpenAI-compatible API. Requests go to the loopback address only, so scan and system data never leave the machine and no API key is needed. Tool-calling support depends on the model you choose.
            </p>
            <Field label="Server">
              <SelectInput
                value={LOCAL_PRESETS.find((p) => p.baseUrl === endpoint.baseUrl)?.id ?? "custom"}
                onChange={(event) => {
                  const preset = LOCAL_PRESETS.find((p) => p.id === event.target.value)
                  if (preset) setEndpoint((e) => ({ ...e, baseUrl: preset.baseUrl }))
                }}
                className="max-w-80"
              >
                {LOCAL_PRESETS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                  </option>
                ))}
                <option value="custom">Custom address</option>
              </SelectInput>
            </Field>
            <Field label="Address" hint={LOCAL_PRESETS.find((p) => p.baseUrl === endpoint.baseUrl)?.hint}>
              <TextInput value={endpoint.baseUrl} onChange={(e) => setEndpoint((x) => ({ ...x, baseUrl: e.target.value }))} placeholder="http://127.0.0.1:11434/v1" className="max-w-md" />
            </Field>
            <Field label="Model name">
              <TextInput value={endpoint.localModel} onChange={(e) => setEndpoint((x) => ({ ...x, localModel: e.target.value }))} placeholder="llama3.1:8b" className="max-w-80" />
            </Field>
            {problem ? <p className="text-xs text-destructive">{problem}</p> : null}
          </div>
        )}
        <p className="text-xs text-muted-foreground">Active: {describeEndpoint(endpoint)}</p>
      </div>
    </Panel>
  )
}
