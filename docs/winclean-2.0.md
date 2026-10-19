# WinClean 2.0 — Optimizer Suite

Extends WinClean 1.x (scan, cleanup, Google Drive offload/restore, AI Insights)
on the same Tauri 2 + Rust + React/TS stack. Design: `docs/superpowers/specs/2026-10-18-winclean-2.0-design.md`.

## Architecture

Rust modules in `src-tauri/src/optimizer/`:

| Module | Responsibility | Windows APIs |
|---|---|---|
| `policy.rs` | Protected services/processes, risk classes, EcoQoS decisions, power profiles, inference routing (pure, unit-tested) | — |
| `services.rs` | Service discovery, dependencies, start-type changes | SCM: `EnumServicesStatusExW`, `QueryServiceConfig(W/2W)`, `ChangeServiceConfig(W/2W)` |
| `processes.rs` | Process list, CPU/memory, EcoQoS apply/restore | ToolHelp snapshot, `GetProcessTimes`, `K32GetProcessMemoryInfo`, `SetProcessInformation(ProcessPowerThrottling)` |
| `power.rs` | Battery telemetry, schemes, discharge watts from real samples | `GetSystemPowerStatus`, `PowerGet/SetActiveScheme`, `PowerReadFriendlyName` |
| `hardware.rs` | CPU/adapters/NPU discovery | Registry only (Display + ComputeAccelerator classes), `GetSystemInfo`, `GlobalMemoryStatusEx` |
| `capabilities.rs` | Windows build, On-device Agent Registry, Windows ML EP DLL presence | Registry + filesystem existence checks |

SQLite adds `optimizer_snapshots`, `optimizer_audit`, `power_history`
(additive, `CREATE TABLE IF NOT EXISTS`). Frontend: `src/lib/optimizer.ts`
(mirrors `policy.rs`), views `services/processes/power/hardware/optimizer-history`.

## Safety boundaries

- Non-admin by default. SCM/scheme writes fail with an explicit elevation
  message; no silent escalation, no bundled helper in this release.
- Protected (never mutable): Defender, Windows Update, Windows Search,
  networking core, audio, PlugPlay, AMD/NVIDIA driver services, Copilot+.
  Enforced in Rust (`policy::classify_service`) and re-checked in commands.
- Every mutation: snapshot → pending audit → apply → verify by re-read →
  completed/failed audit. Rollback re-applies snapshots (Optimization History).
  Interrupted operations remain visible as `pending` rows.
- EcoQoS never terminates processes and never touches foreground apps,
  editors (VS Code/Cursor), Codex/Claude/OpenCode, compilers, games, or
  inference processes. Restore = `ControlMask 0` for the same PID.
  Enabling re-samples the target PID's live CPU (a busy updater/indexer is
  refused) and revalidates its image name. Power profiles only restore
  processes the audit trail shows WinClean throttled (never EcoQoS set by
  another app, and not limited to the 300 rows the UI lists); a reused PID
  is detected by process creation time. The processes a profile changed are
  stored in its snapshot, so rolling the profile back also undoes them.
- AI (existing `@openai/agents`) gains read-only tools only
  (`get_services`, `get_processes`, `get_power_status`, `get_hardware`).
  The AI layer cannot change services in this release: suggestions are
  advisory text only. `validateAiServiceSuggestions` is reserved for a
  future approval flow; the backend re-checks every direct UI mutation.

## ASUS G16 / RTX 4070 sleep policy

Hardware discovery reads registry strings only. No CUDA, NVAPI, DXGI
enumeration, WMI GPU queries, or periodic GPU polling anywhere in the app,
so a powered-off RTX 4070 is never woken. The dGPU appears in inference
routing only when Windows already reports it awake. G-Helper is detected
(read-only); WinClean never writes ASUS firmware, GPU-mode, fan, or
battery-limit state.

## Copilot+ / Windows ML / MCP status

- Supported: Windows 11 build ≥ 26100 + AMD XDNA driver (ASUS/AMD) for
  Copilot+ capability reporting.
- `VitisAIExecutionProvider`, `MIGraphXExecutionProvider`, DirectML and CPU
  providers are probed by DLL presence only. **NPU inference stays disabled**
  until an on-device Windows ML session test with a real model passes;
  routing order is NPU (verified) → Radeon 890M → awake RTX 4070 → CPU.
  Arbitrary LLMs are not expected to run on the NPU.
- MCP: On-device Agent Registry paths are discovered; File Explorer MCP
  stays behind Explorer's per-file consent. WinClean's native scanner is
  unchanged. Microsoft Learn MCP is configured for development time in
  `.mcp.json` and is not shipped in the app. A future Lumen agent connects
  via MCP; no second agent runtime is embedded.

## Validation status

Verified in CI/dev environment: Rust unit tests (policy, power, hardware),
TS tests (AI validation, EcoQoS restore, profiles, discharge watts),
typecheck, lint, production build, `tauri build --no-bundle`.

Requires the physical G16 (not verifiable in a generic Windows VM):
SCM start-type change + rollback with UAC, EcoQoS effect on a sleeping
RTX 4070 system, battery discharge-watt accuracy vs. design capacity,
XDNA NPU enumeration with the ASUS driver, G-Helper coexistence.
