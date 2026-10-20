import { useCallback, useEffect, useState } from "react"
import { Plus, Trash2 } from "lucide-react"
import { toast } from "sonner"

import { EmptyState, Notice, Panel } from "@/components/common"
import { AutoSwitchPanel } from "@/components/optimizer/auto-switch-panel"
import { MeasurementPanel } from "@/components/optimizer/measurement-panel"
import { ProfileButtons } from "@/components/optimizer/profile-buttons"
import { ProfileEditor } from "@/components/optimizer/profile-editor"
import { Button } from "@/components/ui/button"
import { useOptimizerEvents } from "@/hooks/use-optimizer-events"
import { api, formatError } from "@/lib/api"
import type { AutoSwitchConfig, MeasurementView, PowerProfile, ProfilesView } from "@/lib/optimizer"

/**
 * Optimization Profiles: apply a profile, define your own, let WinClean switch
 * automatically (off by default), and measure whether a profile really saves
 * power on this machine.
 */
export function ProfilesView() {
  const [view, setView] = useState<ProfilesView | null>(null)
  const [measurements, setMeasurements] = useState<MeasurementView[]>([])
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [editing, setEditing] = useState(false)

  const load = useCallback(async () => {
    try {
      const [v, m] = await Promise.all([api.optimizerListProfiles(), api.optimizerMeasurementList()])
      setView(v)
      setMeasurements(m)
      setError(null)
    } catch (e) {
      setError(formatError(e))
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])
  useOptimizerEvents((event) => {
    if (event.kind === "profileChanged") void load()
    if (event.kind === "autoSwitchError") toast.warning(`Automatic switch to ${event.data.profileId} failed: ${event.data.message}`)
  })

  async function apply(profile: PowerProfile) {
    setBusyId(profile.id)
    try {
      const outcome = await api.optimizerApplyPowerProfile(profile.id)
      toast.success(`${profile.label}: ${outcome.detail}`)
      await load()
    } catch (e) {
      toast.error(formatError(e))
    } finally {
      setBusyId(null)
    }
  }

  async function mutate(action: () => Promise<ProfilesView>, success: string) {
    setSaving(true)
    try {
      setView(await action())
      toast.success(success)
      return true
    } catch (e) {
      toast.error(formatError(e))
      return false
    } finally {
      setSaving(false)
    }
  }

  const custom = view?.profiles.filter((p) => !p.builtin) ?? []
  const activeId = view?.active?.profileId ?? null

  return (
    <div className="space-y-4">
      {error ? <Notice tone="danger" text={error} /> : null}

      <Panel
        title="Profiles"
        action={
          <Button size="sm" variant="outline" onClick={() => setEditing(true)} disabled={!view}>
            <Plus className="size-4" aria-hidden="true" />
            New profile
          </Button>
        }
      >
        {view ? (
          <ProfileButtons profiles={view.profiles} activeId={activeId} busyId={busyId} onApply={(p) => void apply(p)} />
        ) : (
          <EmptyState text="Loading profiles…" />
        )}
        {custom.length > 0 ? (
          <div className="mt-4">
            <h4 className="mb-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">Your profiles</h4>
            <ul className="grid gap-1.5">
              {custom.map((p) => (
                <li key={p.id} className="flex items-center justify-between gap-3 rounded-md border border-border px-3 py-2 text-sm">
                  <span>{p.label}</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={saving || p.id === activeId}
                    title={p.id === activeId ? "Switch to another profile before deleting this one" : undefined}
                    aria-label={`Delete ${p.label}`}
                    onClick={() => void mutate(() => api.optimizerDeleteCustomProfile(p.id), `${p.label} deleted.`)}
                  >
                    <Trash2 className="size-4" aria-hidden="true" />
                  </Button>
                </li>
              ))}
            </ul>
          </div>
        ) : null}
      </Panel>

      <Panel title="Automatic switching">
        {view ? (
          <AutoSwitchPanel
            config={view.autoSwitch}
            profiles={view.profiles}
            saving={saving}
            onSave={(config: AutoSwitchConfig) => void mutate(() => api.optimizerSetAutoSwitch(config), "Automatic switching rules saved.")}
          />
        ) : (
          <EmptyState text="Loading…" />
        )}
      </Panel>

      <Panel title="Measure the effect">
        {view ? <MeasurementPanel measurements={measurements} profiles={view.profiles} onChanged={load} /> : <EmptyState text="Loading…" />}
      </Panel>

      <ProfileEditor
        open={editing}
        existingIds={view?.profiles.map((p) => p.id) ?? []}
        saving={saving}
        onClose={() => setEditing(false)}
        onSave={(profile) => {
          void mutate(() => api.optimizerSaveCustomProfile(profile), `${profile.label} saved.`).then((ok) => ok && setEditing(false))
        }}
      />
    </div>
  )
}
