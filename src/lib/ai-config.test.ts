import { describe, expect, test } from "bun:test"

import { aiAvailability, DEFAULT_ENDPOINT, describeEndpoint, loadEndpoint, saveEndpoint, validateEndpoint, validateLocalBaseUrl, LOCAL_PRESETS } from "./ai-config"

describe("local endpoint validation", () => {
  test("loopback http(s) addresses are accepted and normalised", () => {
    for (const url of ["http://127.0.0.1:11434/v1", "http://localhost:1234/v1/", "https://localhost:8443", "http://[::1]:8000/api/v1"]) {
      const r = validateLocalBaseUrl(url)
      expect(r.ok).toBe(true)
    }
    const r = validateLocalBaseUrl("  http://localhost:1234/v1/?x=1#f ")
    expect(r.ok && r.url).toBe("http://localhost:1234/v1")
  })

  test("remote hosts, other schemes and credentials are rejected", () => {
    for (const url of ["", "not a url", "ftp://127.0.0.1", "http://192.168.1.10:11434", "https://example.com/v1", "http://user:pw@127.0.0.1:1", "http://127.0.0.1.evil.com", "file:///C:/x"]) {
      expect(validateLocalBaseUrl(url).ok).toBe(false)
    }
  })

  test("every shipped preset is itself valid", () => {
    for (const p of LOCAL_PRESETS) expect(validateLocalBaseUrl(p.baseUrl).ok).toBe(true)
  })

  test("a local endpoint needs a model name; OpenAI needs nothing extra", () => {
    expect(validateEndpoint(DEFAULT_ENDPOINT)).toBeNull()
    expect(validateEndpoint({ kind: "local", baseUrl: "http://127.0.0.1:11434/v1", localModel: "" })).toContain("model")
    expect(validateEndpoint({ kind: "local", baseUrl: "http://10.0.0.5:1/v1", localModel: "m" })).toContain("computer")
    expect(validateEndpoint({ kind: "local", baseUrl: "http://127.0.0.1:11434/v1", localModel: "llama3" })).toBeNull()
  })
})

describe("persistence", () => {
  test("round-trips and survives corrupt or missing storage", () => {
    const store: Record<string, string> = {}
    const storage = { getItem: (k: string) => store[k] ?? null, setItem: (k: string, v: string) => void (store[k] = v) }
    expect(loadEndpoint(storage)).toEqual(DEFAULT_ENDPOINT)
    saveEndpoint({ kind: "local", baseUrl: "http://127.0.0.1:1234/v1", localModel: "m" }, storage)
    expect(loadEndpoint(storage).kind).toBe("local")
    store["winclean-ai-endpoint"] = "{broken"
    expect(loadEndpoint(storage)).toEqual(DEFAULT_ENDPOINT)
    store["winclean-ai-endpoint"] = JSON.stringify({ kind: "weird", baseUrl: 5 })
    expect(loadEndpoint(storage)).toEqual({ kind: "openai", baseUrl: "", localModel: "" })
    const throwing = { getItem: () => { throw new Error("blocked") }, setItem: () => { throw new Error("blocked") } }
    expect(loadEndpoint(throwing)).toEqual(DEFAULT_ENDPOINT)
    expect(() => saveEndpoint(DEFAULT_ENDPOINT, throwing)).not.toThrow()
  })

  test("description names where requests go", () => {
    expect(describeEndpoint(DEFAULT_ENDPOINT)).toContain("api.openai.com")
    expect(describeEndpoint({ kind: "local", baseUrl: "http://127.0.0.1:11434/v1", localModel: "m" })).toBe("Local model at 127.0.0.1:11434")
    expect(describeEndpoint({ kind: "local", baseUrl: "", localModel: "" })).toContain("not configured")
  })
})

describe("assistant availability", () => {
  test("OpenAI needs a key; a valid local endpoint does not", () => {
    expect(aiAvailability(DEFAULT_ENDPOINT, false)).toMatchObject({ ready: false })
    expect(aiAvailability(DEFAULT_ENDPOINT, true)).toEqual({ ready: true })
    const local = { kind: "local" as const, baseUrl: "http://127.0.0.1:11434/v1", localModel: "llama3" }
    expect(aiAvailability(local, false)).toEqual({ ready: true })
    expect(aiAvailability({ ...local, localModel: "" }, true)).toMatchObject({ ready: false })
    expect(aiAvailability({ ...local, baseUrl: "http://example.com/v1" }, true)).toMatchObject({ ready: false })
  })
})
