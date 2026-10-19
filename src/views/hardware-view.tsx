import { useCallback, useEffect, useState } from "react"
import { Cpu, Loader2, RefreshCw } from "lucide-react"

import { Button } from "@/components/ui/button"
import { EmptyState, Metric, Notice, Panel, SummaryList } from "@/components/common"
import { api, formatError } from "@/lib/api"
import type { CapabilityReport, HardwareInfo } from "@/lib/optimizer"

/** Display hardware discovery, NPU verification status, and Windows ML and MCP capabilities. */
export function HardwareView() {
  const [hw, setHw] = useState<HardwareInfo | null>(null)
  const [caps, setCaps] = useState<CapabilityReport | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  /** Refresh hardware and capability data together, updating loading and error state. */
  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [h, c] = await Promise.all([api.optimizerHardwareInfo(), api.optimizerCapabilities()])
      setHw(h)
      setCaps(c)
      setError(null)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  return (
    <div className="space-y-4">
      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <Metric label="CPU" value={hw?.cpuIsRyzenAi9Hx370 ? "Ryzen AI 9 HX 370" : (hw?.cpuName ?? "—")} hint={hw?.cpuArchitectureNote ?? ""} />
        <Metric label="Logical processors" value={hw ? String(hw.logicalProcessors) : "—"} />
        <Metric label="RAM" value={hw?.ramTotalMb ? `${(hw.ramTotalMb / 1024).toFixed(1)} GB` : "—"} />
        <Metric label="NPU" value={!hw ? "—" : hw.npu.present ? "NPU detected" : "Not detected"} hint={!hw ? "" : hw.npu.inferenceVerified ? "Inference verified" : "Inference not verified — CPU fallback"} />
      </div>

      <Panel
        title="Hardware & NPU (registry-only discovery)"
        action={
          <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Refresh
          </Button>
        }
      >
        {error ? <Notice tone="danger" text={error} /> : null}
        {loading ? (
          <EmptyState text="Reading CPU, adapters and NPU from the registry (no GPU wake)…" />
        ) : hw ? (
          <div className="space-y-4">
            <p className="flex items-start gap-2 text-sm text-muted-foreground">
              <Cpu className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              {hw.dgpuPolicy} G-Helper {hw.ghelperDetected ? "detected — WinClean stays read-only for ASUS firmware, GPU mode, fans and battery limits." : "not detected."}
            </p>
            <SummaryList
              rows={[
                ["CPU", hw.cpuName],
                ...hw.adapters.map((a) => [`Display (${a.kind})`, `${a.name}${a.driverVersion ? ` · driver ${a.driverVersion}` : ""}`] as [string, string]),
                ["NPU", hw.npu.name ?? "none"],
                ["NPU note", hw.npu.note],
              ]}
            />
          </div>
        ) : null}
      </Panel>

      <Panel title="MCP Integrations & Windows ML (discovery only)">
        {caps ? (
          <div className="space-y-3 text-sm">
            <SummaryList
              rows={[
                ["Windows build", caps.windowsBuild ? String(caps.windowsBuild) : "unknown"],
                ["Copilot+ capable", caps.copilotPlusCapable == null ? "unverified — needs a measured 40+ TOPS NPU" : caps.copilotPlusCapable ? "yes" : "no"],
                ["On-device Agent Registry", caps.mcpAgentRegistryFound ? caps.mcpRegistryPaths.join(", ") : "not found"],
                ["File Explorer MCP", caps.fileExplorerMcp],
                ["Inference route", caps.inferenceRoute.join(" → ")],
                ["NPU verified for inference", caps.npuVerifiedForInference ? "yes" : "no — stays disabled until a model test passes"],
                ...caps.executionProviders.map((ep) => [ep.name, `${ep.status} — ${ep.detail}`] as [string, string]),
                ...caps.notes.map((n, i) => [`Note ${i + 1}`, n] as [string, string]),
              ]}
            />
            <p className="text-xs text-muted-foreground">
              WinClean keeps its native filesystem scanner; MCP file access stays behind Explorer consent. Local inference and a future Lumen agent connect through MCP — WinClean does not embed another agent runtime.
            </p>
          </div>
        ) : (
          <EmptyState text="No capability report yet" />
        )}
      </Panel>
    </div>
  )
}
