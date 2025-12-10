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

#### Create a download

```ts
import { create } from 'tauri-plugin-download';

async function createDownload() {
   const key = 'file.zip',
         url = 'https://example.com/file.zip',
         path = await join(await appDataDir(), 'downloads', key);

   const download = await create(key, url, path);

   console.debug(`Created '${download.key}':${download.url}`);
}
```

#### List downloads

```ts
import { list } from 'tauri-plugin-download';

async function listDownloads() {
   const downloads = await list();

   for (let download of downloads) {
      console.debug(`Found '${download.key}':${download.url} [${download.state}, ${download.progress}%]`)
   }
}
```

#### Get a download

```ts
import { get } from 'tauri-plugin-download';

async function getDownload() {
   const download = await get('file.zip');

   console.debug(`Found '${download.key}':${download.url} [${download.state}, ${download.progress}%]`)
}
```

#### Start, pause, resume or cancel a download

The API uses discriminated unions for compile-time safety.
Only valid methods are available based on the download's state.

```ts
import { create, get, DownloadState } from 'tauri-plugin-download';

async function createAndStartDownload() {
   const download = await create('file.zip', 'https://example.com/file.zip', '/path/to/file.zip');

   const active = await download.start(); // Returns ActiveDownload
   const paused = await active.pause();   // Returns PausedDownload
   const resumed = await paused.resume(); // Returns ActiveDownload
}

async function getDownloadAndUpdate() {
   const download = await get('file.zip');

   if (download.state === DownloadState.CREATED) {
      await download.start(); // TypeScript knows start() is available
   } else if (download.state === DownloadState.IN_PROGRESS) {
      await download.pause(); // TypeScript knows pause() is available
   } else if (download.state === DownloadState.PAUSED) {
      await download.resume(); // TypeScript knows resume() is available
   }
}
```

#### Listen for progress notifications

```ts
import { get } from 'tauri-plugin-download';

async function getDownloadAndListen() {
   const download = await get('file.zip');

   const unlisten = await download.listen((updatedDownload) => {
      console.debug(`'${updatedDownload.key}':${updatedDownload.progress}%`);
   });

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
