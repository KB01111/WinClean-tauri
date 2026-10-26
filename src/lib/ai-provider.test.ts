import { describe, expect, test } from "bun:test"

import { describeProviderError, parseOpenRouterFreeModels } from "./ai-provider"

describe("OpenRouter free model list", () => {
  const payload = {
    data: [
      { id: "meta-llama/llama-3.3-70b-instruct:free", name: "Llama 3.3 70B", context_length: 65536, pricing: { prompt: "0", completion: "0" }, supported_parameters: ["tools", "temperature"] },
      { id: "paid/model", name: "Paid", context_length: 200000, pricing: { prompt: "0.000003", completion: "0.000015" } },
      { id: "google/gemma:free", name: "Gemma", context_length: 8192, pricing: { prompt: "0", completion: "0" }, supported_parameters: [] },
      { id: "image/only:free", name: "Img", context_length: 1000, pricing: { prompt: "0", completion: "0" }, architecture: { output_modalities: ["image"] } },
      { id: "bad id; rm", pricing: { prompt: "0", completion: "0" } },
      { id: "no-pricing" },
    ],
  }

  test("keeps only text models priced at zero, largest context first", () => {
    const models = parseOpenRouterFreeModels(payload)
    expect(models.map((m) => m.id)).toEqual(["meta-llama/llama-3.3-70b-instruct:free", "google/gemma:free"])
    expect(models[0].supportsTools).toBe(true)
    expect(models[1].supportsTools).toBe(false)
  })

  test("tolerates garbage", () => {
    for (const bad of [null, {}, { data: "x" }, []]) expect(parseOpenRouterFreeModels(bad)).toEqual([])
  })
})

describe("provider errors", () => {
  test("map status codes to plain advice", () => {
    expect(describeProviderError(401, null, "OpenRouter")).toContain("API key")
    expect(describeProviderError(402, null, "OpenRouter")).toContain("credits")
    expect(describeProviderError(429, "slow down", "OpenRouter")).toContain("limited number of requests")
    expect(describeProviderError(404, null, "OpenRouter")).toContain("another one")
    expect(describeProviderError(500, null, "OpenAI")).toContain("HTTP 500")
  })
})
