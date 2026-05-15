# WinClean Privacy Notice

WinClean is local-first. Scan data, file indexes, cleanup history, reports, and diagnostics are stored on the user's Windows device.

## Data Stored Locally

- Volume metadata needed to show disk capacity and filesystem type.
- Scan runs, file paths, directory rollups, file sizes, timestamps, and cleanup history in the local SQLite database.
- JSON scan reports and diagnostics exports when the user explicitly creates them.

## Data Not Collected

- No telemetry is sent from the app.
- No telemetry, cloud sync, or remote crash reporting is enabled.
- Google Drive access is used only when the user explicitly connects Drive and starts an offload batch.
- Scan data does not leave the device unless the user manually shares an exported report or diagnostics bundle.
- Selected file contents are sent to Google Drive only for files the user explicitly selects for offload.

## Cleanup Safety

WinClean's V1 cleanup flow only proposes conservative candidates and sends selected files to the Windows Recycle Bin. Permanent deletion is intentionally not exposed.

Google Drive offload verifies upload completion before moving the local original to the Windows Recycle Bin. If upload fails or is canceled, the local original is left in place.
