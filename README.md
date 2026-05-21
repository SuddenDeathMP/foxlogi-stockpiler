# Foxlogi Stockpiler

A lightweight, cross-platform desktop application that monitors Foxhole game's Unreal Engine save files (`.sav`) for changes, converts them to JSON, extracts specific data, and sends it to a remote web server via API.


## Overview

Foxlogi Stockpiler watches one or more Unreal Engine `.sav` files for modifications. Whenever a save file is updated (its edit timestamp changes), the application automatically:

1. Detects the change
2. Parses the binary `.sav`
3. Extracts the relevant fields
4. Sends the data to your configured web server using an API key as JSON

The interface is intentionally minimal — just an API key field and a file picker. Set it up once and let it run in the background.

## Features

- **Cross-platform** — Single application runs on both macOS, Windows and Linux
- **Minimalistic UI** — Two inputs, no clutter
- **Real-time monitoring** — File changes detected instantly via native OS file watchers
- **Multiple files** — Monitor as many `.sav` files as you need simultaneously
- **Secure** — API key stored locally, transmitted only over HTTPS
- **Persistent config** — Remembers your settings between launches

## How It Works

```
┌──────────────────┐     ┌──────────────────┐     ┌──────────────────┐
│  .sav file       │────▶│  UE Save Sync    │────▶│  Your Web Server │
│  (Unreal Engine) │     │  (this app)      │     │  (API endpoint)  │
└──────────────────┘     └──────────────────┘     └──────────────────┘
        ▲                         │
        │                         │
   file modified            parse + extract
                            + POST as JSON
```

1. You provide an API key and select `.sav` files to watch
2. The app monitors file modification timestamps in the background
3. When a file changes, it's parsed from Unreal Engine's GVAS binary format into JSON
4. Specific data fields are extracted from the JSON
5. The extracted data is POSTed to your web server with the API key in the request headers

## Tech Stack

- **Framework:** [Tauri](https://tauri.app/) — lightweight cross-platform desktop framework
- **Backend:** Rust
  - `notify` — native file system watching
  - `gvas` — Unreal Engine save file parsing
  - `reqwest` — HTTP client for API calls
  - `serde_json` — JSON serialization
- **Frontend:** HTML / CSS / JavaScript (minimal, no framework needed)
- **Config storage:** `tauri-plugin-store`

## Installation

### Pre-built Binaries

Download the latest release from the [Releases page](#):

- **Windows:** `foxlogi-stockpiler-x.x.x-setup.exe`
- **macOS:** `foxlogi-stockpiler-x.x.x.dmg`
- **Linux:** `foxlogi-stockpiler-x.x.x.AppImage`

### Build from Source

**Prerequisites:**
- [Rust](https://rustup.rs/) (latest stable)
- [Node.js](https://nodejs.org/) (v18 or higher)
- Platform-specific Tauri dependencies — see [Tauri prerequisites](https://tauri.app/start/prerequisites/)

```bash
# Clone the repository
git clone https://github.com/yourusername/foxlogi-stockpiler.git
cd foxlogi-stockpiler

# Install dependencies
npm install

# Run in development mode
npm run tauri dev

# Build for production
npm run tauri build
```

The compiled installer will appear in `src-tauri/target/release/bundle/`.

## Usage

1. **Launch the app**
2. **Enter your API key** in the API Key field
3. **Click "Add Files"** and select one or more `.sav` files to monitor
4. **Minimize the window** — the app keeps running in the background and watches for changes
5. Whenever a watched `.sav` file is modified, the data is automatically sent to your server

### Configuration

The app stores its configuration locally, under the app identifier
(`com.foxlogi.stockpiler`):

- **Windows:** `%APPDATA%\com.foxlogi.stockpiler\config.json`
- **macOS:** `~/Library/Application Support/com.foxlogi.stockpiler/config.json`
- **Linux:** `~/.local/share/com.foxlogi.stockpiler/config.json`

## API Contract

Data is sent to your server as a POST request:

```http
POST /api/stockpile/bulk-update/ HTTP/1.1
Host: foxlogi.com
Content-Type: application/json
Authorization: Bearer YOUR_API_KEY

{
  "filename": "SaveGame_01.sav",
  "modified_at": "2026-05-20T14:30:00Z",
  "data": {
    // extracted fields from the .sav file
  }
}
```

### Server endpoint

The base URL is chosen at **build time** based on the build profile:

| Build | Default base URL |
|-------|------------------|
| Development (`npm run tauri dev`, any debug `cargo` build) | `http://127.0.0.1:3000` |
| Release (`npm run tauri build`) | `https://foxlogi.com` |

The full endpoint is the base URL plus `/api/stockpile/bulk-update/`.

> **Note:** under `npm run tauri dev`, syncs POST to `http://127.0.0.1:3000`, so
> they will fail unless you have a local server listening on port 3000. This is
> expected in development — use a release build (or run a local mock server) to
> exercise the real flow.

You can override the base URL at runtime via the `server_url` field in the
[config file](#configuration).

## Roadmap

- [ ] System tray icon and "minimize to tray" behavior
- [ ] Optional auto-launch on system startup
- [ ] Configurable field extraction (currently hardcoded)
- [ ] Local log viewer

## Contributing

Contributions are welcome. Please open an issue first to discuss any significant changes.


## License

This project is licensed under the [Apache-2.0 license](LICENSE).

## Acknowledgments

- [Tauri](https://tauri.app/) for the framework
- The Rust community for the excellent `notify` and `gvas` crates
- Unreal Engine save format reverse-engineering efforts by the community
