// One-shot chat requests to the configured provider. The storage analysis sends a
// single request (a free OpenRouter account allows only a few dozen per day), so
// this is a plain fetch rather than an agent loop with tool calls.

import {
  OPENAI_BASE_URL,
  OPENROUTER_BASE_URL,
  keyProviderFor,
  modelFor,
  validateEndpoint,
  validateLocalBaseUrl,
  type AiEndpoint,
} from "@/lib/ai-config"
import { api } from "@/lib/api"

export type OpenRouterModel = {
  id: string
  name: string
  contextLength: number
  supportsTools: boolean
}

const REQUEST_TIMEOUT_MS = 150_000

/** Models OpenRouter lists as free: both prompt and completion price are zero. */
export function parseOpenRouterFreeModels(json: unknown): OpenRouterModel[] {
  const data = (json as { data?: unknown })?.data
  if (!Array.isArray(data)) return []
  const models: OpenRouterModel[] = []
  for (const raw of data) {
    const m = raw as {
      id?: unknown
      name?: unknown
      context_length?: unknown
      pricing?: { prompt?: unknown; completion?: unknown }
      supported_parameters?: unknown
      architecture?: { output_modalities?: unknown }
    }
    if (typeof m.id !== "string" || !/^[A-Za-z0-9][A-Za-z0-9._:/@+-]{0,199}$/.test(m.id)) continue
    const free = Number(m.pricing?.prompt) === 0 && Number(m.pricing?.completion) === 0 && m.pricing?.prompt !== undefined
    if (!free) continue
    const outputs = m.architecture?.output_modalities
    if (Array.isArray(outputs) && !outputs.includes("text")) continue
    models.push({
      id: m.id,
      name: typeof m.name === "string" ? m.name.slice(0, 120) : m.id,
      contextLength: typeof m.context_length === "number" ? m.context_length : 0,
      supportsTools: Array.isArray(m.supported_parameters) && m.supported_parameters.includes("tools"),
    })
  }
  // Largest context first: the storage brief is long.
  return models.sort((a, b) => b.contextLength - a.contextLength || a.id.localeCompare(b.id))
}

export async function fetchOpenRouterFreeModels(apiKey: string | null, signal?: AbortSignal): Promise<OpenRouterModel[]> {
  const response = await fetch(`${OPENROUTER_BASE_URL}/models`, {
    headers: apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
    signal,
  })
  if (!response.ok) throw new Error(`OpenRouter returned HTTP ${response.status} while listing models.`)
  return parseOpenRouterFreeModels(await response.json())
}

/** Plain-language message for a failed provider request. */
export function describeProviderError(status: number, providerMessage: string | null, provider: string): string {
  const detail = providerMessage ? ` (${providerMessage.slice(0, 200)})` : ""
  switch (status) {
    case 400:
      return `${provider} rejected the request${detail}. Try a different model.`
    case 401:
    case 403:
      return `${provider} did not accept the API key${detail}. Check it in Settings.`
    case 402:
      return `${provider} needs credits for this model${detail}. Choose a free model or add credits.`
    case 404:
      return `${provider} does not offer that model right now${detail}. Pick another one in Settings.`
    case 408:
    case 504:
      return `${provider} timed out${detail}. Try again or choose a faster model.`
    case 429:
      return `${provider} rate limit reached${detail}. Free models allow only a limited number of requests per day; wait or pick another model.`
    default:
      return `${provider} returned an error (HTTP ${status})${detail}.`
  }
}

function baseUrlFor(endpoint: AiEndpoint): string {
  switch (endpoint.kind) {
    case "openai":
      return OPENAI_BASE_URL
    case "openrouter":
      return OPENROUTER_BASE_URL
    default: {
      const checked = validateLocalBaseUrl(endpoint.baseUrl)
      if (!checked.ok) throw new Error(checked.error)
      return checked.url
    }
  }
}

function contentOf(body: unknown): string {
  const b = body as { choices?: { message?: { content?: unknown } }[]; error?: { message?: unknown } }
  if (b?.error?.message) throw new Error(`The provider reported an error: ${String(b.error.message).slice(0, 200)}`)
  const content = b?.choices?.[0]?.message?.content
  if (typeof content === "string" && content.trim()) return content
  if (Array.isArray(content)) {
    const text = content.map((part) => (typeof part?.text === "string" ? part.text : "")).join("")
    if (text.trim()) return text
  }
  throw new Error("The model returned an empty answer. Try again or choose another model.")
}

const OPENCODE_INSTRUCTION = "Read the input. Reply with the JSON object it asks for and nothing else. Do not use tools."

/** Ask the selected provider one question and return the raw text of its answer. */
export async function askModel({
  endpoint,
  openaiModel,
  system,
  user,
}: {
  endpoint: AiEndpoint
  openaiModel: string
  system: string
  user: string
}): Promise<string> {
  const problem = validateEndpoint(endpoint)
  if (problem) throw new Error(problem)
  const model = modelFor(endpoint, openaiModel)

  if (endpoint.kind === "opencode") {
    return api.opencodeRun(model, OPENCODE_INSTRUCTION, `${system}\n\n${user}`)
  }

  const keyProvider = keyProviderFor(endpoint.kind)
  let apiKey = "local"
  if (keyProvider) {
    const stored = await api.getAiApiKey(keyProvider)
    if (!stored) throw new Error(`Add your ${keyProvider === "openai" ? "OpenAI" : "OpenRouter"} API key in Settings first.`)
    apiKey = stored
  }
  const provider = endpoint.kind === "openrouter" ? "OpenRouter" : endpoint.kind === "openai" ? "OpenAI" : "The local model"
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS)
  try {
    const response = await fetch(`${baseUrlFor(endpoint)}/chat/completions`, {
      method: "POST",
      headers: { Authorization: `Bearer ${apiKey}`, "Content-Type": "application/json" },
      body: JSON.stringify({
        model,
        temperature: 0.2,
        messages: [
          { role: "system", content: system },
          { role: "user", content: user },
        ],
      }),
      signal: controller.signal,
    })
    if (!response.ok) {
      let message: string | null = null
      try {
        message = ((await response.json()) as { error?: { message?: string } })?.error?.message ?? null
      } catch {
        // The error body is optional.
      }
      throw new Error(describeProviderError(response.status, message, provider))
    }
    return contentOf(await response.json())
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") {
      throw new Error(`${provider} did not answer within ${REQUEST_TIMEOUT_MS / 1000} seconds.`)
    }
    throw error
  } finally {
    clearTimeout(timer)
  }
}
