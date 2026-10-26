import { afterAll, beforeAll, describe, expect, test } from "bun:test"

import { DEFAULT_ENDPOINT, type AiEndpoint } from "./ai-config"
import { askModel } from "./ai-provider"

// The real fetch / parse / error-mapping code against a loopback server that answers
// the way OpenAI-compatible chat endpoints (OpenRouter included) do.
let server: ReturnType<typeof Bun.serve>
let last: { auth: string | null; body: { model: string; messages: { role: string; content: string }[]; temperature: number } } | null = null
let mode = "ok"

beforeAll(() => {
  server = Bun.serve({
    port: 0,
    hostname: "127.0.0.1",
    async fetch(request) {
      const url = new URL(request.url)
      if (url.pathname !== "/v1/chat/completions") return new Response("nope", { status: 404 })
      last = { auth: request.headers.get("authorization"), body: await request.json() }
      switch (mode) {
        case "rate":
          return Response.json({ error: { message: "Rate limit exceeded: free-models-per-day" } }, { status: 429 })
        case "inline-error":
          return Response.json({ error: { message: "Provider returned error" } })
        case "empty":
          return Response.json({ choices: [{ message: { content: "" } }] })
        case "parts":
          return Response.json({ choices: [{ message: { content: [{ type: "text", text: '{"summary":"a",' }, { type: "text", text: '"proposals":[]}' }] } }] })
        default:
          return Response.json({ choices: [{ message: { content: '```json\n{"summary":"ok","proposals":[]}\n```' } }] })
      }
    },
  })
})
afterAll(() => server.stop(true))

const endpoint = (): AiEndpoint => ({ ...DEFAULT_ENDPOINT, kind: "local", baseUrl: `http://127.0.0.1:${server.port}/v1`, localModel: "test-model" })
const ask = () => askModel({ endpoint: endpoint(), openaiModel: "unused", system: "SYS", user: "USER" })

describe("chat request path", () => {
  test("sends system + user messages to /chat/completions with the chosen model and returns the content", async () => {
    mode = "ok"
    const text = await ask()
    expect(text).toContain('"summary":"ok"')
    expect(last?.body.model).toBe("test-model")
    expect(last?.body.messages).toEqual([
      { role: "system", content: "SYS" },
      { role: "user", content: "USER" },
    ])
    expect(last?.auth).toBe("Bearer local")
  })

  test("accepts content returned as text parts", async () => {
    mode = "parts"
    expect(await ask()).toBe('{"summary":"a","proposals":[]}')
  })

  test("a 429 becomes the plain rate-limit message with the provider's reason", async () => {
    mode = "rate"
    await expect(ask()).rejects.toThrow(/rate limit reached.*free-models-per-day/s)
  })

  test("an error object inside a 200 response and an empty answer are both errors", async () => {
    mode = "inline-error"
    await expect(ask()).rejects.toThrow(/reported an error/)
    mode = "empty"
    await expect(ask()).rejects.toThrow(/empty answer/)
  })

  test("a non-loopback local address is refused before any request is made", async () => {
    mode = "ok"
    last = null
    await expect(askModel({ endpoint: { ...endpoint(), baseUrl: "http://example.com/v1" }, openaiModel: "x", system: "s", user: "u" })).rejects.toThrow(/this computer/)
    expect(last).toBeNull()
  })
})
