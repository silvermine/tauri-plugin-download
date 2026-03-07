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

| Platform  | Supported |
| --------- | --------- |
| Linux     | ✓         |
| Windows   | ✓         |
| macOS     | ✓         |
| Android¹  | ✓         |
| iOS²      | ✓         |

¹ Uses [WorkManager][workmanager] with foreground service notifications for
reliable background downloads with resumable support via HTTP `Range` headers.
See [Android Support](#android-support) for details.

² Supports fully interruptible and resumable background downloads, even when
the app is suspended or terminated using
[`URLSession`](https://developer.apple.com/documentation/foundation/urlsession)
with a background configuration. See
[iOS Support](#ios-support) for details.

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

Run Swift tests (iOS download manager library):

```bash
swift test --package-path ios/DownloadManagerKit
```

Run Kotlin tests (Android download manager library):

```bash
cd android && ./gradlew :lib:test
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
      console.debug(`Found '${download.path}': [${download.status}, ${download.progress}%]`);
   }
}
```

#### Get a download

```ts
import { get, DownloadStatus } from 'tauri-plugin-download';

async function getDownload() {
   const download = await get('/path/to/file.zip');

   if (download.status === DownloadStatus.Pending) {
      console.debug(`Download '${download.path}' not found in store`);
   } else {
      console.debug(`Found '${download.path}': [${download.status}, ${download.progress}%]`);
   }
}
```

#### Create, start, pause, resume or cancel a download

The API uses discriminated unions with type guards for compile-time safety.
Only valid methods are available based on the download's status.

```ts
import { get, DownloadStatus, hasAction, DownloadAction } from 'tauri-plugin-download';

async function createAndStartDownload() {
   const download = await get('/path/to/file.zip');

   if (download.status === DownloadStatus.Pending) {
      // Download not in store - create it first
      const { download: created } = await download.create('https://example.com/file.zip');
      await created.start();
   }
}

async function manageDownload() {
   const download = await get('/path/to/file.zip');

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
   const download = await get('/path/to/file.zip');

   // Attach listener (works for Pending downloads too)
   const unlisten = await download.listen((updated) => {
      console.debug(`'${updated.path}': ${updated.progress}%`);
   });

   // Create and start if pending
   if (download.status === DownloadStatus.Pending) {
      const { download: created } = await download.create('https://example.com/file.zip');
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

## Android Support

On Android, this plugin uses a pure Kotlin download manager library (`:lib` module)
backed by [WorkManager][workmanager] with `CoroutineWorker` for reliable background
execution.

[workmanager]: https://developer.android.com/develop/background-work/background-tasks/persistent/getting-started

### How It Works

1. **App Running**: Downloads run as foreground services with notifications,
   with real-time progress updates
2. **App Backgrounded**: `WorkManager` ensures downloads continue reliably
3. **Resumable**: Supports HTTP `Range` headers for resuming interrupted
   downloads
4. **App Resumed**: The plugin reconciles state and emits completion events

### Project Structure

The `android/` directory is a 3-module Gradle build:

   * **Root module** (`:`): The Tauri plugin bridge (`DownloadPlugin.kt`), depends on
     `:lib` and `:tauri-android`
   * **`:lib` module**: Pure download manager library (`org.silvermine.downloadmanager`),
     no Tauri dependencies, independently buildable and testable
   * **`:example` module**: Standalone example app (Compose UI), depends only on `:lib`

### Running the Example App

Open the `android/` directory in Android Studio, select the `:example` run configuration,
and run on an emulator or device.

## iOS Support

On iOS, this plugin uses `URLSession` with a background configuration, which allows
downloads
to continue even when the app is suspended or terminated by the system.

### How It Works

1. **App Running**: Downloads proceed normally with real-time progress updates
2. **App Suspended**: iOS continues downloads in the background
3. **App Terminated**: iOS completes downloads and relaunches the app in the background
   to deliver results
4. **App Resumed**: The plugin reconciles state and emits completion events

### Running the Example App

Open `ios/DownloadManagerExample/DownloadManagerExample.xcodeproj` in Xcode,
select a simulator or device, and run.

### Tauri Apps

Background downloads work automatically in Tauri apps. When the app resumes, all delegate
callbacks are delivered and state is properly reconciled.

**Note**: Tauri's iOS architecture doesn't currently expose the `AppDelegate` hook for
`handleEventsForBackgroundURLSession`. Without calling this completion handler, iOS cannot
determine when background event processing is complete. This may cause iOS to:

   * Keep the app running longer than necessary (wasting battery)
   * Skip taking a UI snapshot for the app switcher
   * Deprioritize future background execution for this app

In practice, this has minimal impact for typical download scenarios since iOS delivers
all pending delegate callbacks when the app resumes regardless of whether the completion
handler is called.

### Future Integration

If Tauri exposes `AppDelegate` hooks in the future, add this for optimal background
handling:

```swift
import DownloadManagerKit

func application(_ application: UIApplication,
                 handleEventsForBackgroundURLSession identifier: String,
                 completionHandler: @escaping () -> Void) {
   DownloadManager.shared.setBackgroundCompletionHandler(completionHandler)
}
```

## License

MIT

## Contributing

Contributions are welcome! Please follow the established coding standards and commit
message conventions.
