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

## System optimizer (2.0)

- Service, process, battery, hardware and history data that WinClean reads for the optimizer stays on the device. Battery samples and the audit trail are stored in the local SQLite database; they are not transmitted.
- Optimizer changes (service start types, power profiles, EcoQoS) are recorded locally with the previous state so they can be rolled back.
- The administrator helper talks to WinClean over a private local pipe only. WinClean never opens a network connection on its behalf.
- WinClean can run as a local, read-only MCP server (`--mcp-stdio`) when you configure an agent to start it. Whatever that agent does with the diagnostics it reads is governed by that agent, not by WinClean.
- Windows MCP discovery (`odr.exe`) runs locally; WinClean calls only metadata tools of the File Explorer connector and never reads file contents through MCP.

## AI Insights (optional)

The AI assistant is disabled until the user adds their own OpenAI API key or configures a local model. When a *scan* analysis is requested, WinClean sends **file metadata only** — file paths, names, sizes, timestamps, category/extension summaries, and WinClean's own recommendation reasons — to OpenAI using the user's key. File contents are never read or transmitted. The assistant can only suggest candidates that WinClean's local rules already consider actionable, and nothing is cleaned or offloaded without an explicit user click.

A *system* analysis additionally sends, through read-only tools: service names, display names, start types and states; process names with CPU and memory figures; battery state (percentage, AC/DC, watts, health); CPU, GPU and NPU model names and driver versions; the available power profiles and WinClean's recent change history. Hardware discovery can include local agent-registry folder paths, which may contain the Windows profile name. File contents and file paths from your disks are not part of a system analysis.

**Local model option.** If you choose a local OpenAI-compatible server (Ollama, LM Studio, AMD Lemonade, Foundry Local), requests go only to a loopback address on this PC (`127.0.0.1`, `localhost` or `::1`); the app's content-security policy blocks any other host except `api.openai.com`. No API key is needed and nothing leaves the device.

The assistant can only *propose* changes. WinClean's deterministic rules re-validate every proposal and nothing is applied without an explicit click on the review dialog.

## Cleanup Safety

WinClean's V1 cleanup flow only proposes conservative candidates and sends selected files to the Windows Recycle Bin. Permanent deletion is intentionally not exposed.

Google Drive offload verifies upload completion before moving the local original to the Windows Recycle Bin. If upload fails or is canceled, the local original is left in place.
