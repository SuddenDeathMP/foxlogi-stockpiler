# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project State

This repository is **pre-implementation**. As of now it contains only the `README.md`, `LICENSE`, and a `data/` directory of sample fixtures — there is no application source yet (no `package.json`, no `src-tauri/`, no frontend). The README describes the intended product; the sections below capture that intent so the first implementation matches it. When scaffolding, prefer `npm create tauri-app` conventions so the commands in the README work as written.

## What This App Does

Foxlogi Stockpiler is a Tauri desktop app that watches Foxhole's Unreal Engine `.sav` files and syncs extracted data to a remote server. The core data flow:

1. A native file watcher (`notify` crate) detects a modification to a watched `.sav` (its mtime changes).
2. The binary GVAS save is parsed to JSON via the `gvas` crate.
3. Specific fields are extracted from that JSON (currently intended to be hardcoded — see the roadmap note about making this configurable).
4. The extracted payload is POSTed to the configured server (`reqwest`).

The architecturally important consequence: the **pipeline lives in Rust** (watch → parse → extract → POST), and the frontend is deliberately thin — just an API-key field and a file picker. Avoid pulling pipeline logic into the JS layer.

## Stack & Layout Conventions

- **Tauri** (Rust backend + HTML/CSS/JS frontend, no frontend framework). Keep the frontend minimal.
- Rust backend lives under `src-tauri/` (standard Tauri layout) once scaffolded.
- Config is persisted with `tauri-plugin-store` to the OS app-config dir (e.g. `~/Library/Application Support/foxlogi-stockpiler/config.json` on macOS, `%APPDATA%\foxlogi-stockpiler\` on Windows). Note the README still references the old `ue-save-sync` name in some paths — use `foxlogi-stockpiler`.

## Commands

These come from the README and assume a standard Tauri scaffold:

```bash
npm install            # install JS deps
npm run tauri dev      # run in development
npm run tauri build    # production build → src-tauri/target/release/bundle/
```

Rust-side, expect the usual `cargo` commands inside `src-tauri/` (`cargo build`, `cargo test`, `cargo clippy`, `cargo fmt`). No test harness exists yet; when adding one, the `.sav`/JSON pairs in `data/` are the natural fixtures.

## `data/` Fixtures

`data/` holds real Foxhole save samples and their parsed output, useful as test inputs:

- `*.sav` — raw GVAS binary (Foxhole `MapData`, UE 4.24).
- `test*.json` — the JSON form produced by `gvas` parsing, each with a `header` (GVAS metadata: magic, engine version, custom versions) followed by the save properties. Field extraction logic should target the properties section, not the header.

## API Contract

Payloads POST to the server (endpoint configured at build time or via the settings file). The README documents the shape:

```http
POST /api/api/stockpile/bulk-update/
Authorization: Bearer YOUR_API_KEY
Content-Type: application/json

{ "filename": "...", "modified_at": "<ISO8601>", "data": { /* extracted fields */ } }
```

The API key is stored locally and sent only over HTTPS.
