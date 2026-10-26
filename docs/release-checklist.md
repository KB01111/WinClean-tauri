# WinClean Release Checklist

## Local Gates

Run from the repository root:

```bash
bun run release:check
```

This verifies TypeScript, ESLint, Bun tests, frontend build, Rust tests, and a Tauri release executable build without bundling.

## Windows Packaging

- Confirm `src-tauri/tauri.conf.json` version, product name, identifier, publisher, icon set, and bundle targets.
- Build signed installer artifacts on the release machine.
- Verify installer launch, app startup, scan, cleanup preview, Recycle Bin cleanup using disposable test files, diagnostics export, report export, and uninstall.
- Verify Google Drive offload with a test OAuth client and disposable files: upload lands under `WinClean Offloads`, upload status completes, and local originals move to Recycle Bin only after verification.
- Keep the code-signing certificate outside the repository and inject signing through the release environment.
- Publish update metadata only after installer smoke testing passes.

## Manual Acceptance

- Scan `C:\` and cancel while scanning.
- Scan a small disposable fixture and verify largest directories, search, extension/category summaries, and report export.
- Move selected `.tmp` or `.log` fixture files to Recycle Bin and verify action history.
- Offload a large disposable file to Google Drive, cancel one batch before upload, and confirm canceled files remain local.
- Export diagnostics and confirm the bundle contains local health data only.
- Confirm `git status --short` contains only intentional source changes before tagging.

## WinClean 2.0 Hardware Acceptance (ASUS ROG Zephyrus G16)

These checks need the physical machine; the Linux/CI gates cannot cover them. Record results in the release notes.

- **Elevation**: enable the administrator helper (one UAC prompt). Confirm it refuses to start from a user-writable folder without the explicit confirmation, and exits when WinClean closes and after 10 idle minutes.
- **Services**: apply a recommended batch, confirm the read-back verification, then roll it back from History & Rollback. Confirm protected services (Defender, Windows Update, Windows Search, ASUS/AMD/NVIDIA) are not editable.
- **EcoQoS**: throttle a background process, confirm it moves to efficiency cores in Task Manager, restore it, and confirm VS Code/Cursor/cargo/the foreground window are never throttled while coding.
- **Battery**: compare reported watts with a wall-power meter and health with `powercfg /batteryreport`. Run one measurement (baseline → switch → finish) and confirm the verdict wording.
- **dGPU**: open Hardware & NPU repeatedly and confirm the RTX 4070 stays powered down (HWiNFO D3cold / Device Manager power data) while WinClean runs for 30 minutes.
- **NPU / Windows ML**: confirm XDNA 2 enumeration with the current ASUS/AMD driver and that no capability is shown as verified.
- **Mica and themes**: confirm Mica on Windows 11 ≥ 22621, the opaque fallback otherwise, and light/dark/system switching including the title bar.
- **G-Helper**: with G-Helper running, confirm automatic switching pauses and no ASUS setting is changed.
- **MCP**: on Windows build ≥ 26220 confirm `odr.exe mcp list`, the File Explorer metadata lookup, and that content/destructive tools stay blocked.
- **Idle cost**: record `winclean.exe` and `msedgewebview2.exe` CPU, memory and disk over 30 minutes idle on battery with the window minimised.
