import {
  ChevronRight,
  ChevronUp,
  FileSearch,
  FolderOpen,
  FolderTree,
  Search,
} from "lucide-react"

import { Button } from "@/components/ui/button"
import { EmptyState, Panel, TextInput } from "@/components/common"
import { formatBytes, formatDateTime, formatNumber } from "@/lib/format"
import type { DirectoryChild, FileSearchResult, ScanRunRecord } from "@/lib/api"

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
            <EmptyState text="Loading directory data" />
          ) : directoryChildren.length === 0 ? (
            <EmptyState text="No children for this directory" />
          ) : (
            directoryChildren.map((entry) => (
              <button
                key={entry.id}
                type="button"
                onClick={() =>
                  entry.isDirectory && void onOpenDirectory(entry.path)
                }
                disabled={!entry.isDirectory}
                className="grid w-full grid-cols-[1fr_120px_90px] items-center gap-3 border-b border-border/60 py-3 text-left text-sm disabled:cursor-default"
              >
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
              </button>
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
            <EmptyState text="No files match this filter" />
          ) : (
            files.map((file) => (
              <div
                key={file.id}
                className="border-b border-border/60 py-3 text-sm"
              >
                <div className="flex items-center justify-between gap-3">
                  <span className="truncate font-medium">{file.name}</span>
                  <span className="tabular-nums">{formatBytes(file.size)}</span>
                </div>
                <div className="mt-1 truncate text-xs text-muted-foreground">
                  {file.path}
                </div>
              </div>
            ))
          )}
        </div>
      </Panel>
    </div>
  )
}
