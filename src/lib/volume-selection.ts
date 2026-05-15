export type ScanVolumeLike = {
  rootPath: string
  driveType: string
  isReady: boolean
}

export function isSupportedScanVolume(volume: ScanVolumeLike) {
  return (
    volume.isReady &&
    (volume.driveType === "fixed" || volume.driveType === "removable")
  )
}

export function selectDefaultScanRoot(
  volumes: ScanVolumeLike[],
  currentRoot: string
) {
  if (currentRoot && volumes.some((volume) => volume.rootPath === currentRoot && isSupportedScanVolume(volume))) {
    return currentRoot
  }

  return (
    volumes.find((volume) => isSupportedScanVolume(volume))?.rootPath ??
    volumes[0]?.rootPath ??
    ""
  )
}
