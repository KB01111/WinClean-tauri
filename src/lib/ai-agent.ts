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

export const AI_MODELS = [
  "gpt-5.1",
  "gpt-5",
  "gpt-5-mini",
  "gpt-4.1",
  "gpt-4.1-mini",
] as const

const AI_MODEL_STORAGE_KEY = "winclean-ai-model"
const DEFAULT_AI_MODEL = "gpt-5-mini"

export function getStoredAiModel(): string {
  const stored = localStorage.getItem(AI_MODEL_STORAGE_KEY)
  if (stored && (AI_MODELS as readonly string[]).includes(stored)) {
    return stored
  }
  return DEFAULT_AI_MODEL
}

export function setStoredAiModel(model: string) {
  localStorage.setItem(AI_MODEL_STORAGE_KEY, model)
}

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

/** Build scan-scoped storage tools and read-only optimizer tools for the AI analyst. */
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

  return [
    getScanSummary,
    getTopDirectories,
    getLargestFiles,
    getStaleLargeFiles,
    getRecommendations,
    // WinClean 2.0 read-only optimizer tools (defined below via buildOptimizerTools)
    ...buildOptimizerTools(),
  ]
}

/** Build read-only service, process, power, and hardware tools for the AI analyst. */
function buildOptimizerTools() {
  const getServices = tool({
    name: "get_services",
    description:
      "READ-ONLY. Windows services with start type, state, risk and recommendation. Protected services must never be suggested for change.",
    parameters: z.object({}),
    /** Serialize up to 300 services with their current state and risk recommendations. */
    execute: async () => {
      const services = await api.optimizerListServices()
      return JSON.stringify(
        services.slice(0, 300).map((s) => ({
          name: s.name,
          displayName: s.displayName,
          state: s.state,
          startType: s.startType,
          risk: s.classification.risk,
          recommendation: s.classification.recommendation,
        }))
      )
    },
  })
  const getProcesses = tool({
    name: "get_processes",
    description:
      "READ-ONLY. Top processes by memory with CPU %, foreground flag and EcoQoS decision.",
    parameters: z.object({}),
    /** Serialize resource usage and EcoQoS decisions for the first 60 processes returned by the backend. */
    execute: async () => {
      const processes = await api.optimizerListProcesses()
      return JSON.stringify(
        processes.slice(0, 60).map((p) => ({
          pid: p.pid,
          name: p.name,
          workingSetMb: p.workingSetMb,
          cpuPercent: p.cpuPercentRecent,
          ecoDecision: p.ecoDecision,
        }))
      )
    },
  })
  const getPowerStatus = tool({
    name: "get_power_status",
    description:
      "READ-ONLY. Battery percent, AC/DC state, active power scheme, power-history sample count.",
    parameters: z.object({}),
    /** Serialize battery telemetry and the number of recent power samples returned by the backend. */
    execute: async () => {
      const [battery, history] = await Promise.all([
        api.optimizerBatteryStatus(),
        api.optimizerPowerHistory(50),
      ])
      return JSON.stringify({ battery, samples: history.length })
    },
  })
  const getHardware = tool({
    name: "get_hardware",
    description:
      "READ-ONLY. CPU, display adapters (registry-only, never wakes the RTX 4070), NPU and Windows ML / MCP capabilities.",
    parameters: z.object({}),
    /** Serialize hardware discovery and Windows ML and MCP capability information. */
    execute: async () => {
      const [hardware, capabilities] = await Promise.all([
        api.optimizerHardwareInfo(),
        api.optimizerCapabilities(),
      ])
      return JSON.stringify({ hardware, capabilities })
    },
  })
  return [getServices, getProcesses, getPowerStatus, getHardware]
}
const INSTRUCTIONS = `You are WinClean's storage analyst. You help Windows users understand what is
using their disk space and what they can safely clean up or offload to Google Drive.

Process:
1. Call get_scan_summary and get_top_directories to understand the disk.
2. Call get_recommendations to see the actionable candidates and their ids.
3. Optionally call get_largest_files / get_stale_large_files for extra context.
4. Produce a short, friendly summary and 2-6 concrete suggestions.

Optimizer rules:
- get_services / get_processes / get_power_status / get_hardware are READ-ONLY. You cannot change services, processes or power schemes.
- Never suggest changing a protected service or throttling a protected/foreground process. Any service suggestion must name a service returned by get_services and is validated deterministically before the user can approve it.

Rules:
- Suggestions must only reference candidate ids returned by get_recommendations.
- Group related candidates into one suggestion (e.g. "Old installers in Downloads").
- estimatedBytes must be the sum of sizeBytes for the included candidates.
- Prefer high-impact, low-risk wins first. Never invent files or ids.
- Be concise and avoid technical jargon.`

export async function analyzeScanWithAi({
  apiKey,
  model,
  runId,
}: {
  apiKey: string
  model: string
  runId: number
}): Promise<AiReport> {
  setTracingDisabled(true)
  setOpenAIAPI("chat_completions")
  setDefaultOpenAIClient(
    new OpenAI({ apiKey, dangerouslyAllowBrowser: true })
  )

  const agent = new Agent({
    name: "WinClean storage analyst",
    model,
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
