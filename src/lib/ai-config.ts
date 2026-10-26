// AI endpoint configuration. Four providers:
//  - openai:     OpenAI with the user's API key
//  - openrouter: OpenRouter, including its free models, with the user's API key
//  - opencode:   the user's own `opencode` CLI (OpenCode's free models are only
//                offered to the OpenCode client, so WinClean drives that client)
//  - local:      an OpenAI-compatible server on this PC (Ollama, LM Studio, …)
// Pure helpers + localStorage persistence; nothing here talks to the network.

import type { AiKeyMap, AiKeyProvider } from "@/lib/api"

export type AiEndpointKind = "openai" | "openrouter" | "opencode" | "local"

export type AiEndpoint = {
  kind: AiEndpointKind
  /** Base URL of an OpenAI-compatible API (local endpoints only). */
  baseUrl: string
  /** Model id used with a local endpoint (OpenAI models come from the model picker). */
  localModel: string
  /** OpenRouter model id, e.g. `openrouter/free` or `meta-llama/llama-3.3-70b-instruct:free`. */
  openrouterModel: string
  /** OpenCode model as `provider/model`, e.g. `opencode/big-pickle`. */
  opencodeModel: string
}

export const OPENAI_BASE_URL = "https://api.openai.com/v1"
export const OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1"
/** OpenRouter's router that picks a currently available free model. */
export const OPENROUTER_FREE_ROUTER = "openrouter/free"

export const DEFAULT_ENDPOINT: AiEndpoint = {
  kind: "openai",
  baseUrl: "",
  localModel: "",
  openrouterModel: OPENROUTER_FREE_ROUTER,
  opencodeModel: "",
}

export const PROVIDER_LABELS: Record<AiEndpointKind, string> = {
  openai: "OpenAI",
  openrouter: "OpenRouter",
  opencode: "OpenCode",
  local: "Local model",
}

/** The stored API key a provider needs, or null when it signs in elsewhere (or needs none). */
export function keyProviderFor(kind: AiEndpointKind): AiKeyProvider | null {
  return kind === "openai" || kind === "openrouter" ? kind : null
}

/** Model identifier sent for the selected provider (OpenAI's comes from the model picker). */
export function modelFor(endpoint: AiEndpoint, openaiModel: string): string {
  // Validation trims, so the value that is sent must be trimmed too.
  switch (endpoint.kind) {
    case "local":
      return endpoint.localModel.trim()
    case "openrouter":
      return endpoint.openrouterModel.trim()
    case "opencode":
      return endpoint.opencodeModel.trim()
    default:
      return openaiModel
  }
}

export type LocalPreset = { id: string; label: string; baseUrl: string; hint: string }

/** Default ports of popular local servers. Foundry Local picks its port at runtime. */
export const LOCAL_PRESETS: LocalPreset[] = [
  { id: "ollama", label: "Ollama", baseUrl: "http://127.0.0.1:11434/v1", hint: "ollama serve" },
  { id: "lmstudio", label: "LM Studio", baseUrl: "http://127.0.0.1:1234/v1", hint: "Start the local server in LM Studio" },
  { id: "lemonade", label: "Lemonade Server (AMD NPU/GPU)", baseUrl: "http://127.0.0.1:8000/api/v1", hint: "AMD's local server for Ryzen AI" },
  { id: "foundry", label: "Foundry Local", baseUrl: "http://127.0.0.1:5272/v1", hint: "Run `foundry service status` for the actual port" },
]

const LOOPBACK_HOSTS = new Set(["127.0.0.1", "localhost", "[::1]", "::1"])

/**
 * A local endpoint must be plain http(s) on the loopback interface. The app's
 * content-security policy only allows these hosts (plus api.openai.com), so
 * validating here gives a clear message instead of a blocked request.
 */
export function validateLocalBaseUrl(value: string): { ok: true; url: string } | { ok: false; error: string } {
  const trimmed = value.trim()
  if (!trimmed) return { ok: false, error: "Enter the server address, for example http://127.0.0.1:11434/v1." }
  let url: URL
  try {
    url = new URL(trimmed)
  } catch {
    return { ok: false, error: "That is not a valid URL." }
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return { ok: false, error: "Use an http:// or https:// address." }
  if (!LOOPBACK_HOSTS.has(url.hostname)) {
    return { ok: false, error: "Only servers on this computer (127.0.0.1 or localhost) are allowed. File metadata never leaves the machine for a local endpoint." }
  }
  if (url.username || url.password) return { ok: false, error: "Do not put credentials in the URL." }
  url.hash = ""
  url.search = ""
  return { ok: true, url: url.toString().replace(/\/$/, "") }
}

const MODEL_ID = /^[A-Za-z0-9][A-Za-z0-9._:/@+-]{0,199}$/

export function validateEndpoint(endpoint: AiEndpoint): string | null {
  if (endpoint.kind === "openai") return null
  if (endpoint.kind === "openrouter") {
    return MODEL_ID.test(endpoint.openrouterModel.trim()) ? null : "Choose an OpenRouter model, for example openrouter/free."
  }
  if (endpoint.kind === "opencode") {
    const model = endpoint.opencodeModel.trim()
    return /^[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._:/-]*$/.test(model) && !model.includes("..")
      ? null
      : "Choose an OpenCode model, for example opencode/big-pickle."
  }
  const check = validateLocalBaseUrl(endpoint.baseUrl)
  if (!check.ok) return check.error
  if (!endpoint.localModel.trim()) return "Enter the model name your local server should use."
  return null
}

const STORAGE_KEY = "winclean-ai-endpoint"

export function loadEndpoint(storage: Pick<Storage, "getItem"> = localStorage): AiEndpoint {
  try {
    const raw = storage.getItem(STORAGE_KEY)
    if (!raw) return DEFAULT_ENDPOINT
    const parsed = JSON.parse(raw) as Partial<AiEndpoint>
    const kind: AiEndpointKind =
      parsed.kind === "local" || parsed.kind === "openrouter" || parsed.kind === "opencode" ? parsed.kind : "openai"
    const text = (value: unknown, fallback: string) => (typeof value === "string" ? value : fallback)
    return {
      kind,
      baseUrl: text(parsed.baseUrl, ""),
      localModel: text(parsed.localModel, ""),
      openrouterModel: text(parsed.openrouterModel, DEFAULT_ENDPOINT.openrouterModel),
      opencodeModel: text(parsed.opencodeModel, ""),
    }
  } catch {
    return DEFAULT_ENDPOINT
  }
}

export function saveEndpoint(endpoint: AiEndpoint, storage: Pick<Storage, "setItem"> = localStorage) {
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(endpoint))
  } catch {
    // Storage can be unavailable; the choice then lasts for this session only.
  }
}

/** Where requests go, for display ("api.openai.com" or "127.0.0.1:11434"). */
export function describeEndpoint(endpoint: AiEndpoint): string {
  switch (endpoint.kind) {
    case "openai":
      return "OpenAI (api.openai.com)"
    case "openrouter":
      return `OpenRouter (openrouter.ai) · ${endpoint.openrouterModel || "no model chosen"}`
    case "opencode":
      return `OpenCode on this PC · ${endpoint.opencodeModel || "no model chosen"}`
    default: {
      const check = validateLocalBaseUrl(endpoint.baseUrl)
      return check.ok ? `Local model at ${new URL(check.url).host}` : "Local model (not configured)"
    }
  }
}

/** Whether data leaves this PC for the selected provider (OpenCode's own client decides for itself). */
export function sendsDataOffDevice(endpoint: AiEndpoint): boolean {
  return endpoint.kind !== "local"
}

/** Where the assistant's data goes, for disclosure text ("OpenRouter", "OpenAI", "the OpenCode client", …). */
export function destinationLabel(endpoint: AiEndpoint): string {
  switch (endpoint.kind) {
    case "openai":
      return "OpenAI"
    case "openrouter":
      return "OpenRouter (and the model provider behind the model you chose)"
    case "opencode":
      return "your OpenCode client (and the model provider it uses)"
    default:
      return "your local model"
  }
}

/**
 * Whether the assistant can run with the current endpoint; `reason` is shown to the user when not.
 * The system analysis needs tool calling, which the OpenCode bridge does not offer.
 */
export function aiAvailability(endpoint: AiEndpoint, keys: Partial<AiKeyMap>, feature: "storage" | "system" = "storage"): { ready: true } | { ready: false; reason: string } {
  const problem = validateEndpoint(endpoint)
  if (problem) return { ready: false, reason: problem }
  if (feature === "system" && endpoint.kind === "opencode") {
    return { ready: false, reason: "System analysis needs a provider with tool calling. Choose OpenRouter, OpenAI or a local model in Settings." }
  }
  switch (endpoint.kind) {
    case "openai":
      return keys.openai ? { ready: true } : { ready: false, reason: "Add your OpenAI API key in Settings, or switch to another provider." }
    case "openrouter":
      return keys.openrouter
        ? { ready: true }
        : { ready: false, reason: "Add your OpenRouter API key in Settings (free models need a free account key)." }
    default:
      return { ready: true }
  }
}
