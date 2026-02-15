# EGUI Version Switch Guide

## Branch info
- EGUI branch: `feature/egui-version`
- Current branch includes the full migration from Tauri WebView UI to egui UI.
- Pre-migration checkpoint: `checkpoint/pre-egui-20260214-233917` (same commit as `backup/pre-egui-20260214-233917`).

## Switch to EGUI branch
```bash
git fetch origin
git checkout feature/egui-version
git pull --ff-only origin feature/egui-version
```

If the remote branch does not exist yet, create and push it once:
```bash
git checkout feature/egui-version
git push -u origin feature/egui-version
```

## Confirm you are on EGUI version
```bash
git branch --show-current
git log --oneline -n 5
```

Expected branch name is `feature/egui-version`.

## Run the app (Tauri backend + egui UI)
```bash
pnpm install
pnpm tauri dev
```

## Roll back to pre-EGUI version
```bash
git checkout checkpoint/pre-egui-20260214-233917
```

Or create a local branch from pre-EGUI checkpoint:
```bash
git checkout -b feature/pre-egui checkpoint/pre-egui-20260214-233917
```
