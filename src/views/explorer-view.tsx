import {
  ChevronRight,
  ChevronUp,
  FileSearch,
  FolderOpen,
  FolderSearch,
  FolderTree,
  Search,
  SearchX,
} from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Panel, TextInput } from "@/components/common"
import { api, formatError } from "@/lib/api"
import { formatBytes, formatDateTime, formatNumber } from "@/lib/format"
import type { DirectoryChild, FileSearchResult, ScanRunRecord } from "@/lib/api"

/** Render scan directory navigation, file filters, results, and Explorer reveal actions. */
export function ExplorerView({
  activeRun,
  currentDirectory,
  breadcrumbs,
  parentDirectory,
  directoryChildren,
  files,
  fileQuery,
  extensionFilter,
  loadingRunData,
  minSizeMbFilter,
  onFileQueryChange,
  onExtensionFilterChange,
  onMinSizeMbFilterChange,
  onOpenDirectory,
}: {
  activeRun: ScanRunRecord | null
  currentDirectory: string | null
  breadcrumbs: { label: string; path: string }[]
  parentDirectory: string | null
  directoryChildren: DirectoryChild[]
  files: FileSearchResult[]
  fileQuery: string
  extensionFilter: string
  loadingRunData: boolean
  minSizeMbFilter: string
  onFileQueryChange: (value: string) => void
  onExtensionFilterChange: (value: string) => void
  onMinSizeMbFilterChange: (value: string) => void
  onOpenDirectory: (path: string) => void
}) {
  return (
    <div className="grid gap-4 xl:grid-cols-[1fr_420px]">
      <Panel
        title="Directory browser"
        action={
          <Button
            variant="outline"
            size="sm"
            disabled={!parentDirectory || loadingRunData}
            onClick={() => parentDirectory && onOpenDirectory(parentDirectory)}
          >
            <ChevronUp className="size-4" aria-hidden="true" />
            Up
          </Button>
        }
      >
        <div className="mb-3 space-y-2 rounded-lg bg-muted/60 px-3 py-2 text-sm text-muted-foreground">
          <div className="flex items-center gap-2">
            <FolderOpen className="size-4" aria-hidden="true" />
            <span className="truncate">
              {currentDirectory ?? activeRun?.rootPath ?? "No scan selected"}
            </span>
          </div>
          {breadcrumbs.length > 0 ? (
            <div className="flex flex-wrap items-center gap-1 text-xs">
              {breadcrumbs.map((crumb, index) => (
                <span key={crumb.path} className="flex items-center gap-1">
                  {index > 0 ? (
                    <ChevronRight className="size-3" aria-hidden="true" />
                  ) : null}
                  <button
                    type="button"
                    className="max-w-44 truncate rounded px-1 py-0.5 text-primary hover:bg-card"
                    onClick={() => onOpenDirectory(crumb.path)}
                    disabled={loadingRunData || crumb.path === currentDirectory}
                    title={crumb.path}
                  >
                    {crumb.label}
                  </button>
                </span>
              ))}
            </div>
          ) : null}
        </div>
        <div className="max-h-[560px] overflow-auto">
          {loadingRunData ? (
            <div className="space-y-2">
              {Array.from({ length: 8 }, (_, index) => (
                <div
                  key={index}
                  className="h-12 w-full animate-pulse rounded-md bg-muted"
                />
              ))}
            </div>
          ) : directoryChildren.length === 0 ? (
            <div className="flex flex-col items-center gap-2 py-10 text-center text-sm text-muted-foreground">
              <FolderSearch className="size-8" aria-hidden="true" />
              <span>No children for this directory</span>
            </div>
          ) : (
            directoryChildren.map((entry) => (
              <div
                key={entry.id}
                className="relative grid grid-cols-[minmax(0,1fr)_120px_90px_auto] items-center gap-3 border-b border-border/60 py-3 text-sm"
              >
                {entry.isDirectory ? (
                  <button
                    type="button"
                    aria-label={`Open ${entry.name}`}
                    className="absolute inset-0 cursor-pointer rounded-sm transition-colors hover:bg-accent/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
                    onClick={() => void onOpenDirectory(entry.path)}
                  />
                ) : null}
                <span className="min-w-0">
                  <span className="flex items-center gap-2 truncate font-medium">
                    {entry.isDirectory ? (
                      <FolderTree
                        className="size-4 text-primary"
                        aria-hidden="true"
                      />
                    ) : (
                      <FileSearch
                        className="size-4 text-muted-foreground"
                        aria-hidden="true"
                      />
                    )}
                    {entry.name}
                  </span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {entry.path}
                  </span>
                </span>
                <span className="text-right tabular-nums">
                  {formatBytes(entry.totalSize || entry.size)}
                </span>
                <span className="text-right text-xs text-muted-foreground">
                  {entry.isDirectory
                    ? `${formatNumber(entry.fileCount)} files`
                    : formatDateTime(entry.modifiedAtMs)}
                </span>
                <RevealButton path={entry.path} name={entry.name} />
              </div>
            ))
          )}
        </div>
      </Panel>

      <Panel title="File search">
        <div className="mb-3 grid grid-cols-[1fr_90px_110px] gap-2">
          <label className="flex items-center gap-2 rounded-md border border-input bg-card px-3 py-2 transition-colors focus-within:border-ring focus-within:ring-2 focus-within:ring-ring/40">
            <Search
              className="size-4 text-muted-foreground"
              aria-hidden="true"
            />
            <input
              value={fileQuery}
              onChange={(event) => onFileQueryChange(event.target.value)}
              className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
              placeholder="Search files"
            />
          </label>
          <TextInput
            value={extensionFilter}
            onChange={(event) => onExtensionFilterChange(event.target.value)}
            placeholder=".log"
          />
          <TextInput
            value={minSizeMbFilter}
            onChange={(event) => onMinSizeMbFilterChange(event.target.value)}
            placeholder="Min MB"
            inputMode="numeric"
          />
        </div>
        <div className="max-h-[560px] overflow-auto">
          {files.length === 0 ? (
            <div className="flex flex-col items-center gap-2 py-10 text-center text-sm text-muted-foreground">
              <SearchX className="size-8" aria-hidden="true" />
              <span>No files match this filter</span>
            </div>
          ) : (
            files.map((file) => (
              <div
                key={file.id}
                className="border-b border-border/60 py-3 text-sm"
              >
                <div className="flex items-center justify-between gap-3">
                  <span className="min-w-0 truncate font-medium">
                    {file.name}
                  </span>
                  <span className="flex shrink-0 items-center gap-2">
                    <span className="tabular-nums">
                      {formatBytes(file.size)}
                    </span>
                    <RevealButton path={file.path} name={file.name} />
                  </span>
                </div>
                <div className="mt-1 truncate text-xs text-muted-foreground">
                  {file.path}
                </div>
                <div className="mt-0.5 truncate text-xs text-muted-foreground">
                  {formatDateTime(file.modifiedAtMs)}
                </div>
              </div>
            ))
          )}
        </div>
      </Panel>
    </div>
  )
}

/** Reveal a path in Windows Explorer without triggering its parent row’s click action. */
function RevealButton({ path, name }: { path: string; name: string }) {
  return (
    <button
      type="button"
      aria-label={`Reveal ${name} in Explorer`}
      className="relative z-10 shrink-0 rounded-md p-1.5 text-primary transition-colors hover:text-primary/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
      onClick={(event) => {
        event.stopPropagation()
        api.revealInExplorer(path).catch((error) => {
          toast.error(formatError(error))
        })
      }}
    >
      <FolderOpen className="size-3.5" aria-hidden="true" />
    </button>
  )
}
