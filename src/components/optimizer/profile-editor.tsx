import { useState } from "react"
import { Loader2 } from "lucide-react"

import { Field, SelectInput, TextInput } from "@/components/common"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { POWER_SCHEMES, ecoQosModeLabel, newCustomProfile, type EcoQosMode, type PowerProfile } from "@/lib/optimizer"

const MODES: EcoQosMode[] = ["off", "conservative", "battery"]

/** Create a custom profile: a name, a Windows power plan and a background-throttling mode. */
export function ProfileEditor({
  open,
  existingIds,
  saving,
  onSave,
  onClose,
}: {
  open: boolean
  existingIds: string[]
  saving: boolean
  onSave: (profile: PowerProfile) => void
  onClose: () => void
}) {
  const [label, setLabel] = useState("")
  const [description, setDescription] = useState("")
  const [schemeGuid, setSchemeGuid] = useState<string>(POWER_SCHEMES[1].guid)
  const [mode, setMode] = useState<EcoQosMode>("conservative")
  const problem = label.trim().length === 0 ? "Give the profile a name." : label.trim().length > 40 ? "Names are limited to 40 characters." : description.length > 200 ? "Descriptions are limited to 200 characters." : null

  return (
    <Dialog open={open} onOpenChange={(o) => (!o && !saving ? onClose() : undefined)}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New custom profile</DialogTitle>
          <DialogDescription>A custom profile switches the Windows power plan and sets how aggressively background processes are throttled. Built-in profiles cannot be edited.</DialogDescription>
        </DialogHeader>
        <div className="grid gap-3">
          <Field label="Name">
            <TextInput value={label} maxLength={40} onChange={(e) => setLabel(e.target.value)} autoFocus />
          </Field>
          <Field label="Description (optional)">
            <TextInput value={description} maxLength={200} onChange={(e) => setDescription(e.target.value)} />
          </Field>
          <Field label="Windows power plan">
            <SelectInput value={schemeGuid} onChange={(e) => setSchemeGuid(e.target.value)}>
              {POWER_SCHEMES.map((s) => (
                <option key={s.guid} value={s.guid}>
                  {s.label}
                </option>
              ))}
            </SelectInput>
          </Field>
          <Field label="Background throttling (EcoQoS)" hint="Conservative only touches clearly idle background processes; Battery is wider. Developer tools, games and AI inference are always exempt.">
            <SelectInput value={mode} onChange={(e) => setMode(e.target.value as EcoQosMode)}>
              {MODES.map((m) => (
                <option key={m} value={m}>
                  {ecoQosModeLabel(m)}
                </option>
              ))}
            </SelectInput>
          </Field>
          {problem && label.length > 0 ? <p className="text-xs text-destructive">{problem}</p> : null}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={saving}>
            Cancel
          </Button>
          <Button
            disabled={problem !== null || saving}
            onClick={() => onSave(newCustomProfile({ label, description, schemeGuid, ecoQosMode: mode, existingIds }))}
          >
            {saving ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
            Save profile
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
