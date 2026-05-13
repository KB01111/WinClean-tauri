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
- Keep the code-signing certificate outside the repository and inject signing through the release environment.
- Publish update metadata only after installer smoke testing passes.

## Manual Acceptance

- Scan `C:\` and cancel while scanning.
- Scan a small disposable fixture and verify largest directories, search, extension/category summaries, and report export.
- Move selected `.tmp` or `.log` fixture files to Recycle Bin and verify action history.
- Export diagnostics and confirm the bundle contains local health data only.
- Confirm `git status --short` contains only intentional source changes before tagging.
