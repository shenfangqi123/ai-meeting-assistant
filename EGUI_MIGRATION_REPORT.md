# EGUI Migration Report

This document records the completed `phase1 -> phase6` migration work and self-check results.

## Scope

- Removed legacy URL loading flow from the original project UI and backend commands.
- Migrated primary runtime UI path from WebView-driven split layout to native `egui`.
- Kept Tauri as hidden host/runtime container for existing services and state.

## Phase 1 - Remove URL Flow + Event Bus Base

### Changes

- Removed URL input/button from `index.html`.
- Removed URL navigation handlers from `src/main.js`.
- Removed backend commands that opened external URL windows from `src-tauri/src/main.rs`.
- Added shared UI event bus module `src-tauri/src/ui_events.rs`.

### Self-check

- `pnpm build`: passed.
- `cargo check` (in `src-tauri`): passed.

## Phase 2 - Egui Shell Bootstrapping

### Changes

- Added `eframe` dependency.
- Added `src-tauri/src/egui_app.rs` as native UI shell.
- Wired Tauri setup to launch egui and hide Tauri main window.

### Self-check

- `cargo check` (in `src-tauri`): passed.

## Phase 3 - Real-time Output Migration

### Changes

- Connected audio/live events to the shared `ui_events` bus.
- Subscribed in egui and rendered:
  - live partial/final text
  - segment list updates
  - translation stream chunks

### Self-check

- `cargo check` (in `src-tauri`): passed.

## Phase 4 - Control Panel + Project + RAG in Egui

### Changes

- Added egui controls for:
  - start/stop capture
  - clear segments
  - ASR provider/fallback/language
  - translate provider
- Added project management in egui:
  - reload/list/select
  - create + index
  - sync selected
  - delete selected
- Added RAG ask flow in egui.
- Added direct helper functions in `src-tauri/src/rag/mod.rs` for non-`State` usage.

### Self-check

- `cargo check` (in `src-tauri`): passed.

## Phase 5 - Remove WebView Layout Path

### Changes

- Removed split-layout child WebView path and related resize/layout command handling.
- Simplified output emission to event bus (no output webview dependency).
- Updated `src-tauri/tauri.conf.json` main window with `"visible": false`.
- Removed unused `url` crate dependency.

### Self-check

- `cargo check` (in `src-tauri`): passed.

## Phase 6 - Final Validation

### Final self-check commands

- `pnpm build`: passed.
- `cargo check` (in `src-tauri`): passed.
- `cargo check --release` (in `src-tauri`): passed.

### Notes

- There are existing compile warnings unrelated to migration correctness (mostly dead code in RAG/embedding utility paths).
- Legacy web assets still exist in repository for rollback/reference, but default runtime UI path is now egui.
