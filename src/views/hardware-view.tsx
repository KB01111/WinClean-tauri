import { useCallback, useEffect, useState } from "react"
import { Loader2, RefreshCw } from "lucide-react"

import { EmptyState, Metric, Notice, Panel, SummaryList } from "@/components/common"
import { EpPanel } from "@/components/optimizer/ep-panel"
import { ModelAnalyzer } from "@/components/optimizer/model-analyzer"
import { Badge, type BadgeTone } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { api, formatError } from "@/lib/api"
import type { CapabilityReport, DgpuPower, HardwareInfo } from "@/lib/optimizer"

const ADAPTER_KIND: Record<string, string> = { igpu: "Integrated GPU", dgpu: "Discrete GPU", other: "Display adapter" }

const DGPU_LABEL: Record<DgpuPower, { label: string; tone: BadgeTone }> = {
  awake: { label: "Awake", tone: "warn" },
  poweredDown: { label: "Powered down", tone: "good" },
  disabled: { label: "Disabled in Windows", tone: "neutral" },
  absent: { label: "Not present", tone: "neutral" },
  unknown: { label: "State unknown", tone: "neutral" },
}

/**
 * Hardware & NPU. Discovery reads the registry, SetupAPI and the CPU topology
 * only. The discrete GPU is described from Windows' cached PnP power state and
 * is never queried through a GPU API, so looking at this page cannot wake it.
 */
export function HardwareView() {
  const [hw, setHw] = useState<HardwareInfo | null>(null)
  const [caps, setCaps] = useState<CapabilityReport | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  const load = useCallback(async (refresh = false) => {
    setLoading(true)
    try {
      const [h, c] = await Promise.all([api.optimizerHardwareInfo(refresh), api.optimizerCapabilities()])
      setHw(h)
      setCaps(c)
      setError(null)
    } catch (e) {
      setError(formatError(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const dgpu = hw ? DGPU_LABEL[hw.dgpu.power] : null

  return (
    <div className="space-y-4">
      {error ? <Notice tone="danger" text={error} /> : null}
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <Metric label="CPU" value={hw ? (hw.cpuIsRyzenAi9Hx370 ? "Ryzen AI 9 HX 370" : hw.cpuName) : "—"} hint={hw?.cpuArchitectureNote} />
        <Metric
          label="Cores / threads"
          value={hw?.topology ? `${hw.topology.physicalCores} / ${hw.topology.logicalProcessors}` : hw ? `— / ${hw.logicalProcessors}` : "—"}
          hint={hw?.topology ? (hw.topology.smtEnabled ? "SMT on" : "SMT off") : undefined}
        />
        <Metric label="Memory" value={hw?.ramTotalMb ? `${(hw.ramTotalMb / 1024).toFixed(1)} GB` : "—"} />
        <Metric
          label="NPU"
          value={!hw ? "—" : hw.npu.present ? (hw.npu.generation ?? "Detected") : "Not detected"}
          hint={hw?.npu.present ? "inference not verified" : undefined}
        />
      </div>

      <Panel
        title="Platform"
        action={
          <Button variant="outline" size="sm" onClick={() => void load(true)} disabled={loading}>
            {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-4" aria-hidden="true" />}
            Re-scan hardware
          </Button>
        }
      >
        {loading && !hw ? (
          <EmptyState text="Reading CPU, adapters and NPU (no GPU is woken)…" />
        ) : hw ? (
          <div className="space-y-4">
            <SummaryList
              rows={[
                ["System", [hw.platform.manufacturer, hw.platform.productName].filter(Boolean).join(" ") || "unknown"],
                ["BIOS", hw.platform.biosVersion ?? "unknown"],
                ["ROG Zephyrus G16", hw.platform.isZephyrusG16 ? "detected" : "not detected"],
                ["G-Helper", hw.ghelper.installed ? (hw.ghelper.running ? "installed, running" : "installed") : "not found"],
              ]}
            />
            <p className="text-sm text-muted-foreground">{hw.ghelper.coexistence}</p>
          </div>
        ) : null}
      </Panel>

      {hw?.topology ? (
        <Panel title="CPU topology">
          <ul className="grid gap-2 md:grid-cols-2">
            {hw.topology.clusters.map((c) => (
              <li key={`${c.label}-${c.efficiencyClass}`} className="rounded-lg border border-border bg-card p-3 text-sm">
                <div className="flex items-center justify-between gap-2">
                  <span className="font-medium">{c.label}</span>
                  <Badge tone={c.role === "efficiency" ? "info" : "neutral"}>{c.role}</Badge>
                </div>
                <p className="mt-1 text-xs text-muted-foreground">
                  {c.physicalCores} cores · {c.logicalProcessors} threads{c.l3Kb ? ` · ${(c.l3Kb / 1024).toFixed(0)} MB L3` : ""}
                </p>
              </li>
            ))}
          </ul>
          {hw.topology.notes.length ? (
            <ul className="mt-3 list-disc space-y-0.5 pl-5 text-xs text-muted-foreground">
              {hw.topology.notes.map((n) => (
                <li key={n}>{n}</li>
              ))}
            </ul>
          ) : null}
        </Panel>
      ) : hw?.topologyError ? (
        <Notice tone="warning" text={`CPU topology unavailable: ${hw.topologyError}`} />
      ) : null}

      {hw ? (
        <Panel title="Graphics and NPU">
          <ul className="grid gap-2">
            {hw.adapters.map((a) => (
              <li key={a.hardwareId ?? a.name} className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border bg-card p-3 text-sm">
                <span>
                  <span className="font-medium">{a.name}</span>
                  <span className="block text-xs text-muted-foreground">{ADAPTER_KIND[a.kind] ?? a.kind}{a.driverVersion ? ` · driver ${a.driverVersion}` : ""}{a.driverDate ? ` (${a.driverDate})` : ""}</span>
                </span>
                {a.kind === "dgpu" && dgpu ? <Badge tone={dgpu.tone}>{dgpu.label}</Badge> : null}
              </li>
            ))}
            <li className="rounded-lg border border-border bg-card p-3 text-sm">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="font-medium">{hw.npu.name ?? "No NPU found"}</span>
                {hw.npu.present ? <Badge tone="neutral">inference not verified</Badge> : null}
              </div>
              <p className="mt-1 text-xs text-muted-foreground">{hw.npu.note}{hw.npu.driverVersion ? ` Driver ${hw.npu.driverVersion}.` : ""}</p>
            </li>
          </ul>
          <p className="mt-3 text-xs text-muted-foreground">{hw.dgpuPolicy} {hw.dgpu.note}</p>
        </Panel>
      ) : null}

      <Panel title="Windows ML and execution providers">
        {caps ? (
          <div className="space-y-4">
            <Notice tone="info" text={caps.copilotPlus.note} />
            <EpPanel report={caps} />
            {caps.notes.length ? (
              <ul className="list-disc space-y-0.5 pl-5 text-xs text-muted-foreground">
                {caps.notes.map((n) => (
                  <li key={n}>{n}</li>
                ))}
              </ul>
            ) : null}
          </div>
        ) : (
          <EmptyState text={loading ? "Assessing execution providers…" : "No capability report"} />
        )}
      </Panel>

      <Panel title="Analyze an ONNX model">
        <ModelAnalyzer />
      </Panel>
    </div>
  )
}
