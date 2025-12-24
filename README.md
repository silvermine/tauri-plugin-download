# Tauri Plugin Download

[![CI][ci-badge]][ci-url]

State-driven, resumable download API for Tauri 2.x apps.

This plugin provides a cross-platform download interface with resumable downloads,
progress tracking, and proper resource management.

[ci-badge]: https://github.com/silvermine/tauri-plugin-download/actions/workflows/ci.yml/badge.svg
[ci-url]: https://github.com/silvermine/tauri-plugin-download/actions/workflows/ci.yml

## Features

   * Parallel, resumable download support
   * Persistable, thread-safe store
   * State and progress notifications
   * Cross-platform support (Linux, Windows, macOS, Android, iOS)

| Platform | Supported |
| -------- | --------- |
| Linux    | ✓         |
| Windows  | ✓         |
| macOS    | ✓         |
| Android  | ✓         |
| iOS¹     | ✓         |

¹ Supports fully interruptible and resumable background downloads, even when the app
is suspended or terminated using
[`URLSession`](https://developer.apple.com/documentation/foundation/urlsession) with a
background configuration.

## Getting Started

### Installation

1. Install NPM dependencies:

   ```bash
   npm install
   ```

2. Build the TypeScript bindings:

   ```bash
   npm run build
   ```

3. Build the Rust plugin:

   ```bash
   cargo build
   ```

### Tests

Run Rust tests:

```bash
cargo test
```

## Install

_This plugin requires a Rust version of at least **1.77.2**_

### Rust

Add the plugin to your `Cargo.toml`:

`src-tauri/Cargo.toml`

```toml
[dependencies]
tauri-plugin-download = { git = "https://github.com/silvermine/tauri-plugin-download" }
```

### JavaScript/TypeScript

Install the JavaScript bindings:

```sh
npm install @silvermine/tauri-plugin-download
```

## Usage

### Prerequisites

Initialize the plugin in your `tauri::Builder`:

```rust
fn main() {
   tauri::Builder::default()
      .plugin(tauri_plugin_download::init())
      .run(tauri::generate_context!())
      .expect("error while running tauri application");
}
```

### API

#### List downloads

```ts
import { list } from 'tauri-plugin-download';

async function listDownloads() {
   const downloads = await list();

   for (const download of downloads) {
      console.debug(`Found '${download.key}': [${download.status}, ${download.progress}%]`);
   }
}
```

#### Get a download

```ts
import { get, DownloadStatus } from 'tauri-plugin-download';

async function getDownload() {
   const download = await get('file.zip');

   if (download.status === DownloadStatus.Pending) {
      console.debug(`Download '${download.key}' not found in store`);
   } else {
      console.debug(`Found '${download.key}': [${download.status}, ${download.progress}%]`);
   }
}
```

#### Create, start, pause, resume or cancel a download

The API uses discriminated unions with type guards for compile-time safety.
Only valid methods are available based on the download's status.

```ts
import { get, DownloadStatus, hasAction, DownloadAction } from 'tauri-plugin-download';

async function createAndStartDownload() {
   const download = await get('file.zip');

   if (download.status === DownloadStatus.Pending) {
      // Download not in store - create it first
      const { download: created } = await download.create(
         'https://example.com/file.zip',
         '/path/to/file.zip'
      );
      await created.start();
   }
}

async function manageDownload() {
   const download = await get('file.zip');

   if (hasAction(download, DownloadAction.Start)) {
      await download.start(); // TypeScript knows start() is available
   } else if (hasAction(download, DownloadAction.Pause)) {
      await download.pause(); // TypeScript knows pause() is available
   } else if (hasAction(download, DownloadAction.Resume)) {
      await download.resume(); // TypeScript knows resume() is available
   }
}
```

#### Listen for progress notifications

Listeners can be attached to downloads in any status, including `Pending`.
This allows you to set up listeners before creating the download.

```ts
import { get, DownloadStatus } from 'tauri-plugin-download';

async function setupAndStartDownload() {
   const download = await get('file.zip');

   // Attach listener (works for Pending downloads too)
   const unlisten = await download.listen((updated) => {
      console.debug(`'${updated.key}': ${updated.progress}%`);
   });

   // Create and start if pending
   if (download.status === DownloadStatus.Pending) {
      const { download: created } = await download.create(
         'https://example.com/file.zip',
         '/path/to/file.zip'
      );
      await created.start();
   }

   // To stop listening
   unlisten();
}
```

### Examples

Check out the [examples/tauri-app](examples/tauri-app) directory for a working example of
how to use this plugin.

## Development Standards

This project follows the
[Silvermine standardization](https://github.com/silvermine/standardization)
guidelines. Key standards include:

   * **EditorConfig**: Consistent editor settings across the team
   * **Markdownlint**: Markdown linting for documentation
   * **Commitlint**: Conventional commit message format
   * **Code Style**: 3-space indentation, LF line endings

### Running Standards Checks

```bash
npm run standards
```

## License

MIT

## Contributing

Contributions are welcome! Please follow the established coding standards and commit
message conventions.
