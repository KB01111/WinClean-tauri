# WinClean Privacy Notice

WinClean is local-first. Scan data, file indexes, cleanup history, reports, and diagnostics are stored on the user's Windows device.

## Data Stored Locally

- Volume metadata needed to show disk capacity and filesystem type.
- Scan runs, file paths, directory rollups, file sizes, timestamps, and cleanup history in the local SQLite database.
- JSON scan reports and diagnostics exports when the user explicitly creates them.

## Data Not Collected

- No telemetry is sent from the app.
- No telemetry, cloud sync, or remote crash reporting is enabled.
- Google Drive access is used only when the user explicitly connects Drive and starts an offload or restore.
- Scan data does not leave the device unless the user manually shares an exported report or diagnostics bundle.
- Selected file contents are sent to Google Drive only for files the user explicitly selects for offload.

## Credentials

- The Google Drive refresh token and connected account email are stored in Windows Credential Manager, never in the SQLite database or on disk in plain text.
- The optional OpenAI API key is also stored in Windows Credential Manager and can be removed at any time from Settings.

## AI Insights (optional)

The AI assistant is disabled until the user adds their own OpenAI API key. When an analysis is requested, WinClean sends **file metadata only** — file paths, names, sizes, timestamps, category/extension summaries, and WinClean's own recommendation reasons — to OpenAI using the user's key. File contents are never read or transmitted. The assistant can only suggest candidates that WinClean's local rules already consider actionable, and nothing is cleaned or offloaded without an explicit user click.

## Cleanup Safety

WinClean's V1 cleanup flow only proposes conservative candidates and sends selected files to the Windows Recycle Bin. Permanent deletion is intentionally not exposed.

Google Drive offload verifies upload completion before moving the local original to the Windows Recycle Bin. If upload fails or is canceled, the local original is left in place.
