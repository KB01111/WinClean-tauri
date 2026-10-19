# WinClean 2.0 — Adaptive Windows 11 Optimization Suite

WinClean 2.0 keeps everything from 1.x (disk scan, cleanup, Google Drive
offload/restore, SQLite history, AI Insights) and adds a native optimizer for
Windows services, background processes, power, hardware and local AI. It runs
on the same stack: Tauri 2, Rust, React/TypeScript, shadcn/ui, Tailwind.

It is tuned for the ASUS ROG Zephyrus G16 (Ryzen AI 9 HX 370, Radeon 890M,
RTX 4070 Laptop GPU, XDNA 2 NPU) but every feature degrades gracefully on other
Windows 11 machines.

Verified Microsoft platform facts that the design rests on — and the places
where the first 2.0 baseline was wrong and was corrected — are in
[`windows-api-research.md`](windows-api-research.md).

## Design principles

1. **Never invent data.** Watts come from the battery's own report; when a
   battery does not report them the UI shows "—". NPU inference is never
   labelled "verified" because WinClean never runs an inference session.
2. **The app is unprivileged.** Only a tiny helper is ever elevated, and it can
   do exactly one thing.
3. **One gate per mutation.** Every change — manual, automatic or proposed by an
   AI — passes the same policy check, snapshot, audit and verification.
4. **Reversible by default.** Snapshots for every change, one-click rollback,
   crash recovery for changes that were interrupted.
5. **Idle means idle.** Event-driven where Windows offers events, otherwise a
   slow adaptive timer, and nothing polls while the window is hidden.
6. **Do not wake the RTX 4070.** No GPU API is called anywhere. A test enforces it.

## Architecture

```
React UI ──invoke──▶ optimizer::commands (thin adapters)
   ▲                        │
   │ optimizer-event        ▼
   │            orchestrator ─ policy ─ service_plan ─ service_graph      (pure, tested everywhere)
   │                 │
   │      ┌──────────┼───────────────┬──────────────┐
   │      ▼          ▼               ▼              ▼
   │   backend    recovery       profiles       measure / auto_switch
   │   traits        │
   │   ┌──┴───────┐  └── SQLite (snapshots, audit, settings, telemetry)
   │   ▼          ▼
   │ Windows   fakes (tests)
   │   │
   │   ├─ services.rs   SCM (read-only in the app)
   │   ├─ processes.rs  ToolHelp, SetProcessInformation (EcoQoS)
   │   ├─ battery.rs    IOCTL_BATTERY_*   telemetry.rs (adaptive sampling)
   │   ├─ power.rs      PowrProf schemes  power_events.rs (event sources)
   │   └─ hardware.rs / topology.rs       (no GPU API)
   │
   └─ runtime.rs  one background thread: condvar wake + slow timer
```

Every Windows effect sits behind a trait in `backend.rs` (`ServiceBackend`,
`ProcessBackend`, `PowerBackend`). Production uses the Windows implementations;
tests use in-memory fakes, which is why service protection, rollback, EcoQoS
restoration, profile switching, elevation and recovery are all tested on Linux.

| Module | Responsibility |
|---|---|
| `policy.rs` | Protected services and processes, risk classes, built-in profiles, custom-profile validation. Pure. |
| `service_graph.rs` | Reverse dependencies, protection closure, "would this break something running". Pure. |
| `service_plan.rs` | `validate_service_plan` and the recommendation engine. Pure. |
| `services.rs` | SCM enumeration/config/dependencies. Writes only run in the helper or an already-elevated process. |
| `privileged.rs` | `guarded_set_start_type` — the single authoritative write gate (used by the helper and the in-process elevated path). |
| `helper_protocol.rs` / `helper_client.rs` / `helper_windows.rs` | Elevated-helper protocol, manager and named-pipe transport. |
| `orchestrator.rs` | Transactional operations: policy → preflight → snapshot → pending audit → apply → verify → completed audit; all-or-nothing batches; approval tokens. |
| `recovery.rs` | Reconciles `pending` audit rows left by a crash. |
| `processes.rs` / `ecoqos.rs` / `workload.rs` | Process list, EcoQoS apply/restore with an ownership ledger, workload classification. |
| `battery.rs` / `telemetry.rs` / `power.rs` / `power_events.rs` | Battery IOCTLs, adaptive sampling, power schemes, event sources. |
| `profiles.rs` / `auto_switch.rs` / `measure.rs` / `runtime.rs` | Profile catalogue, automatic switching decision, A/B measurement, background loop. |
| `hardware.rs` / `topology.rs` | CPU topology, adapters, NPU, dGPU power state, G-Helper detection. |
| `winml.rs` / `onnx_model.rs` / `capabilities.rs` | Windows ML readiness assessment, static ONNX inspection, routing. |
| `mcp.rs` / `mcp_server.rs` | Windows MCP discovery/client with a tool policy; WinClean as a read-only MCP server. |
| `commands.rs` | Tauri command adapters. |

SQLite additions (`db_optimizer.rs`, all `CREATE TABLE IF NOT EXISTS`):
`optimizer_snapshots`, `optimizer_audit`, `power_history`, `power_telemetry`,
`optimizer_settings`, `power_measurements`. Existing 1.x tables are untouched.

## 1. Windows service optimizer

- Native SCM discovery with `windows-sys` (no shell-outs): configuration,
  start type incl. delayed auto-start, trigger-start flag, dependencies,
  dependents, and the working set for services that own their process.
- Classification in `policy.rs`: **protected / caution / safe**, with a category
  and a reason. Protected means *never changeable*: Defender, Windows Update,
  Windows Search, networking, audio, PnP/RPC core, security, drivers, ASUS
  (ATK/Armoury/ROG), AMD, NVIDIA, Realtek, Intel, Bluetooth, Copilot+/AI.
  Protection extends to the **dependency closure** (a service that a protected
  service needs is protected). Unknown services can never be disabled, and a
  disable is refused while a running dependent would lose its dependency.
- Recommendations are deterministic (`service_plan.rs`), curated per category,
  and never include protected services. Version-suffixed updater services (for
  example `GoogleUpdaterService147.0.7704.0`) are matched by prefix.
- Plans: the UI (or an AI) submits a *requested* change set;
  `optimizer_prepare_service_plan` returns what the policy actually allows, what
  was refused and why, a fingerprint, and a **single-use approval token** bound to
  that exact plan. `optimizer_apply_service_plan(token)` applies exactly it. If
  the live services changed in between, the backend answers `conflict` and the
  UI re-reviews instead of applying a stale plan.
- Apply = snapshot → pending audit → write → verify by re-reading the SCM →
  completed audit. A failure part-way rolls the already-applied steps back.
- Changing a start type never stops a running service.

## 2. Processes and EcoQoS

- `SetProcessInformation(ProcessPowerThrottling)` with the documented EcoQoS
  state mask; restore writes `ControlMask = 0` (the system-managed default),
  never an explicit "HighQoS" pin.
- WinClean throttles only what the policy marks **eligible**. Never throttled:
  the foreground window and its whole process tree, VS Code, Cursor, Codex,
  Claude Code, OpenCode, Rust toolchain (cargo/rustc/rust-analyzer/…), Node, games,
  active AI inference, and system/security processes.
- **Ownership ledger** (`ecoqos.rs`): derived from completed audit rows. Restore
  only touches processes WinClean throttled — never EcoQoS set by another app —
  and a PID reused by a newer process (checked by creation time) is left alone.
- Modes in a profile: **Off**, **Conservative** (clearly idle background work),
  **Battery** (wider). The workload detector classifies coding / gaming / AI
  inference / AI development / general with the evidence shown in the UI.

## 3. Battery and power

- Telemetry from `IOCTL_BATTERY_QUERY_STATUS` (`Rate` in mW) and
  `BATTERY_INFORMATION` (health = full-charge ÷ design capacity, cycle count).
  Batteries reporting *relative* units produce no watts at all.
- Robust statistics: trimmed-mean average discharge power, an energy-delta
  cross-check, runtime from measured draw (falls back to the Windows estimate,
  labelled as such).
- History is stored (`power_telemetry`), bucketed for an SVG chart, and the AC
  stretches are shaded rather than drawn as measurements.
- Profiles: **Battery Saver, Balanced, Coding, Gaming, AI Development** plus up
  to a capped number of custom profiles (a Windows power scheme + EcoQoS mode).
- **Automatic switching** (`auto_switch.rs`, off by default): rules for AC, DC,
  low battery %, gaming, AI development and coding; minimum dwell time, hysteresis,
  a pause after a manual choice, and a pause while G-Helper is running.
  The decision is a pure function; the runtime feeds it events.
- **Measurement** (`measure.rs`): record real discharge samples, apply a profile,
  record again. Only discharge samples count; the first minute after the switch is
  discarded; samples are averaged per minute (consecutive readings are
  correlated); the verdict is *improved/worse* only if the difference exceeds its
  95% confidence interval (Welch), otherwise *inconclusive*, with workload
  mismatches flagged. WinClean never claims a saving from theory.

## 4. ASUS G16 hardware

- CPU topology from `GetLogicalProcessorInformationEx` (efficiency-class
  clusters, L3 domains). "Zen 5 + Zen 5c" is named only when the observed layout
  (4 + 8 cores, 16 MB + 8 MB L3) actually matches.
- Adapters from the registry; classified iGPU / dGPU / other.
- NPU generation from the PCI ID (`VEN_1022 & DEV_17F0` = XDNA 2); driver version
  from the driver store.
- **dGPU power state** comes from the PnP manager's cached power data (the same
  data Device Manager shows) — no device I/O. `hardware.rs` documents what is
  deliberately absent: CUDA, NVAPI/NVML, DXGI enumeration, Direct3D devices, WMI
  `Win32_VideoController`, periodic GPU polling. A static test scans the optimizer
  sources and `Cargo.toml` for those APIs and dependencies and fails if any
  appears.
- Everything is lazy (nothing runs until the Hardware view or the AI tool asks)
  and cached.
- **G-Helper**: detected read-only. WinClean never writes ASUS firmware, fan,
  GPU-mode or battery-limit state, and automatic profile switching pauses while
  G-Helper is running.

## 5. Microsoft Copilot+ / MCP

- Windows MCP (preview, build ≥ 26220): discovery through `odr.exe mcp list`.
  Every failure is reported verbatim; nothing is assumed.
- **Tool policy** (`mcp::check_call`): only the File Explorer connector's
  *metadata* tools are callable. Content-reading tools, every destructive tool and
  every unknown tool are blocked; paths must be plain absolute Windows paths.
  Windows applies its own consent prompts on top.
- WinClean's **native scanner is unchanged** — MCP is never used to scan or delete.
- **WinClean as an MCP server** (`winclean.exe --mcp-stdio`): seven read-only
  tools, each annotated `readOnlyHint: true`; a test asserts no tool is anything
  else. Example client configuration for a future Lumen agent:

  ```json
  { "mcpServers": { "winclean": { "command": "C:\\Program Files\\WinClean\\winclean.exe", "args": ["--mcp-stdio"] } } }
  ```

- **Microsoft Learn MCP** (`.mcp.json`, `.vscode/mcp.json`) is a development-time
  tool for checking Windows API documentation. It is not bundled into the app.

## 6. AMD NPU and Windows ML

- `winml.rs` assesses each execution provider layer by layer — OS, hardware,
  driver, package, runtime, model — with `pass / fail / unknown / notRun`.
  The final layer (an actual inference session) is **always `notRun`**.
- Providers assessed: VitisAI (NPU), MIGraphX (Radeon; not suggested for
  generative-LLM-shaped models), DirectML (Microsoft lists it as legacy), NVIDIA
  TensorRT-RTX (excluded while the dGPU is powered down unless the user consents),
  and CPU.
- Routing: on battery NPU → iGPU → CPU; on AC iGPU → NPU → CPU. The RTX 4070 is
  never a candidate while it is asleep.
- `onnx_model.rs` inspects a user-selected `.onnx` file statically (streaming
  protobuf reader, bounded allocations): opset, operators, quantisation, dynamic
  shapes, external data, LLM-likeness; and says which providers it plausibly fits.
  "Good" is a structural match, not a benchmark.
- Copilot+ status is reported **"by spec"** (hardware class and OS build), never as
  a measured TOPS figure.

## 7. AI Optimization Assistant

- Extends the existing OpenAI Agents SDK integration. The assistant has **nine
  read-only tools** (services, recommendations, processes, workload, battery,
  hardware, capabilities, profiles, history). A test asserts the tool list and that
  the tool source calls only read commands.
- It can only **propose**: structured output with findings, service proposals and
  an optional profile proposal. Proposals are filtered in the UI, validated again by
  the backend policy (same code path as manual changes, origin `ai`), shown in the
  same review dialog, and applied only after the user's explicit click.
- **Local inference**: any OpenAI-compatible server on this PC (Ollama, LM Studio,
  AMD Lemonade, Foundry Local). The endpoint must be on loopback; the CSP only
  allows `api.openai.com` and loopback hosts.
- The heavy SDK is loaded lazily when the AI view is opened.

## 8. User interface

Grouped navigation: **System** (System Overview) · **Storage and Cleanup**
(Disk overview, Explorer, Recover space) · **Optimize** (Windows Services,
Processes & EcoQoS, Battery & Power, Hardware & NPU, Optimization Profiles) ·
**Assistant** (AI Insights, MCP Integrations) · **Records** (History & Rollback,
Storage history, Settings).

- System Overview lists what is worth doing next from plain rules
  (`system-overview.ts`) — each suggestion states its exact reason.
- Windows 11 look: Segoe UI Variable stack, Mica via `window.set_effects`
  (build ≥ 22621) with an opaque fallback (`html[data-mica]`), native theme sync
  (`window.setTheme`), reduced-motion support, thin scrollbars, keyboard-reachable
  controls, `aria-current` navigation, text alternatives for the chart.
- `App.tsx` is a thin shell; state lives in `use-storage` / `use-run-data` and the
  optimizer hooks. Polling is visibility-aware (`use-polling`), fast battery
  sampling is a lease the Battery view renews while visible (`use-telemetry-lease`),
  and live changes arrive as `optimizer-event`s.

## 9. Performance and idle cost

| Mechanism | Detail |
|---|---|
| Background thread | One, sleeping on a condition variable. Wakes for power-setting callbacks, foreground-window changes, and a slow timer. |
| Battery sampling | Pure function `next_interval`: 300 s on AC idle, 60 s on battery, 30 s at ≤ 20 %, 15 s / 5 s while the Battery view is open (AC / DC), 10 s during a measurement. Budget tests cap samples per hour (12 / 60 / 120 / 720). |
| Telemetry writes | Only when something changed or the timer fired; cached `BATTERY_INFORMATION`. |
| UI polling | Only while the window is visible; stops on hide, refreshes on show. Process table 10 s, Battery 5 s, Overview 15 s. |
| Hardware / services | Lazy, cached, no background refresh. |
| Frontend bundle | The AI SDK is a lazily loaded chunk; the startup bundle is ~478 kB (146 kB gzip) instead of ~1.2 MB. |

## 10. Security and recovery

- **Elevation**: the same executable is re-launched with `--winclean-helper` via
  UAC. The app is the pipe *server* (random name, `FIRST_PIPE_INSTANCE`, protected
  DACL for the current user + Administrators, remote clients rejected, only the
  PID it just launched accepted). The helper is the *client* and connects only to a
  server that is its parent and runs the identical image. It exits when the parent
  exits or after 10 idle minutes. If WinClean runs from a user-writable folder
  (development, portable) the user must confirm explicitly.
- **Protocol**: `hello`, `ping`, `setStartType`, `shutdown` — no command, path,
  registry or file operation. Frames are length-prefixed JSON, ≤ 16 KiB,
  `deny_unknown_fields`.
- **The helper trusts nothing**: it re-reads the SCM and re-runs the complete
  protection policy (`guarded_set_start_type`) before every write, then verifies.
- **Approval tokens** are single-use, expire, and are bound to a plan fingerprint.
- **Audit** rows are written before the change (`pending`) and completed after; on
  start `recovery.rs` classifies leftovers (completed / interrupted) without
  changing anything, and the UI surfaces them. Rollback of a snapshot is verified
  and itself audited; a partial rollback stays retryable.
- **AI and MCP cannot bypass policy**: the assistant has no mutating tool; the MCP
  server has no mutating tool; the MCP client only calls allow-listed metadata
  tools.
- Never auto-disabled: Defender, Windows Update, Windows Search, critical
  networking, AMD/NVIDIA drivers (protected by name, vendor prefix and dependency
  closure).

## Testing

| Layer | Command |
|---|---|
| TypeScript | `bun run typecheck`, `bun run lint`, `bun run test` |
| Rust | `cargo test --manifest-path src-tauri/Cargo.toml` |
| Windows-only code compiles | `cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-gnu --lib --tests` |
| Full gate | `bun run release:check` |

Covered by tests: service protection (names, prefixes, dependency closure, unknown
services, active dependents), plan validation and stale-plan conflicts, elevation
boundaries (the helper rejects protected and vendor services on its own), rollback
(single, batch, partial), crash recovery, EcoQoS ownership and restoration
(PID reuse, foreign EcoQoS), profile switching and auto-switch rules (dwell,
hysteresis, manual override, G-Helper pause), battery math and measurement
statistics, GPU fallback and the no-GPU-API guard, monitoring overhead budgets,
MCP tool policy and the stdio server, ONNX parsing on hostile input, and the
frontend helpers, proposal gate and navigation model.

## Validation status

Verified here (Linux CI environment):

- All TypeScript and Rust tests above; the Windows-only Rust code is type-checked
  against the real `windows-sys` bindings by cross-compiling to
  `x86_64-pc-windows-gnu`.
- All 14 views render against a mocked backend in headless Chromium without
  console errors, the review/approve flow works, protected services are not
  editable, there is no horizontal overflow at 1080 px, and no IPC call is made
  while the window is hidden.

**Not verifiable without the physical G16 / Windows 11 — please validate:**

| Area | What to check |
|---|---|
| UAC and helper | One UAC prompt; helper starts, is rejected from a user-writable folder without confirmation, exits with the app and after 10 idle minutes. |
| SCM writes | Start-type change on a safe service, verification, rollback; protected services refused. |
| EcoQoS | Effect on efficiency cores in Task Manager; restore; foreground tree untouched while coding. |
| Battery IOCTL | Reported watts vs. a wall-power meter; health vs. `powercfg /batteryreport`. |
| dGPU | RTX 4070 stays in D3cold (HWiNFO / Device Manager power data) while WinClean runs and the Hardware view is opened repeatedly. |
| XDNA 2 | NPU enumeration with the ASUS/AMD driver; Windows ML package state. |
| Mica | Backdrop on build ≥ 22621, opaque fallback otherwise. |
| G-Helper | Coexistence; auto-switch pauses while it runs. |
| MCP | `odr.exe` on build ≥ 26220 and File Explorer MCP tools (preview feature). |
| Idle cost | Task Manager CPU/memory/disk of `winclean.exe` and `msedgewebview2.exe` over 30 min idle on battery. |
