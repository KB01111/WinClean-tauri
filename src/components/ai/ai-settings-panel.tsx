import { useEffect, useState } from "react"
import { Cloud, KeyRound, Loader2, RefreshCw, Server, Terminal } from "lucide-react"
import { toast } from "sonner"

import { Field, Notice, Panel, SelectInput, TextInput } from "@/components/common"
import { Button } from "@/components/ui/button"
import { AI_MODELS, getStoredAiModel, setStoredAiModel } from "@/lib/ai-models"
import {
  LOCAL_PRESETS,
  OPENROUTER_FREE_ROUTER,
  describeEndpoint,
  loadEndpoint,
  saveEndpoint,
  validateEndpoint,
  type AiEndpoint,
  type AiEndpointKind,
} from "@/lib/ai-config"
import { fetchOpenRouterFreeModels, type OpenRouterModel } from "@/lib/ai-provider"
import { api, formatError, type AiKeyMap, type AiKeyProvider, type OpenCodeStatus } from "@/lib/api"
import { cn } from "@/lib/utils"

const KINDS: { value: AiEndpointKind; label: string; icon: typeof KeyRound }[] = [
  { value: "openrouter", label: "OpenRouter (free models)", icon: Cloud },
  { value: "opencode", label: "OpenCode", icon: Terminal },
  { value: "openai", label: "OpenAI", icon: KeyRound },
  { value: "local", label: "Local model", icon: Server },
]

/** A looks-free OpenCode model id: the `-free` convention plus Big Pickle. */
function looksFree(model: string) {
  return /(^|\/)(big-pickle|.*-free)$/.test(model)
}

function KeyEditor({ provider, label, placeholder, configured, onChanged }: { provider: AiKeyProvider; label: string; placeholder: string; configured: boolean; onChanged: (provider: AiKeyProvider, configured: boolean) => void }) {
  const [keyInput, setKeyInput] = useState("")
  const [pending, setPending] = useState(false)

  async function save() {
    if (!keyInput.trim() || pending) return
    setPending(true)
    try {
      const status = await api.setAiApiKey(keyInput.trim(), provider)
      onChanged(provider, status.configured)
      setKeyInput("")
      toast.success(`${label} API key saved to Windows Credential Manager.`)
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setPending(false)
    }
  }

  async function clear() {
    if (pending) return
    setPending(true)
    try {
      const status = await api.clearAiApiKey(provider)
      onChanged(provider, status.configured)
      toast.success(`${label} API key removed.`)
    } catch (error) {
      toast.error(formatError(error))
    } finally {
      setPending(false)
    }
  }

  return (
    <div className="flex flex-wrap items-center gap-2">
      <TextInput
        type="password"
        value={keyInput}
        onChange={(event) => setKeyInput(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && keyInput.trim() && !pending) void save()
        }}
        placeholder={configured ? "Key configured — enter to replace" : placeholder}
        className="max-w-80 flex-1"
        autoComplete="off"
        aria-label={`${label} API key`}
      />
      <Button onClick={save} disabled={!keyInput.trim() || pending}>
        {pending ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
        Save key
      </Button>
      {configured ? (
        <Button variant="outline" onClick={clear} disabled={pending}>
          Remove key
        </Button>
      ) : null}
    </div>
  )
}

function OpenRouterSection({ endpoint, setEndpoint, keys, onKeyChanged }: { endpoint: AiEndpoint; setEndpoint: (update: (e: AiEndpoint) => AiEndpoint) => void; keys: AiKeyMap; onKeyChanged: (provider: AiKeyProvider, configured: boolean) => void }) {
  const [models, setModels] = useState<OpenRouterModel[]>([])
  const [loading, setLoading] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)

  async function loadModels() {
    setLoading(true)
    setLoadError(null)
    try {
      const key = keys.openrouter ? await api.getAiApiKey("openrouter") : null
      const list = await fetchOpenRouterFreeModels(key)
      setModels(list)
      if (list.length === 0) setLoadError("OpenRouter lists no free models right now.")
    } catch (error) {
      setLoadError(formatError(error))
    } finally {
      setLoading(false)
    }
  }

  const known = models.some((m) => m.id === endpoint.openrouterModel) || endpoint.openrouterModel === OPENROUTER_FREE_ROUTER
  return (
    <div className="space-y-3">
      <p className="text-sm text-muted-foreground">
        OpenRouter offers many models at no cost. Create a free key at openrouter.ai/keys; it is stored in Windows Credential Manager. Free models are limited to about 50 requests per day (1,000 with $10 of credit) — one analysis uses one request. Only file metadata (paths, sizes, dates) is sent, never file contents, and free-model providers may log prompts.
      </p>
      <KeyEditor provider="openrouter" label="OpenRouter" placeholder="sk-or-..." configured={keys.openrouter} onChanged={onKeyChanged} />
      <Field label="Free model" hint="“Free models router” lets OpenRouter pick whichever free model is available.">
        <div className="flex flex-wrap items-center gap-2">
          <SelectInput value={known ? endpoint.openrouterModel : "custom"} onChange={(event) => event.target.value !== "custom" && setEndpoint((e) => ({ ...e, openrouterModel: event.target.value }))} className="max-w-96">
            <option value={OPENROUTER_FREE_ROUTER}>Free models router ({OPENROUTER_FREE_ROUTER})</option>
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name} · {Math.round(m.contextLength / 1000)}k{m.supportsTools ? " · tools" : ""}
              </option>
            ))}
            {!known ? <option value="custom">Custom: {endpoint.openrouterModel}</option> : null}
          </SelectInput>
          <Button variant="outline" onClick={() => void loadModels()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Load free models
          </Button>
        </div>
      </Field>
      <Field label="Model id" hint="Any OpenRouter model id; free ones end in :free.">
        <TextInput value={endpoint.openrouterModel} onChange={(e) => setEndpoint((x) => ({ ...x, openrouterModel: e.target.value }))} placeholder={OPENROUTER_FREE_ROUTER} className="max-w-md" />
      </Field>
      {loadError ? <Notice tone="danger" text={loadError} /> : null}
    </div>
  )
}

function OpenCodeSection({ endpoint, setEndpoint }: { endpoint: AiEndpoint; setEndpoint: (update: (e: AiEndpoint) => AiEndpoint) => void }) {
  const [status, setStatus] = useState<OpenCodeStatus | null>(null)
  const [models, setModels] = useState<string[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function check() {
    setLoading(true)
    setError(null)
    try {
      const next = await api.opencodeStatus()
      setStatus(next)
      if (next.installed) {
        const list = await api.opencodeModels()
        setModels(list)
        if (list.length === 0) setError("OpenCode lists no models. Run `opencode auth login` or `opencode models --refresh`.")
      }
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="space-y-3">
      <p className="text-sm text-muted-foreground">
        OpenCode&apos;s free models are only offered to the OpenCode app itself, so WinClean does not call them over the web. Instead it runs your installed <code>opencode</code> command in an empty folder, hands it the storage summary, and reads the answer. Sign in once with <code>opencode auth login</code>. Free models are for trial use: do not analyze confidential data with them.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="outline" onClick={() => void check()} disabled={loading}>
          {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
          Detect OpenCode and list models
        </Button>
        {status ? <span className="text-xs text-muted-foreground">{status.installed ? `Found${status.version ? ` ${status.version}` : ""}` : status.detail}</span> : null}
      </div>
      {models.length > 0 ? (
        <Field label="Model">
          <SelectInput value={models.includes(endpoint.opencodeModel) ? endpoint.opencodeModel : ""} onChange={(event) => setEndpoint((e) => ({ ...e, opencodeModel: event.target.value }))} className="max-w-96">
            <option value="">Choose a model…</option>
            {models.map((m) => (
              <option key={m} value={m}>
                {m}
                {looksFree(m) ? " (free)" : ""}
              </option>
            ))}
          </SelectInput>
        </Field>
      ) : null}
      <Field label="Model id" hint="provider/model, for example opencode/big-pickle.">
        <TextInput value={endpoint.opencodeModel} onChange={(e) => setEndpoint((x) => ({ ...x, opencodeModel: e.target.value }))} placeholder="opencode/big-pickle" className="max-w-md" />
      </Field>
      {error ? <Notice tone="danger" text={error} /> : null}
    </div>
  )
}

/**
 * Where the AI assistant runs: OpenRouter (free models), the user's OpenCode
 * client, OpenAI, or a local OpenAI-compatible server on this PC. API keys live
 * in Windows Credential Manager; local endpoints must be on loopback.
 */
export function AiSettingsPanel({ keys, onChanged }: { keys: AiKeyMap; onChanged: (provider: AiKeyProvider, configured: boolean) => void }) {
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

  return (
    <Panel title="AI assistant">
      <div className="space-y-4">
        <div role="radiogroup" aria-label="AI provider" className="inline-flex flex-wrap gap-1 rounded-lg border border-border bg-muted/50 p-1">
          {KINDS.map((k) => (
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

        {endpoint.kind === "openrouter" ? <OpenRouterSection endpoint={endpoint} setEndpoint={setEndpoint} keys={keys} onKeyChanged={onChanged} /> : null}
        {endpoint.kind === "opencode" ? <OpenCodeSection endpoint={endpoint} setEndpoint={setEndpoint} /> : null}

        {endpoint.kind === "openai" ? (
          <div className="space-y-3">
            <p className="text-sm text-muted-foreground">
              The key is stored in Windows Credential Manager. Storage analysis sends file metadata (paths, sizes, dates); system analysis sends service/process names, battery state and hardware model names. File contents are never sent.
            </p>
            <KeyEditor provider="openai" label="OpenAI" placeholder="sk-..." configured={keys.openai} onChanged={onChanged} />
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
        ) : null}

        {endpoint.kind === "local" ? (
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
          </div>
        ) : null}

        {problem ? <p className="text-xs text-destructive">{problem}</p> : null}
        <p className="text-xs text-muted-foreground">Active: {describeEndpoint(endpoint)}</p>
      </div>
    </Panel>
  )
}
