// AI endpoint configuration: OpenAI by default, or a local OpenAI-compatible
// server (Ollama, LM Studio, Lemonade, Foundry Local…) for fully local
// inference. Pure helpers + localStorage persistence; nothing here talks to
// the network.

export type AiEndpointKind = "openai" | "local"

export type AiEndpoint = {
  kind: AiEndpointKind
  /** Base URL of an OpenAI-compatible API (ignored for `openai`). */
  baseUrl: string
  /** Model id used with a local endpoint (OpenAI models come from the model picker). */
  localModel: string
}

export const OPENAI_BASE_URL = "https://api.openai.com/v1"

export const DEFAULT_ENDPOINT: AiEndpoint = { kind: "openai", baseUrl: "", localModel: "" }

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

export function validateEndpoint(endpoint: AiEndpoint): string | null {
  if (endpoint.kind === "openai") return null
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
    const kind: AiEndpointKind = parsed.kind === "local" ? "local" : "openai"
    return {
      kind,
      baseUrl: typeof parsed.baseUrl === "string" ? parsed.baseUrl : "",
      localModel: typeof parsed.localModel === "string" ? parsed.localModel : "",
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
  if (endpoint.kind === "openai") return "OpenAI (api.openai.com)"
  const check = validateLocalBaseUrl(endpoint.baseUrl)
  return check.ok ? `Local model at ${new URL(check.url).host}` : "Local model (not configured)"
}

/** Whether the assistant can run with the current endpoint; `reason` is shown to the user when not. */
export function aiAvailability(endpoint: AiEndpoint, keyConfigured: boolean): { ready: true } | { ready: false; reason: string } {
  if (endpoint.kind === "local") {
    const problem = validateEndpoint(endpoint)
    return problem ? { ready: false, reason: problem } : { ready: true }
  }
  return keyConfigured ? { ready: true } : { ready: false, reason: "Add your OpenAI API key in Settings, or switch to a local model." }
}
