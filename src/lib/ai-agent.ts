import { Agent, run, setDefaultOpenAIClient, setOpenAIAPI, setTracingDisabled } from "@openai/agents"
import OpenAI from "openai"

import { OPENAI_BASE_URL, OPENROUTER_BASE_URL, modelFor, validateEndpoint, validateLocalBaseUrl, type AiEndpoint } from "@/lib/ai-config"
import { buildOptimizerTools } from "@/lib/ai-tools"
import { systemReportSchema, type SystemReport } from "@/lib/ai-proposals"

/** Point the Agents SDK at OpenAI, OpenRouter or a validated loopback OpenAI-compatible server. */
function configureClient(apiKey: string, endpoint: AiEndpoint) {
  const problem = validateEndpoint(endpoint)
  if (problem) {
    throw new Error(problem)
  }
  if (endpoint.kind === "opencode") {
    throw new Error("System analysis needs a provider with tool calling. Choose OpenRouter, OpenAI or a local model in Settings.")
  }
  setTracingDisabled(true)
  setOpenAIAPI("chat_completions")
  if (endpoint.kind === "local") {
    const checked = validateLocalBaseUrl(endpoint.baseUrl)
    if (!checked.ok) {
      throw new Error(checked.error)
    }
    // A local server needs no key; the SDK still wants a non-empty string.
    setDefaultOpenAIClient(new OpenAI({ apiKey: "local", baseURL: checked.url, dangerouslyAllowBrowser: true }))
  } else {
    const baseURL = endpoint.kind === "openrouter" ? OPENROUTER_BASE_URL : OPENAI_BASE_URL
    setDefaultOpenAIClient(new OpenAI({ apiKey, baseURL, dangerouslyAllowBrowser: true }))
  }
}

const SYSTEM_INSTRUCTIONS = `You are WinClean's Windows 11 optimization analyst for an ASUS ROG Zephyrus G16
(AMD Ryzen AI 9 HX 370, Radeon 890M, RTX 4070, AMD XDNA 2 NPU). You analyse services, processes,
battery consumption, hardware activity and optimization opportunities using READ-ONLY tools.

Process:
1. Call get_workload, get_battery, get_hardware and get_profiles to understand the situation.
2. Call get_service_recommendations first; call get_services and get_processes for evidence.
3. Call get_capabilities only if local AI/NPU questions matter. Call get_optimization_history to avoid repeating work.
4. Return a short summary, concrete findings backed by tool evidence, and optional proposals.

Hard rules:
- You can only PROPOSE. You cannot change anything. Every proposal is validated by deterministic code and the user must approve it explicitly before anything happens.
- serviceProposals may only name services returned by get_services whose risk is NOT 'protected'. Prefer the exact changes from get_service_recommendations. Never propose disabling a service unless its canDisable is true.
- Never propose anything for ASUS, AMD, NVIDIA, Windows Update, Defender, Windows Search, networking, audio or Copilot+ components.
- profileProposal must use an id from get_profiles. Do not propose a profile that fights the current workload (for example Battery Saver while gaming).
- Battery figures can be null; say so instead of estimating. Never invent numbers.
- The RTX 4070 may be powered down. Do not recommend anything that would wake it.
- Be concise and avoid technical jargon in summaries.`

export async function analyzeSystemWithAi({
  apiKey,
  model,
  endpoint,
}: {
  apiKey: string
  model: string
  endpoint: AiEndpoint
}): Promise<SystemReport> {
  configureClient(apiKey, endpoint)

  const agent = new Agent({
    name: "WinClean optimization analyst",
    model: modelFor(endpoint, model),
    instructions: SYSTEM_INSTRUCTIONS,
    tools: buildOptimizerTools(),
    outputType: systemReportSchema,
  })

  const result = await run(agent, "Analyze this PC's efficiency and propose safe optimizations, if any are justified by the data.")
  const report = result.finalOutput
  if (!report) {
    throw new Error("The AI assistant did not return a report. Try again.")
  }
  return report
}
