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
