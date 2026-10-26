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
- Optional OpenAI and OpenRouter API keys are also stored in Windows Credential Manager (one entry per provider) and can be removed at any time from Settings. OpenCode signs in through its own command-line client; WinClean never sees or stores that credential.

## System optimizer (2.0)

- Service, process, battery, hardware and history data that WinClean reads for the optimizer stays on the device. Battery samples and the audit trail are stored in the local SQLite database; they are not transmitted.
- Optimizer changes (service start types, power profiles, EcoQoS) are recorded locally with the previous state so they can be rolled back.
- The administrator helper talks to WinClean over a private local pipe only. WinClean never opens a network connection on its behalf.
- WinClean can run as a local, read-only MCP server (`--mcp-stdio`) when you configure an agent to start it. Whatever that agent does with the diagnostics it reads is governed by that agent, not by WinClean.
- Windows MCP discovery (`odr.exe`) runs locally; WinClean calls only metadata tools of the File Explorer connector and never reads file contents through MCP.

## AI Insights (optional)

The AI assistant is disabled until the user picks a provider: OpenRouter (including its free models) or OpenAI with their own API key, the OpenCode command-line client installed on this PC, or a local model. Storage suggestions and duplicate detection also work with no AI at all ("Quick suggestions (no AI)").

When a *storage* analysis is requested, WinClean sends **file metadata only** — file paths, names, sizes, ages, WinClean's own recommendation reasons and totals, capped to roughly 16,000 characters in a single request — to the chosen provider. File contents are never read or transmitted. Control and direction-override characters are stripped from paths before they are sent. The assistant can only group candidates that WinClean's local rules already consider actionable; every suggestion is resolved against the scan by candidate id, re-sized and re-checked on disk by the backend (a model never supplies a path), and nothing is cleaned or offloaded without an explicit user click in Recover space.

- **OpenRouter** — requests go to `openrouter.ai` using the user's key. Free models are limited by OpenRouter (about 50 requests per day, 1,000 with credit) and the provider behind a free model may log prompts; do not analyse data you consider confidential with a free model.
- **OpenCode** — OpenCode's free models are offered to the OpenCode client only, so WinClean does not call them over HTTP. It runs the user's installed `opencode run` command with the storage summary on standard input, in an empty scratch folder inside the app data directory that holds an `opencode.json` asking OpenCode to deny every tool permission, and without auto-approving anything. OpenCode is a separate program running with your own Windows permissions: the scratch folder and the deny-all setting are requests WinClean makes of it, not a sandbox WinClean enforces, and they depend on OpenCode honouring them. What OpenCode sends where is governed by OpenCode and the chosen model's terms (OpenCode marks some free models as trial-only: no personal or confidential data).
- **OpenAI** — requests go to `api.openai.com` using the user's key.

A *system* analysis additionally sends, through read-only tools: service names, display names, start types and states; process names with CPU and memory figures; battery state (percentage, AC/DC, watts, health); CPU, GPU and NPU model names and driver versions; the available power profiles and WinClean's recent change history. Hardware discovery can include local agent-registry folder paths, which may contain the Windows profile name. File contents and file paths from your disks are not part of a system analysis.

**Local model option.** If you choose a local OpenAI-compatible server (Ollama, LM Studio, AMD Lemonade, Foundry Local), requests go only to a loopback address on this PC (`127.0.0.1`, `localhost` or `::1`); the app's content-security policy blocks any other host except `api.openai.com` and `openrouter.ai`. No API key is needed and nothing leaves the device.

The assistant can only *propose* changes. WinClean's deterministic rules re-validate every proposal and nothing is applied without an explicit click on the review dialog.

## Cleanup Safety

WinClean's cleanup flow only proposes conservative candidates and sends selected files to the Windows Recycle Bin. Permanent deletion is intentionally not exposed.

System folders, program folders and application data are never candidates. The only application-data locations WinClean cleans are a user's `AppData\Local\Temp` folder (only files untouched for at least 7 days, whichever rule matches; documents are never included) and `AppData\Local\CrashDumps`. Source-code projects, cloud-synced folders (OneDrive, Google Drive, Dropbox…), databases, virtual disks, mail stores, key material, documents and any folder the user adds under Settings → Protected folders are never touched. The duplicate finder only reads files when the user clicks "Compare file contents", within a 4 GB budget, at background priority, and stops when the PC goes onto battery; it never deletes anything.

Google Drive offload verifies upload completion before moving the local original to the Windows Recycle Bin. If upload fails or is canceled, the local original is left in place.
