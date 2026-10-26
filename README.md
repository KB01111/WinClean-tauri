# WinClean

Windows-first disk analyzer, safe cleanup tool and adaptive Windows 11 optimization suite, built with Tauri v2, React, TypeScript, SQLite and a Rust backend. WinClean 2.0 is tuned for the ASUS ROG Zephyrus G16 (Ryzen AI 9 HX 370, Radeon 890M, RTX 4070, XDNA 2 NPU) and degrades gracefully elsewhere.

## WinClean 2.0 — optimization suite

Full design, security model and validation status: [`docs/winclean-2.0.md`](docs/winclean-2.0.md).

- **Windows Services** — native Service Control Manager discovery, dependency-aware classification (protected / caution / safe), deterministic recommendations, and a prepare → review → approve → apply flow with a snapshot, read-back verification and one-click rollback. Defender, Windows Update, Windows Search, networking, drivers, ASUS/AMD/NVIDIA and Copilot+ components can never be changed.
- **Administrator helper** — the app itself stays unprivileged. One UAC prompt starts a narrow helper (one request type: change a service start type) that re-validates every request on its own.
- **Processes & EcoQoS** — reversible per-process efficiency mode for background work; the foreground window, editors, AI coding tools, compilers, games and AI inference are never throttled.
- **Battery & Power** — real discharge watts and battery health from the battery's own IOCTLs (never estimated), runtime, history chart, five profiles plus custom profiles, optional event-driven automatic switching, and an honest before/after measurement with a confidence interval.
- **Hardware & NPU** — Zen 5/5c topology, Radeon 890M, RTX 4070 power state (read from Windows' cached PnP data — no GPU API is ever called, so the dGPU is never woken), XDNA 2 NPU, G-Helper coexistence (WinClean never writes ASUS firmware, fan, GPU-mode or battery-limit state).
- **Windows ML readiness** — layered execution-provider assessment (VitisAI, MIGraphX, DirectML, TensorRT-RTX, CPU) and a static ONNX model analyzer. No inference is run, so nothing is ever labelled "verified".
- **MCP Integrations** — Windows on-device MCP discovery with a read-only tool policy for File Explorer metadata, and WinClean as a read-only MCP server (`winclean.exe --mcp-stdio`) for agents such as Lumen.
- **AI Insights** — the existing OpenAI Agents SDK assistant gains read-only system tools and structured proposals that are validated by deterministic code and applied only after your explicit approval. Works with OpenAI or a local OpenAI-compatible model (Ollama, LM Studio, AMD Lemonade, Foundry Local).
- **History & Rollback** — every change is audited and snapshotted; interrupted changes are recovered and shown, never hidden.
- Windows 11 look and feel: grouped navigation, Mica backdrop with an opaque fallback, native theme sync, reduced-motion support.

## Storage — Production V1 (unchanged)

- Lists local Windows volumes with filesystem and capacity metadata.
- Scans a selected root on a background Rust thread and stores scan runs, file entries, directory rollups, and action history in SQLite.
- Avoids following reparse points, symlinks, and junctions during recursive traversal.
- Exposes scan history, directory browsing, file search, extension summaries, category summaries, and JSON report export.
- Plans conservative cleanup candidates from scan data and moves selected files to the Windows Recycle Bin.
- Separates Recover Space recommendations into Clean, Offload, and Review modes so large files can be uploaded to Google Drive only after explicit selection, with size/age filters.
- Supports direct Google Drive offload with chunked resumable uploads (retry with exponential backoff, mid-upload cancel, live byte progress) and checksum-verified upload before local originals are moved to the Windows Recycle Bin. Incomplete or corrupt Drive copies are deleted automatically.
- Shows the connected Google account, keeps an offload batch history with per-item Drive links, and can restore offloaded files back from Drive with size/MD5 verification.
- Rehydrates a running offload batch after an app restart and marks interrupted batches in history.
- Surfaces storage hogs: largest directories with drill-down plus large files untouched for 6+ months.
- Optional AI insights: bring your own OpenAI API key (stored in Windows Credential Manager), or use a local model on this PC, and an OpenAI Agents SDK agent analyzes scan metadata and suggests what to clean or offload. Only file metadata (paths, sizes, dates) is sent — never file contents.
- Light, dark, and system themes.
- Keeps diagnostics local by default and exports a local support bundle only when the user chooses it.
- Records NTFS USN intent as a post-V1 acceleration path; V1 correctness is based on recursive scanning.

Google Drive offload requires a desktop OAuth client id supplied as `WINCLEAN_GOOGLE_CLIENT_ID`. No client secret is embedded in the app.

### Restoring offloaded files

Every offload batch is listed under History → Offload batches. Each uploaded item keeps its Drive link and a **Restore** button that downloads the file from Google Drive, verifies its size and MD5 checksum, and puts it back at its original path (the restore fails safely if a file already exists there).

## Development

```bash
bun install
bun run typecheck
bun run lint
bun run test
bun run build
cargo test --manifest-path src-tauri/Cargo.toml
node node_modules/@tauri-apps/cli/tauri.js build --no-bundle
```

Windows-only Rust code can be type-checked from Linux or macOS against the real `windows-sys` bindings:

```bash
rustup target add x86_64-pc-windows-gnu   # plus mingw-w64 (e.g. apt install gcc-mingw-w64-x86-64)
cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-gnu --lib --tests
```

One integration test (`cleanup_execution_reports_missing_and_changed_candidates_as_skipped`) uses `C:\tmp` and only passes on Windows.

Use the full local release gate before packaging:

```bash
bun run release:check
```

The project uses Bun's hoisted linker through `bunfig.toml` so Node-based tools and Tauri child processes can resolve dependencies consistently on Windows.
