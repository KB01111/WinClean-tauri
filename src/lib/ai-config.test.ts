import { describe, expect, test } from "bun:test"

import { aiAvailability, keyProviderFor, modelFor, sendsDataOffDevice, DEFAULT_ENDPOINT, describeEndpoint, loadEndpoint, saveEndpoint, validateEndpoint, validateLocalBaseUrl, LOCAL_PRESETS } from "./ai-config"

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
    expect(validateEndpoint({ ...DEFAULT_ENDPOINT, kind: "local", baseUrl: "http://127.0.0.1:11434/v1", localModel: "" })).toContain("model")
    expect(validateEndpoint({ ...DEFAULT_ENDPOINT, kind: "local", baseUrl: "http://10.0.0.5:1/v1", localModel: "m" })).toContain("computer")
    expect(validateEndpoint({ ...DEFAULT_ENDPOINT, kind: "local", baseUrl: "http://127.0.0.1:11434/v1", localModel: "llama3" })).toBeNull()
  })
})

describe("persistence", () => {
  test("round-trips and survives corrupt or missing storage", () => {
    const store: Record<string, string> = {}
    const storage = { getItem: (k: string) => store[k] ?? null, setItem: (k: string, v: string) => void (store[k] = v) }
    expect(loadEndpoint(storage)).toEqual(DEFAULT_ENDPOINT)
    saveEndpoint({ ...DEFAULT_ENDPOINT, kind: "local", baseUrl: "http://127.0.0.1:1234/v1", localModel: "m" }, storage)
    expect(loadEndpoint(storage).kind).toBe("local")
    store["winclean-ai-endpoint"] = "{broken"
    expect(loadEndpoint(storage)).toEqual(DEFAULT_ENDPOINT)
    store["winclean-ai-endpoint"] = JSON.stringify({ kind: "weird", baseUrl: 5 })
    expect(loadEndpoint(storage)).toEqual(DEFAULT_ENDPOINT)
    const throwing = { getItem: () => { throw new Error("blocked") }, setItem: () => { throw new Error("blocked") } }
    expect(loadEndpoint(throwing)).toEqual(DEFAULT_ENDPOINT)
    expect(() => saveEndpoint(DEFAULT_ENDPOINT, throwing)).not.toThrow()
  })

  test("description names where requests go", () => {
    expect(describeEndpoint(DEFAULT_ENDPOINT)).toContain("api.openai.com")
    expect(describeEndpoint({ ...DEFAULT_ENDPOINT, kind: "local", baseUrl: "http://127.0.0.1:11434/v1", localModel: "m" })).toBe("Local model at 127.0.0.1:11434")
    expect(describeEndpoint({ ...DEFAULT_ENDPOINT, kind: "local" })).toContain("not configured")
  })
})

describe("assistant availability", () => {
  test("OpenAI needs a key; a valid local endpoint does not", () => {
    expect(aiAvailability(DEFAULT_ENDPOINT, {})).toMatchObject({ ready: false })
    expect(aiAvailability(DEFAULT_ENDPOINT, { openai: true })).toEqual({ ready: true })
    const local = { ...DEFAULT_ENDPOINT, kind: "local" as const, baseUrl: "http://127.0.0.1:11434/v1", localModel: "llama3" }
    expect(aiAvailability(local, {})).toEqual({ ready: true })
    expect(aiAvailability({ ...local, localModel: "" }, { openai: true })).toMatchObject({ ready: false })
    expect(aiAvailability({ ...local, baseUrl: "http://example.com/v1" }, { openai: true })).toMatchObject({ ready: false })
  })
})

describe("cloud providers", () => {
  const openrouter = { ...DEFAULT_ENDPOINT, kind: "openrouter" as const }
  const opencode = { ...DEFAULT_ENDPOINT, kind: "opencode" as const }

  test("OpenRouter defaults to the free router and needs its own key", () => {
    expect(openrouter.openrouterModel).toBe("openrouter/free")
    expect(validateEndpoint(openrouter)).toBeNull()
    expect(aiAvailability(openrouter, { openai: true })).toMatchObject({ ready: false })
    expect(aiAvailability(openrouter, { openrouter: true })).toEqual({ ready: true })
    expect(validateEndpoint({ ...openrouter, openrouterModel: "bad model; rm" })).toContain("OpenRouter")
    expect(validateEndpoint({ ...openrouter, openrouterModel: "meta-llama/llama-3.3-70b-instruct:free" })).toBeNull()
  })

  test("OpenCode needs a provider/model id and no stored key", () => {
    expect(validateEndpoint(opencode)).toContain("OpenCode")
    for (const bad of ["big-pickle", "opencode/", "opencode/--auto", "a/../b", "opencode/a b"]) {
      expect(validateEndpoint({ ...opencode, opencodeModel: bad })).not.toBeNull()
    }
    const ok = { ...opencode, opencodeModel: "opencode/big-pickle" }
    expect(validateEndpoint(ok)).toBeNull()
    expect(aiAvailability(ok, {})).toEqual({ ready: true })
    expect(keyProviderFor("opencode")).toBeNull()
    expect(keyProviderFor("openrouter")).toBe("openrouter")
  })

  test("model selection follows the provider and only local keeps data on device", () => {
    expect(modelFor({ ...openrouter, openrouterModel: "x/y:free" }, "gpt-5")).toBe("x/y:free")
    expect(modelFor(DEFAULT_ENDPOINT, "gpt-5")).toBe("gpt-5")
    expect(sendsDataOffDevice({ ...DEFAULT_ENDPOINT, kind: "local" })).toBe(false)
    expect(sendsDataOffDevice(openrouter)).toBe(true)
  })

  test("older saved settings without the new fields still load", () => {
    const store: Record<string, string> = { "winclean-ai-endpoint": JSON.stringify({ kind: "openrouter", baseUrl: "", localModel: "" }) }
    const storage = { getItem: (k: string) => store[k] ?? null }
    expect(loadEndpoint(storage)).toEqual({ ...DEFAULT_ENDPOINT, kind: "openrouter" })
  })
})
