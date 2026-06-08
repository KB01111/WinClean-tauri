# AGENTS.md

## Cursor Cloud specific instructions

WinClean is a **Tauri v2** desktop app (React/Vite frontend + Rust backend). See `README.md` for standard dev commands.

### Services

| Service | Port | Start command |
|---------|------|---------------|
| Vite dev server | 1420 | Started automatically by `bun run tauri dev` (or `bun run dev` alone) |
| Tauri desktop app | — | `bun run tauri dev` |

There is no separate API server or Docker stack. SQLite is embedded in the Rust backend.

### Package managers

- **Bun** — JS/TS dependencies and frontend tests (`bun install`, `bun run test`)
- **Cargo** — Rust backend (`cargo test --manifest-path src-tauri/Cargo.toml`)

Bun uses a hoisted linker via `bunfig.toml` for Tauri child-process compatibility.

### One-time Linux system dependencies (Tauri GUI builds)

On Debian/Ubuntu, install Tauri v2 prerequisites before `bun run tauri dev` or `tauri build`:

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev pkg-config libsoup-3.0-dev
```

### Rust toolchain

The lockfile pulls crates that require **Rust stable ≥ 1.85** (edition 2024). If `cargo test` fails with an `edition2024` error, run `rustup default stable` once per VM.

### Linux vs Windows behavior

This app is **Windows-first**. On Linux cloud VMs:

- The UI and most Rust unit/integration tests run normally.
- Volume enumeration, Recycle Bin cleanup, and Windows Credential Manager features use stubs or limited behavior.
- One integration test (`cleanup_execution_reports_missing_and_changed_candidates_as_skipped`) may fail because it uses Windows temp paths (`C:\tmp\...`). This is expected off Windows.

Full Windows behavior (volumes, scan, Recycle Bin, Google Drive offload) requires a Windows dev machine plus `WINCLEAN_GOOGLE_CLIENT_ID` for Drive features.

### Common commands

| Task | Command |
|------|---------|
| Install JS deps | `bun install` |
| Typecheck | `bun run typecheck` |
| Lint | `bun run lint` |
| Frontend tests | `bun run test` |
| Frontend build | `bun run build` |
| Rust tests | `cargo test --manifest-path src-tauri/Cargo.toml` |
| Dev (GUI) | `bun run tauri dev` |
| Full release gate | `bun run release:check` |

### Running the dev app in tmux

Use a named tmux session for long-running dev servers:

```bash
SESSION_NAME="winclean-tauri-dev"
tmux -f /exec-daemon/tmux.portal.conf new-session -d -s "$SESSION_NAME" -c /workspace -- "${SHELL:-zsh}" -l
tmux -f /exec-daemon/tmux.portal.conf send-keys -t "$SESSION_NAME:0.0" 'bun run tauri dev' C-m
```

The Vite URL is `http://127.0.0.1:1420/`. Requires `DISPLAY` (or Wayland) for the native window.
