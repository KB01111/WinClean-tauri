import { useState } from "react"
import { Loader2, ScanSearch } from "lucide-react"

import { Notice, TextInput } from "@/components/common"
import { Badge, type BadgeTone } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { api, formatError } from "@/lib/api"
import type { CapabilityReport, ModelFit } from "@/lib/optimizer"

const FIT_TONE: Record<ModelFit["fit"], BadgeTone> = { good: "good", risky: "warn", unsupported: "bad", unknown: "neutral" }

/**
 * Reads an ONNX file's structure (operators, quantisation, shapes) without
 * running it, and says which execution providers it plausibly fits. "Good" is
 * a structural match, not a benchmark or a guarantee that the model runs.
 */
export function ModelAnalyzer() {
  const [path, setPath] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [model, setModel] = useState<CapabilityReport["model"]>(null)

  async function analyze() {
    const trimmed = path.trim().replace(/^"(.*)"$/, "$1")
    if (!trimmed || busy) return
    setBusy(true)
    setError(null)
    try {
      const report = await api.optimizerCapabilities(trimmed)
      setModel(report.model)
      if (report.model?.error) setError(report.model.error)
    } catch (e) {
      setError(formatError(e))
      setModel(null)
    } finally {
      setBusy(false)
    }
  }

  const profile = model?.profile
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <TextInput
          value={path}
          onChange={(e) => setPath(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void analyze()}
          placeholder={String.raw`C:\models\model.onnx`}
          aria-label="Path to an ONNX model"
          className="max-w-md flex-1"
        />
        <Button onClick={() => void analyze()} disabled={!path.trim() || busy}>
          {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <ScanSearch className="size-4" aria-hidden="true" />}
          Analyze model
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">The file is read locally; only its structure is inspected. The model is not run and nothing is uploaded.</p>
      {error ? <Notice tone="warning" text={error} /> : null}
      {profile ? (
        <div className="space-y-3 text-sm">
          <div className="flex flex-wrap gap-2">
            <Badge>{(profile.fileSizeBytes / 1024 / 1024).toFixed(1)} MB</Badge>
            <Badge>{profile.nodeCount} nodes</Badge>
            {profile.opset != null ? <Badge>opset {profile.opset}</Badge> : null}
            {profile.quantized ? <Badge tone="info">quantized</Badge> : null}
            {profile.blockQuantized ? <Badge tone="info">block-quantized</Badge> : null}
            {profile.float16Weights ? <Badge tone="info">fp16 weights</Badge> : null}
            {profile.dynamicInputShapes ? <Badge tone="warn">dynamic shapes</Badge> : null}
            {profile.usesExternalData ? <Badge tone="warn">external data</Badge> : null}
          </div>
          <ul className="grid gap-2">
            {model?.fits.map((fit) => (
              <li key={fit.ep} className="rounded-lg border border-border bg-card p-3">
                <div className="flex items-center gap-2">
                  <span className="font-medium">{fit.ep.replace("ExecutionProvider", "")}</span>
                  <Badge tone={FIT_TONE[fit.fit]}>{fit.fit}</Badge>
                </div>
                <ul className="mt-1 list-disc pl-5 text-xs text-muted-foreground">
                  {fit.reasons.map((r) => (
                    <li key={r}>{r}</li>
                  ))}
                </ul>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </div>
  )
}
