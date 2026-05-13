# WinClean

Windows-first storage cleaner and disk analyzer built with Tauri v2, React, TypeScript, and a Rust command backend.

## Milestone 1

- Lists local Windows volumes with best-effort filesystem and capacity metadata.
- Scans a selected root on a background Rust thread.
- Stores scan runs, file entries, directory rollups, and future action history in SQLite.
- Avoids following reparse points, symlinks, and junctions during recursive traversal.
- Records NTFS USN intent as an explicit fallback path until journal enumeration is implemented.
- Shows live scan counters and top directories by size in the desktop UI.

## Development

```bash
bun install
bun run typecheck
bun run lint
bun run build
cargo test --manifest-path src-tauri/Cargo.toml
node node_modules/@tauri-apps/cli/tauri.js build --no-bundle
```

The project uses Bun's hoisted linker through `bunfig.toml` so Node-based tools and Tauri child processes can resolve dependencies consistently on Windows.
