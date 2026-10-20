import { useEffect, useState } from "react"
import { Loader2 } from "lucide-react"

import { Field, SelectInput, TextInput } from "@/components/common"
import { Button } from "@/components/ui/button"
import { Switch } from "@/components/ui/switch"
import { clampInt, type AutoSwitchConfig, type PowerProfile } from "@/lib/optimizer"

type ProfileSlot = "onAcProfile" | "onBatteryProfile" | "lowBatteryProfile" | "gamingProfile" | "aiDevelopmentProfile" | "codingProfile"

const SLOTS: { key: ProfileSlot; label: string }[] = [
  { key: "onAcProfile", label: "Plugged in" },
  { key: "onBatteryProfile", label: "On battery" },
  { key: "lowBatteryProfile", label: "Battery is low" },
  { key: "gamingProfile", label: "Gaming" },
  { key: "aiDevelopmentProfile", label: "AI development" },
  { key: "codingProfile", label: "Coding" },
]

/**
 * Rules for switching profiles automatically. Off by default. Switching is
 * event-driven (power source, battery level, foreground app) with a minimum
 * dwell time and hysteresis so it cannot flap, and a manual choice always wins
 * for a while.
 */
export function AutoSwitchPanel({
  config,
  profiles,
  saving,
  onSave,
}: {
  config: AutoSwitchConfig
  profiles: PowerProfile[]
  saving: boolean
  onSave: (config: AutoSwitchConfig) => void
}) {
  const [draft, setDraft] = useState(config)
  useEffect(() => setDraft(config), [config])
  const dirty = JSON.stringify(draft) !== JSON.stringify(config)
  const set = <K extends keyof AutoSwitchConfig>(key: K, value: AutoSwitchConfig[K]) => setDraft((d) => ({ ...d, [key]: value }))

  return (
    <div className="space-y-4">
      <label className="flex items-start justify-between gap-4">
        <span className="text-sm">
          <span className="font-medium">Switch profiles automatically</span>
          <span className="block text-xs text-muted-foreground">When on, WinClean applies the profile below that matches your power source, battery level and current activity.</span>
        </span>
        <Switch checked={draft.enabled} onCheckedChange={(v) => set("enabled", v)} aria-label="Switch profiles automatically" />
      </label>

      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {SLOTS.map(({ key, label }) => (
          <Field key={key} label={label}>
            <SelectInput value={draft[key]} onChange={(e) => set(key, e.target.value)} disabled={!draft.enabled}>
              {profiles.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </SelectInput>
          </Field>
        ))}
      </div>

      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <Field label="Low battery at (%)" hint="5–50">
          <TextInput type="number" min={5} max={50} value={draft.lowBatteryPercent} disabled={!draft.enabled} onChange={(e) => set("lowBatteryPercent", clampInt(Number(e.target.value), 5, 50, 20))} />
        </Field>
        <Field label="Hysteresis (%)" hint="Extra margin before leaving low-battery">
          <TextInput type="number" min={1} max={20} value={draft.hysteresisPercent} disabled={!draft.enabled} onChange={(e) => set("hysteresisPercent", clampInt(Number(e.target.value), 1, 20, 5))} />
        </Field>
        <Field label="Minimum time between switches (s)" hint="30–3600">
          <TextInput type="number" min={30} max={3600} value={draft.minDwellSecs} disabled={!draft.enabled} onChange={(e) => set("minDwellSecs", clampInt(Number(e.target.value), 30, 3600, 120))} />
        </Field>
        <Field label="After a manual choice, pause for (min)" hint="0–240">
          <TextInput type="number" min={0} max={240} value={draft.manualOverrideMinutes} disabled={!draft.enabled} onChange={(e) => set("manualOverrideMinutes", clampInt(Number(e.target.value), 0, 240, 30))} />
        </Field>
      </div>

      <label className="flex items-start justify-between gap-4">
        <span className="text-sm">
          <span className="font-medium">Pause while G-Helper is running</span>
          <span className="block text-xs text-muted-foreground">G-Helper also manages ASUS power modes. With this on, WinClean will not fight it by switching profiles on its own.</span>
        </span>
        <Switch checked={draft.respectGhelper} onCheckedChange={(v) => set("respectGhelper", v)} aria-label="Pause automatic switching while G-Helper is running" />
      </label>

      <div className="flex justify-end gap-2">
        <Button variant="outline" disabled={!dirty || saving} onClick={() => setDraft(config)}>
          Reset
        </Button>
        <Button disabled={!dirty || saving} onClick={() => onSave(draft)}>
          {saving ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
          Save rules
        </Button>
      </div>
    </div>
  )
}
