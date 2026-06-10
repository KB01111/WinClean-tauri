import { AlertTriangle, Info } from "lucide-react"

import { cn } from "@/lib/utils"

export function NavButton({
  icon: Icon,
  label,
  active,
  onClick,
}: {
  icon: React.ComponentType<{ className?: string; "aria-hidden"?: boolean }>
  label: string
  active: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex h-9 w-full items-center gap-2 rounded-md px-3 text-left text-sm font-medium transition-colors",
        active
          ? "bg-primary text-primary-foreground shadow-sm"
          : "text-muted-foreground hover:bg-accent hover:text-accent-foreground"
      )}
    >
      <Icon className="size-4" aria-hidden={true} />
      {label}
    </button>
  )
}

export function Panel({
  title,
  action,
  children,
  className,
}: {
  title: string
  action?: React.ReactNode
  children: React.ReactNode
  className?: string
}) {
  return (
    <section
      className={cn(
        "rounded-xl border border-border bg-card text-card-foreground shadow-sm",
        className
      )}
    >
      <div className="flex items-center justify-between gap-3 border-b border-border/70 px-4 py-3">
        <h3 className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          {title}
        </h3>
        {action}
      </div>
      <div className="p-4">{children}</div>
    </section>
  )
}

export function Metric({
  label,
  value,
  hint,
}: {
  label: string
  value: string
  hint?: string
}) {
  return (
    <div className="rounded-xl border border-border bg-card p-4 shadow-sm">
      <div className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
        {label}
      </div>
      <div className="mt-1 truncate text-xl font-semibold tabular-nums">
        {value}
      </div>
      {hint ? (
        <div className="mt-0.5 truncate text-xs text-muted-foreground">
          {hint}
        </div>
      ) : null}
    </div>
  )
}

export function Notice({
  text,
  tone,
}: {
  text: string
  tone: "warning" | "danger" | "info"
}) {
  const classes =
    tone === "danger"
      ? "border-destructive/40 bg-destructive/10 text-destructive"
      : tone === "warning"
        ? "border-amber-600/40 bg-amber-500/10 text-amber-800 dark:text-amber-300"
        : "border-primary/30 bg-accent/60 text-accent-foreground"
  const Icon = tone === "info" ? Info : AlertTriangle
  return (
    <div
      className={cn("mb-3 flex gap-3 rounded-lg border p-3 text-sm", classes)}
    >
      <Icon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <p className="min-w-0 break-words">{text}</p>
    </div>
  )
}

export function DataTable({
  headers,
  rows,
  empty = "No data",
}: {
  headers: string[]
  rows: string[][]
  empty?: string
}) {
  return (
    <table className="w-full table-fixed border-collapse text-sm">
      <thead>
        <tr className="border-b border-border text-left text-xs text-muted-foreground uppercase">
          {headers.map((header, index) => (
            <th
              key={header}
              className={cn("py-2 font-semibold", index > 0 && "text-right")}
            >
              {header}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.length === 0 ? (
          <tr>
            <td
              className="py-10 text-center text-muted-foreground"
              colSpan={headers.length}
            >
              {empty}
            </td>
          </tr>
        ) : (
          rows.map((row, rowIndex) => (
            <tr key={rowIndex} className="border-b border-border/60">
              {row.map((cell, index) => (
                <td
                  key={`${cell}-${index}`}
                  className={cn(
                    "truncate py-3",
                    index > 0
                      ? "text-right tabular-nums"
                      : "pr-3 font-medium"
                  )}
                  title={cell}
                >
                  {cell}
                </td>
              ))}
            </tr>
          ))
        )}
      </tbody>
    </table>
  )
}

export function SummaryList({ rows }: { rows: [string, string][] }) {
  if (rows.length === 0) {
    return <EmptyState text="No summary data yet" />
  }
  return (
    <div className="space-y-2">
      {rows.map(([label, value]) => (
        <div
          key={label}
          className="flex items-center justify-between gap-3 text-sm"
        >
          <span className="truncate text-muted-foreground" title={label}>
            {label}
          </span>
          <span className="shrink-0 font-medium tabular-nums">{value}</span>
        </div>
      ))}
    </div>
  )
}

export function Posture({
  icon: Icon,
  title,
  text,
}: {
  icon: React.ComponentType<{ className?: string; "aria-hidden"?: boolean }>
  title: string
  text: string
}) {
  return (
    <div className="rounded-lg border border-border bg-muted/40 p-3">
      <div className="mb-2 flex items-center gap-2 font-medium">
        <Icon className="size-4 text-primary" aria-hidden={true} />
        {title}
      </div>
      <p className="text-sm text-muted-foreground">{text}</p>
    </div>
  )
}

export function EmptyState({ text }: { text: string }) {
  return (
    <div className="py-10 text-center text-sm text-muted-foreground">
      {text}
    </div>
  )
}

export function ProgressBar({
  percent,
  className,
}: {
  percent: number
  className?: string
}) {
  return (
    <div
      className={cn(
        "h-2 overflow-hidden rounded-full bg-muted",
        className
      )}
      role="progressbar"
      aria-valuenow={Math.min(100, Math.max(0, percent))}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div
        className="h-full rounded-full bg-primary transition-[width] duration-300"
        style={{ width: `${Math.min(100, Math.max(0, percent))}%` }}
      />
    </div>
  )
}

const STATUS_TONES: Record<string, string> = {
  completed: "bg-primary/15 text-primary",
  recycled: "bg-primary/15 text-primary",
  restored: "bg-primary/15 text-primary",
  running: "bg-sky-500/15 text-sky-700 dark:text-sky-300",
  uploading: "bg-sky-500/15 text-sky-700 dark:text-sky-300",
  validating: "bg-sky-500/15 text-sky-700 dark:text-sky-300",
  queued: "bg-muted text-muted-foreground",
  canceled: "bg-muted text-muted-foreground",
  interrupted: "bg-amber-500/15 text-amber-700 dark:text-amber-300",
  completed_with_errors: "bg-amber-500/15 text-amber-700 dark:text-amber-300",
  uploaded_recycle_failed: "bg-amber-500/15 text-amber-700 dark:text-amber-300",
  skipped: "bg-amber-500/15 text-amber-700 dark:text-amber-300",
  failed: "bg-destructive/15 text-destructive",
}

export function StatusBadge({ status }: { status: string }) {
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center rounded-full px-2 py-0.5 text-xs font-medium whitespace-nowrap",
        STATUS_TONES[status] ?? "bg-muted text-muted-foreground"
      )}
    >
      {status.replaceAll("_", " ")}
    </span>
  )
}

export function TextInput(props: React.ComponentProps<"input">) {
  const { className, ...rest } = props
  return (
    <input
      {...rest}
      className={cn(
        "h-9 w-full rounded-md border border-input bg-card px-3 text-sm outline-none transition-colors placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/40",
        className
      )}
    />
  )
}
