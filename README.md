# WinClean

Windows-first disk analyzer and safe cleanup desktop app built with Tauri v2, React, TypeScript, SQLite, and a Rust command backend.

## Production V1

- Lists local Windows volumes with filesystem and capacity metadata.
- Scans a selected root on a background Rust thread and stores scan runs, file entries, directory rollups, and action history in SQLite.
- Avoids following reparse points, symlinks, and junctions during recursive traversal.
- Exposes scan history, directory browsing, file search, extension summaries, category summaries, and JSON report export.
- Plans conservative cleanup candidates from scan data and moves selected files to the Windows Recycle Bin.
- Separates Recover Space recommendations into Clean, Offload, and Review modes so large files can be uploaded to Google Drive only after explicit selection.
- Supports direct Google Drive offload with verified upload before local originals are moved to the Windows Recycle Bin.
- Keeps diagnostics local by default and exports a local support bundle only when the user chooses it.
- Records NTFS USN intent as a post-V1 acceleration path; V1 correctness is based on recursive scanning.

Google Drive offload requires a desktop OAuth client id supplied as `WINCLEAN_GOOGLE_CLIENT_ID`. No client secret is embedded in the app.

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

Use the full local release gate before packaging:

```bash
bun run release:check
```

The project uses Bun's hoisted linker through `bunfig.toml` so Node-based tools and Tauri child processes can resolve dependencies consistently on Windows.
