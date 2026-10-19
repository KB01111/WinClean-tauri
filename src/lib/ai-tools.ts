// Read-only diagnostic tools for the AI optimization assistant.
//
// Every tool here only READS. There is deliberately no tool that changes a
// service, a process, a power profile or a file: the assistant proposes, the
// backend validates, and only the user's explicit approval applies anything.
// Each tool returns compact JSON (the model has a limited context window).

import { tool } from "@openai/agents"
import { z } from "zod"

import { api } from "@/lib/api"
import { recommendationMemoryMb } from "@/lib/optimizer"

const empty = z.object({})

function json(value: unknown): string {
  return JSON.stringify(value)
}

export function buildOptimizerTools() {
  const getServices = tool({
    name: "get_services",
    description:
      "READ-ONLY. Windows services with start type, state, dependents, real memory (own-process services only), risk class and category. Services with risk 'protected' must never be proposed for change.",
    parameters: z.object({ limit: z.number().describe("How many services to return, max 200") }),
    execute: async ({ limit }) => {
      const services = await api.optimizerListServices()
      return json(
        services.slice(0, Math.min(Math.max(1, Math.round(limit)), 200)).map((s) => ({
          name: s.name,
          displayName: s.displayName,
          state: s.state,
          startType: s.startType,
          triggerStart: s.triggerStart,
          dependents: s.dependents.length,
          memoryMb: s.memoryMb,
          risk: s.classification.risk,
          category: s.classification.category,
          canDisable: s.classification.canDisable,
        }))
      )
    },
  })

  const getServiceRecommendations = tool({
    name: "get_service_recommendations",
    description:
      "READ-ONLY. WinClean's own deterministic start-type recommendations (already policy-valid). Prefer these over inventing changes.",
    parameters: empty,
    execute: async () => {
      const recs = await api.optimizerServiceRecommendations()
      return json({
        runningMemoryMb: recommendationMemoryMb(recs),
        recommendations: recs.map((r) => ({
          serviceName: r.serviceName,
          from: r.currentStartType,
          to: r.suggestedStartType,
          rationale: r.rationale,
          confidence: r.confidence,
          running: r.running,
          memoryMb: r.memoryMb,
        })),
      })
    },
  })

  const getProcesses = tool({
    name: "get_processes",
    description: "READ-ONLY. Top processes by memory with CPU %, foreground flag and EcoQoS eligibility.",
    parameters: empty,
    execute: async () => {
      const processes = await api.optimizerListProcesses()
      return json(
        processes.slice(0, 60).map((p) => ({
          pid: p.pid,
          name: p.name,
          workingSetMb: p.workingSetMb,
          cpuPercent: p.cpuPercentRecent,
          foreground: p.isForeground,
          ecoDecision: p.ecoDecision,
          ecoQosApplied: p.ecoQosApplied,
        }))
      )
    },
  })

  const getWorkload = tool({
    name: "get_workload",
    description: "READ-ONLY. What the user is doing right now: coding, gaming, AI inference/development or general use, with evidence.",
    parameters: empty,
    execute: async () => json(await api.optimizerWorkload()),
  })

  const getBattery = tool({
    name: "get_battery",
    description:
      "READ-ONLY. Real battery telemetry: instantaneous and average discharge watts, health %, cycle count, estimated runtime, AC/DC state. Figures are null when the battery does not report them; never estimate missing values.",
    parameters: empty,
    execute: async () => {
      const view = await api.optimizerBatteryTelemetry()
      const t = view.telemetry
      return json({
        onAc: view.status?.onAc ?? null,
        percent: view.status?.percent ?? null,
        activeScheme: view.status?.activeSchemeName ?? null,
        dischargeWatts: t.dischargeWatts,
        averageDischargeWatts: t.averageDischargeWatts,
        energyCrossCheckWatts: t.energyCrossCheckWatts,
        estimatedRuntimeMinutes: t.estimatedRuntimeMinutes,
        healthPercent: t.info?.healthPercent ?? null,
        cycleCount: t.info?.cycleCount ?? null,
        notes: t.notes,
      })
    },
  })

  const getHardware = tool({
    name: "get_hardware",
    description:
      "READ-ONLY. CPU topology, adapters (registry only — this never wakes the RTX 4070), dGPU power state, NPU, G-Helper presence. If the dGPU is powered down, do not suggest anything that would use it.",
    parameters: empty,
    execute: async () => {
      const hw = await api.optimizerHardwareInfo(false)
      return json({
        cpu: hw.cpuName,
        topology: hw.topology && { physicalCores: hw.topology.physicalCores, logicalProcessors: hw.topology.logicalProcessors, clusters: hw.topology.clusters, zenLayoutConfirmed: hw.topology.zenLayoutConfirmed },
        adapters: hw.adapters.map((a) => ({ name: a.name, kind: a.kind, driver: a.driverVersion })),
        dgpu: hw.dgpu,
        igpuOnly: hw.igpuOnly,
        npu: { present: hw.npu.present, generation: hw.npu.generation, driver: hw.npu.driverVersion, inferenceVerified: hw.npu.inferenceVerified },
        gHelper: hw.ghelper,
        platform: hw.platform,
      })
    },
  })

  const getCapabilities = tool({
    name: "get_capabilities",
    description:
      "READ-ONLY. Windows ML execution-provider assessment and routing for battery/AC. Providers are at most candidates: no inference session is run. The sleeping RTX 4070 is excluded.",
    parameters: empty,
    execute: async () => {
      const c = await api.optimizerCapabilities()
      return json({
        windowsBuild: c.windowsBuild,
        copilotPlus: c.copilotPlus,
        routeOnBattery: c.routeOnBattery.map((r) => ({ ep: r.ep, status: r.status })),
        routeOnAc: c.routeOnAc.map((r) => ({ ep: r.ep, status: r.status })),
        providers: c.providers.map((p) => ({ name: p.name, readiness: p.readiness, summary: p.summary })),
        npuVerifiedForInference: c.npuVerifiedForInference,
      })
    },
  })

  const getProfiles = tool({
    name: "get_profiles",
    description: "READ-ONLY. Available power profiles (ids, labels, EcoQoS mode) and which one is active, plus the automatic switching rules.",
    parameters: empty,
    execute: async () => {
      const v = await api.optimizerListProfiles()
      return json({
        active: v.active?.profileId ?? null,
        profiles: v.profiles.map((p) => ({ id: p.id, label: p.label, ecoQosMode: p.ecoQosMode, description: p.description })),
        autoSwitch: { enabled: v.autoSwitch.enabled, respectGhelper: v.autoSwitch.respectGhelper },
      })
    },
  })

  const getHistory = tool({
    name: "get_optimization_history",
    description: "READ-ONLY. The most recent optimizer changes (what WinClean already did), newest first.",
    parameters: empty,
    execute: async () => {
      const h = await api.optimizerHistory(25)
      return json(h.audit.map((a) => ({ kind: a.kind, target: a.target, action: a.action, status: a.status, at: a.createdAtMs })))
    },
  })

  return [getServices, getServiceRecommendations, getProcesses, getWorkload, getBattery, getHardware, getCapabilities, getProfiles, getHistory]
}

/** Names of the read-only tools; exported so a test can assert none mutates. */
export const OPTIMIZER_TOOL_NAMES = [
  "get_services",
  "get_service_recommendations",
  "get_processes",
  "get_workload",
  "get_battery",
  "get_hardware",
  "get_capabilities",
  "get_profiles",
  "get_optimization_history",
] as const
