import {
  Agent,
  run,
  setDefaultOpenAIClient,
  setOpenAIAPI,
  setTracingDisabled,
  tool,
} from "@openai/agents"
import OpenAI from "openai"
import { z } from "zod"

import { api } from "@/lib/api"
import { OPENAI_BASE_URL, validateEndpoint, validateLocalBaseUrl, type AiEndpoint } from "@/lib/ai-config"
import { buildOptimizerTools } from "@/lib/ai-tools"
import { systemReportSchema, type SystemReport } from "@/lib/ai-proposals"

const suggestionSchema = z.object({
  title: z.string().describe("Short, friendly title for the suggestion"),
  action: z
    .enum(["clean", "offload", "review"])
    .describe("Which recover-space mode this suggestion belongs to"),
  candidateIds: z
    .array(z.string())
    .describe(
      "Candidate ids from the get_recommendations tool that this suggestion covers"
    ),
  estimatedBytes: z
    .number()
    .describe("Total bytes that would be recovered by this suggestion"),
  reason: z
    .string()
    .describe("One or two sentences explaining why this is safe and useful"),
})

const reportSchema = z.object({
  summary: z
    .string()
    .describe(
      "A short, friendly overview of the disk situation and biggest wins"
    ),
  suggestions: z.array(suggestionSchema),
})

export type AiSuggestion = z.infer<typeof suggestionSchema>
export type AiReport = z.infer<typeof reportSchema>

function toMb(bytes: number) {
  return Math.round((bytes / (1024 * 1024)) * 10) / 10
}

/** Build scan-scoped, read-only storage tools for the AI storage analyst. */
function buildTools(runId: number) {
  const getScanSummary = tool({
    name: "get_scan_summary",
    description:
      "Returns overall scan metrics, file categories, and top extensions for the selected scan run.",
    parameters: z.object({}),
    execute: async () => {
      const [runs, categories, extensions] = await Promise.all([
        api.scanRuns(500),
        api.categorySummary(runId),
        api.extensionSummary(runId, 15),
      ])
      const matchedRun = runs.find((record) => record.id === runId)
      return JSON.stringify({
        run: matchedRun
          ? {
              rootPath: matchedRun.rootPath,
              filesIndexed: matchedRun.filesIndexed,
              directoriesIndexed: matchedRun.directoriesIndexed,
              totalMb: toMb(matchedRun.bytesIndexed),
              finishedAtMs: matchedRun.finishedAtMs,
            }
          : null,
        categories: categories.map((category) => ({
          category: category.category,
          files: category.fileCount,
          totalMb: toMb(category.totalSize),
        })),
        topExtensions: extensions.map((extension) => ({
          extension: extension.extension ?? "none",
          files: extension.fileCount,
          totalMb: toMb(extension.totalSize),
        })),
      })
    },
  })

  const getTopDirectories = tool({
    name: "get_top_directories",
    description: "Returns the largest directories by total size.",
    parameters: z.object({
      limit: z.number().describe("How many directories to return, max 25"),
    }),
    execute: async ({ limit }) => {
      const directories = await api.topDirectories(
        runId,
        Math.min(Math.max(1, Math.round(limit)), 25)
      )
      return JSON.stringify(
        directories.map((directory) => ({
          path: directory.path,
          totalMb: toMb(directory.totalSize),
          files: directory.fileCount,
        }))
      )
    },
  })

  const getLargestFiles = tool({
    name: "get_largest_files",
    description: "Returns the largest individual files on the scanned volume.",
    parameters: z.object({
      limit: z.number().describe("How many files to return, max 50"),
    }),
    execute: async ({ limit }) => {
      const files = await api.fileSearch({
        runId,
        limit: Math.min(Math.max(1, Math.round(limit)), 50),
      })
      return JSON.stringify(
        files.map((file) => ({
          path: file.path,
          sizeMb: toMb(file.size),
          modifiedAtMs: file.modifiedAtMs,
        }))
      )
    },
  })

  const getStaleLargeFiles = tool({
    name: "get_stale_large_files",
    description:
      "Returns large files that have not been modified for a long time (storage hogs).",
    parameters: z.object({
      minSizeMb: z.number().describe("Minimum file size in megabytes"),
      minAgeDays: z.number().describe("Minimum days since last modification"),
    }),
    execute: async ({ minSizeMb, minAgeDays }) => {
      const files = await api.largestStaleFiles({
        runId,
        minSize: Math.max(1, Math.round(minSizeMb)) * 1024 * 1024,
        minAgeDays: Math.max(1, Math.round(minAgeDays)),
        limit: 40,
      })
      return JSON.stringify(
        files.map((file) => ({
          path: file.path,
          sizeMb: toMb(file.size),
          modifiedAtMs: file.modifiedAtMs,
        }))
      )
    },
  })

  const getRecommendations = tool({
    name: "get_recommendations",
    description:
      "Returns WinClean's actionable candidates with their ids. Suggestions MUST reference these candidate ids. kind 'clean' candidates can be recycled, 'offload' candidates can be moved to Google Drive, 'review' candidates need the user's judgment.",
    parameters: z.object({}),
    execute: async () => {
      const plan = await api.recommendations(runId)
      if (!plan) {
        return JSON.stringify({ candidates: [] })
      }
      return JSON.stringify({
        cleanMb: toMb(plan.cleanBytes),
        offloadMb: toMb(plan.offloadBytes),
        reviewMb: toMb(plan.reviewBytes),
        candidates: plan.candidates.slice(0, 250).map((candidate) => ({
          id: candidate.id,
          path: candidate.path,
          sizeBytes: candidate.size,
          kind: candidate.kind,
          reason: candidate.reason,
          ageDays: candidate.ageDays,
        })),
      })
    },
  })

  return [getScanSummary, getTopDirectories, getLargestFiles, getStaleLargeFiles, getRecommendations]
}

const INSTRUCTIONS = `You are WinClean's storage analyst. You help Windows users understand what is
using their disk space and what they can safely clean up or offload to Google Drive.

Process:
1. Call get_scan_summary and get_top_directories to understand the disk.
2. Call get_recommendations to see the actionable candidates and their ids.
3. Optionally call get_largest_files / get_stale_large_files for extra context.
4. Produce a short, friendly summary and 2-6 concrete suggestions.

Rules:
- Suggestions must only reference candidate ids returned by get_recommendations.
- Group related candidates into one suggestion (e.g. "Old installers in Downloads").
- estimatedBytes must be the sum of sizeBytes for the included candidates.
- Prefer high-impact, low-risk wins first. Never invent files or ids.
- Be concise and avoid technical jargon.`

/** Point the Agents SDK at OpenAI or at a validated loopback OpenAI-compatible server. */
function configureClient(apiKey: string, endpoint: AiEndpoint) {
  const problem = validateEndpoint(endpoint)
  if (problem) {
    throw new Error(problem)
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
    setDefaultOpenAIClient(new OpenAI({ apiKey, baseURL: OPENAI_BASE_URL, dangerouslyAllowBrowser: true }))
  }
}

export async function analyzeScanWithAi({
  apiKey,
  model,
  runId,
  endpoint,
}: {
  apiKey: string
  model: string
  runId: number
  endpoint: AiEndpoint
}): Promise<AiReport> {
  configureClient(apiKey, endpoint)

  const agent = new Agent({
    name: "WinClean storage analyst",
    model: endpoint.kind === "local" ? endpoint.localModel : model,
    instructions: INSTRUCTIONS,
    tools: buildTools(runId),
    outputType: reportSchema,
  })

  const result = await run(
    agent,
    "Analyze the selected scan and recommend what to clean and what to offload to Google Drive."
  )
  const report = result.finalOutput
  if (!report) {
    throw new Error("The AI assistant did not return a report. Try again.")
  }
  return report
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
    model: endpoint.kind === "local" ? endpoint.localModel : model,
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
