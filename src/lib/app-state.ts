export function shouldCommitRunDataResponse({
  requestId,
  latestRequestId,
}: {
  requestId: number
  latestRequestId: number
}) {
  return requestId === latestRequestId
}

export function cleanupActionDisabled({
  hasPlan,
  selectedCount,
  pending,
  scanning,
}: {
  hasPlan: boolean
  selectedCount: number
  pending: boolean
  scanning: boolean
}) {
  return !hasPlan || selectedCount === 0 || pending || scanning
}

export function parentDirectoryPath(
  currentDirectory: string | null | undefined,
  rootPath: string | null | undefined
) {
  if (!currentDirectory || !rootPath) {
    return null
  }
  const current = normalizeWindowsPath(currentDirectory)
  const root = normalizeWindowsPath(rootPath)
  if (!isSameOrChildPath(current, root) || current === root) {
    return null
  }

  const trimmed = current.replace(/\\+$/, "")
  const index = trimmed.lastIndexOf("\\")
  if (index < 0) {
    return null
  }
  const parent = trimmed.slice(0, index + (index === 2 ? 1 : 0))
  return isSameOrChildPath(parent, root) ? parent : null
}

export function directoryBreadcrumbs(
  rootPath: string | null | undefined,
  currentDirectory: string | null | undefined
) {
  if (!rootPath || !currentDirectory) {
    return []
  }
  const root = normalizeWindowsPath(rootPath)
  const current = normalizeWindowsPath(currentDirectory)
  if (!isSameOrChildPath(current, root)) {
    return [{ label: root, path: root }]
  }
  if (current === root) {
    return [{ label: root, path: root }]
  }

  const rootTrimmed = root.replace(/\\+$/, "")
  const relative = current.slice(root.length).replace(/^\\+/, "")
  const segments = relative.split("\\").filter(Boolean)
  const breadcrumbs = [{ label: root, path: root }]
  let path = rootTrimmed
  for (const segment of segments) {
    path = `${path}\\${segment}`
    breadcrumbs.push({ label: segment, path })
  }
  return breadcrumbs
}

export function pruneCleanupSelection(
  selectedIds: Set<string>,
  candidateIds: Iterable<string>
) {
  const available = new Set(candidateIds)
  return new Set([...selectedIds].filter((id) => available.has(id)))
}

function normalizeWindowsPath(path: string) {
  const normalized = path.trim().replace(/\//g, "\\")
  if (/^[A-Za-z]:$/.test(normalized)) {
    return `${normalized}\\`
  }
  if (/^[A-Za-z]:\\?$/.test(normalized)) {
    return normalized.slice(0, 2).toUpperCase() + "\\"
  }
  return normalized.replace(/\\+$/, "")
}

function isSameOrChildPath(path: string, root: string) {
  const normalizedPath = path.toLowerCase()
  const normalizedRoot = root.toLowerCase()
  if (normalizedPath === normalizedRoot) {
    return true
  }
  const childPrefix = normalizedRoot.endsWith("\\")
    ? normalizedRoot
    : `${normalizedRoot}\\`
  return normalizedPath.startsWith(childPrefix)
}
