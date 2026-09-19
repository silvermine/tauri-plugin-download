# Tauri Plugin Download

[![CI][ci-badge]][ci-url]

State-driven, resumable download API for Tauri 2.x apps.

[ci-badge]: https://github.com/silvermine/tauri-plugin-download/actions/workflows/ci.yml/badge.svg
[ci-url]: https://github.com/silvermine/tauri-plugin-download/actions/workflows/ci.yml

## Contents

   * [Features](#features)
   * [Getting Started](#getting-started)
   * [Install](#install)
   * [Usage](#usage)
      * [Prerequisites](#prerequisites)
      * [Configuration](#configuration)
      * [API](#api)
      * [Testing with mocks](#testing-with-mocks)
   * [Android Support](#android-support)
      * [Manifest Declarations](#manifest-declarations)
      * [Google Play Console](#google-play-console)
   * [iOS Support](#ios-support)
   * [Download Store Schema](#download-store-schema)
      * [Store Tests](#store-tests)
   * [Development Standards](#development-standards)
   * [License](#license)
   * [Contributing](#contributing)

## Features

   * Parallel, resumable download support
   * Persistable, thread-safe store
   * State, byte count, and progress notifications

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

_This plugin requires a Rust version of at least **1.94.0**_

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

### Configuration

Use `Builder` instead of `init()` to configure the plugin. `init()` is shorthand for
`Builder::new().build()`.

#### Builder options

| Method | Type | Default | Description |
| --- | --- | --- | --- |
| `user_agent` | `impl Into<String>` | None | `User-Agent` sent with every download request |
| `on_setup` | `FnOnce(&AppHandle, &mut SetupConfig) -> Result<(), Box<dyn Error>>` | None | Hook for settings that need the app instance |

#### Setup options

Set inside `on_setup`, which runs once the app exists and `app.path()` is available:

| Method | Type | Default | Description |
| --- | --- | --- | --- |
| `store_dir` | `impl Into<PathBuf>` | Platform-specific | Directory holding `downloads.json` |

Full example:

```rust
use tauri::Manager;

fn main() {
   tauri::Builder::default()
      .plugin(
         tauri_plugin_download::Builder::new()
            .user_agent("my-app/1.0")
            .on_setup(|app, config| {
               config.store_dir(app.path().app_data_dir()?.join("downloads"));
               Ok(())
            })
            .build(),
      )
      .run(tauri::generate_context!())
      .expect("error while running tauri application");
}
```

#### Platform defaults

Both settings are opt-in. Left unset, each platform keeps its own:

| Platform | Store location | User agent |
| --- | --- | --- |
| Desktop | `app_data_dir()/downloads.json` | none |
| Android | `filesDir/downloads.json` | `okhttp/<version>` |
| iOS | `Application Support/downloads.json` | `<app>/<version> CFNetwork/… Darwin/…` |

Key behaviors:

   * **The store is not migrated.** Changing `store_dir`, or adopting it for the first
     time, leaves any old records where they are, invisible to the plugin — treat it as
     discarding the download history
   * `store_dir` must be absolute, and on mobile inside the app sandbox; a relative path
     fails plugin initialization
   * **Keep `store_dir` on internal storage on Android.** The store lists every
     download's URL and local path, so on external or shared storage it is readable by
     any app holding storage access on pre-scoped-storage devices. The default,
     `filesDir`, is app-private
   * The directory need not exist — iOS creates a configured one during plugin
     initialization, everything else on the first write
   * A configured directory that cannot be created fails plugin initialization on iOS,
     rather than leaving every write to fail into a log line the caller never sees
   * Returning `Err` from `on_setup` aborts startup
   * `user_agent` must be printable ASCII or horizontal tab, the rule all three HTTP
     stacks share; anything else fails plugin initialization
   * An empty `user_agent` is accepted and sends an empty header rather than none
   * A download resumed after a relaunch uses the new run's user agent on desktop and
     Android; iOS keeps the value the download started with when it resumes from resume
     data


### API

#### List downloads

```ts
import { list } from 'tauri-plugin-download';

async function listDownloads() {
   const downloads = await list();

   for (const download of downloads) {
      const totalBytes = download.totalBytes ?? 'unknown';

      console.debug(
         `Found '${download.path}': [${download.status}, ${download.receivedBytes}/${totalBytes} bytes, ${download.progress}%]`
      );
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
      const totalBytes = download.totalBytes ?? 'unknown';

      console.debug(
         `Found '${download.path}': [${download.status}, ${download.receivedBytes}/${totalBytes} bytes, ${download.progress}%]`
      );
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

A download can be restricted to unmetered, unconstrained networks when it is created:

```ts
const download = await get('/path/to/large-file.zip');

if (download.status === DownloadStatus.Pending) {
   const { download: created } = await download.create(
      'https://example.com/large-file.zip',
      { allowMetered: false }
   );

   await created.start();
}
```

`allowMetered` defaults to `true`. When it is `false`, the download is confined to
connections the platform reports as unmetered — no cellular, no personal hotspot.
Desktop and iOS also exclude constrained connections (Low Data Mode on iOS); Android
has no equivalent, so there the option maps to WorkManager's unmetered constraint
alone.

Only mobile has an OS scheduler to defer to, so desktop refuses where mobile waits:

| | No eligible network at `start()`/`resume()` | Network stops qualifying mid-transfer |
| --- | --- | --- |
| Desktop | Rejects; the download stays `Idle` or `Paused` to retry later. | Keeps running. |
| iOS | Resolves, status `InProgress`; the background `URLSession` task waits, transferring nothing. | Stalls, then continues on its own. |
| Android | Resolves, status `InProgress`; WorkManager holds the work request. | Stalls, then continues on its own — see below. |

Recovery needs no call from you on either platform, but they differ. iOS leaves the task
alone and it continues when a network satisfies it. Android's worker cannot survive the
connection going away, so WorkManager retries it on the same constraint, resuming from
the partial file — expect it to lag by the backoff delay rather than restarting the
moment the network qualifies.

Android can also report `Paused` mid-hold: if the constraint tracker stops the worker
before the connection drops — the two race — the retry moves it back to `InProgress`. A
record merely waiting, on the unmetered constraint or in a retry backoff, stays
`InProgress` with no worker running; restart the app then and the plugin reconciles it to
`Paused`, or `Idle` at zero bytes when no partial file survives, before the pending work
moves it back. Reconciliation emits no event, so the stale value arrives through the next
`get()` or `list()`.

So treat `Paused` and `Idle` as "not currently transferring" rather than "waiting for the
user", and drive recovery off events. No bytes are lost either way. Constraint holds are
not time-limited; transient errors are. A download gives up after at most five retries —
fewer if constraint interruptions have spent part of the same budget — then needs
`resume()`, or `start()` if it reconciled to `Idle`.

> Resuming an iOS download goes through `downloadTask(withResumeData:)`, which takes no
> request, so the policy is inherited from the original task rather than reapplied. That
> it survives is confirmed on device, but Apple does not document resume data as carrying
> request properties — worth re-checking against new iOS releases.

The network policy is fixed when the download is first created. Every download state
exposes its resolved policy through `download.options.allowMetered`. Calling `create()`
again for an existing path returns the existing record without changing its URL or
options.

#### Listen for progress notifications

Listeners can be attached to downloads in any status, including `Pending`, so they can
be set up before the download is created. Each download state includes `receivedBytes`,
`totalBytes`, and `progress`. When the server does not provide a content length,
`totalBytes` is `null`; `progress` remains `0` until the terminal `Completed` event,
where it is `100`.

```ts
import { get, DownloadStatus } from 'tauri-plugin-download';

async function setupAndStartDownload() {
   const download = await get('/path/to/file.zip');

   // Attach listener (works for Pending downloads too)
   const unlisten = await download.listen((updated) => {
      console.debug(
         `'${updated.path}': ${updated.receivedBytes}/${updated.totalBytes ?? 'unknown'} bytes (${updated.progress}%)`
      );
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

Alternatively, pass `{ autoUnlisten: true }` to automatically remove the listener
when the download reaches a terminal state (`Completed` or `Canceled`):

```ts
await download.listen((updated) => {
   console.debug(`'${updated.path}': ${updated.progress}%`);
}, { autoUnlisten: true });
```

### Examples

Check out the [examples/tauri-app](examples/tauri-app) directory for a working example of
how to use this plugin.

### Testing with mocks

For unit tests, this package publishes `@silvermine/tauri-plugin-download/mocks` so you
can keep using the real JavaScript API while mocking the Tauri backend.

This helper is designed for tests that need to:

   * Seed one or more downloads before the test runs
   * Exercise `get()`, `list()`, and download actions without a real Tauri app
   * Emit download change events for listener tests
   * Inject command errors for failure scenarios

The mock helper approximates backend/native state transitions for common test flows.
It is not a backend contract and does not transition downloads to `Completed`.
Canceling a download, or emitting a `Canceled` or `Completed` change, removes it from
the store as the native platforms do, so `get()` then returns a `Pending` download.
Use `emitChange()` to simulate progress updates or terminal-state events, or
`setDownload()` to seed a specific state without emitting an event.
Seeded downloads, and those passed to `setDownload()`, must be `Idle`, `InProgress` or
`Paused`, the only statuses a native store holds; to test a download that does not
exist yet, seed nothing and `get()` returns it as `Pending`.
It only simulates the desktop event path and returns `false` for `is_native`,
so tests for the native/mobile listener branch need a separate approach.
As on the native platforms, `start`, `resume`, `pause` and `cancel` reject with
`Not Found: <path>` for a path with no stored download.

`createMockDownloadState()` computes `progress` from `receivedBytes` and
`totalBytes` when `progress` is not explicitly provided. For unknown-size
downloads, use `totalBytes: null`. A generated `Completed` state reports
`progress: 100`.

```ts
import { afterEach, expect, it } from 'vitest';
import {
   DownloadAction,
   DownloadStatus,
   get,
   hasAction,
} from '@silvermine/tauri-plugin-download';
import {
   clearDownloadMocks,
   createMockDownloadState,
   mockDownloadPlugin,
} from '@silvermine/tauri-plugin-download/mocks';

afterEach(() => {
   clearDownloadMocks();
});

it('starts a mocked download', async () => {
   mockDownloadPlugin({
      downloads: [
         createMockDownloadState(DownloadStatus.Idle, {
            path: '/tmp/file.zip',
         }),
      ],
   });

   const download = await get('/tmp/file.zip');

   if (hasAction(download, DownloadAction.Start)) {
      const response = await download.start();

      expect(response.download.status).toBe(DownloadStatus.InProgress);
   }
});
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

### Manifest Declarations

Downloads run in a foreground service, which Android 14 (API 34) requires an app to
declare. The plugin declares it in its own manifest and the manifest merger folds it
into the app, so **there is nothing to add to
`gen/android/app/src/main/AndroidManifest.xml`**. What the merge adds:

| Declaration | Why it is needed |
| --- | --- |
| `android.permission.INTERNET` | The transfer itself |
| `android.permission.FOREGROUND_SERVICE` | `WorkManager` runs each download as a foreground service so it survives the app being backgrounded |
| `android.permission.FOREGROUND_SERVICE_DATA_SYNC` | The typed permission API 34 requires for a `dataSync` service |
| `android.permission.POST_NOTIFICATIONS` | The ongoing progress notification a foreground service must show |
| `<service android:name="androidx.work.impl.foreground.SystemForegroundService" android:foregroundServiceType="dataSync" />` | `WorkManager`'s own service, typed for API 34 |

`WorkManager` adds `WAKE_LOCK`, `ACCESS_NETWORK_STATE` and `RECEIVE_BOOT_COMPLETED`. To
see what an app ships:

```sh
cd src-tauri/gen/android && ./gradlew :app:processDebugMainManifest
cat app/build/intermediates/merged_manifest/*Debug/*/AndroidManifest.xml
```

#### Notification permission

`POST_NOTIFICATIONS` is a runtime permission on Android 13 (API 33) and later, and the
plugin does not request it. Downloads run either way; the progress notification appears
only once the app requests it and the user grants it.

#### Merge conflicts

If the app or another dependency declares `SystemForegroundService` with a different
`android:foregroundServiceType`, the merge fails. Declare the service in the app manifest
with every type it needs and let it win:

```xml
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    xmlns:tools="http://schemas.android.com/tools">
    <application>
        <service
            android:name="androidx.work.impl.foreground.SystemForegroundService"
            android:foregroundServiceType="dataSync|location"
            android:exported="false"
            tools:replace="android:foregroundServiceType" />
    </application>
</manifest>
```

The list must still include `dataSync`, and each type needs its matching
`FOREGROUND_SERVICE_*` permission.

#### Opting out of the foreground service

To ship without the foreground service permissions — and without the Play declaration
below — remove them in the app manifest, which needs
`xmlns:tools="http://schemas.android.com/tools"` on its `<manifest>` element:

```xml
<uses-permission android:name="android.permission.FOREGROUND_SERVICE"
    tools:node="remove" />
<uses-permission android:name="android.permission.FOREGROUND_SERVICE_DATA_SYNC"
    tools:node="remove" />
```

Downloads still run: the worker logs the failed promotion and continues as ordinary
background work. Expect the system to stop it soon after the app leaves the foreground,
and `WorkManager` to resume it later.

### Google Play Console

An app that ships `FOREGROUND_SERVICE_DATA_SYNC` and targets API 34 or later must
[declare that use][play-fgs] before it can roll out. In Play Console: **Monitor and
improve → App content → Foreground service permissions**.

   1. Select the **Data sync** type — the one this plugin uses
   2. Describe the feature: user-started file downloads that keep transferring while the
      app is backgrounded
   3. Say what happens if the system defers or interrupts the download — a required part
      of the declaration, not the same question as step 2
   4. Link a video showing the download continuing, not just the download UI
   5. Submit — it is reviewed, and an incomplete declaration blocks a rollout

[play-fgs]: https://support.google.com/googleplay/android-developer/answer/13392821

The declaration belongs to the app rather than to a release, so revisit it only when the
set of foreground service types changes.

**Note**: an app _targeting_ API 35 or higher gets [a six-hour cap][fgs-timeout] on
`dataSync` foreground service runtime per 24 hours — it follows the target SDK, not the
device, so targeting 34 avoids it on an Android 15 phone. Bringing the app to the
foreground resets the budget; long transfers are resumed rather than run in one stretch.

[fgs-timeout]: https://developer.android.com/about/versions/15/behavior-changes-15

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

### Testing the Network Policy

No SIM needed. Mark the Wi-Fi network as metered from its settings page — Network &
internet on stock Android, Connections on One UI — where the option reads "Treat as
metered". The example app's "Allow metered networks" toggle sets `allowMetered` on the
downloads it creates.

With the toggle off, a download holds at zero bytes until the network is marked
unmetered. Marking it metered mid-transfer stalls the download; it continues by itself
a backoff delay later.

## iOS Support

On iOS, this plugin uses `URLSession` with a background configuration, so downloads
continue even when the app is suspended or terminated by the system.

### How It Works

1. **App Running**: Downloads proceed normally with real-time progress updates
2. **App Suspended**: iOS continues downloads in the background
3. **App Terminated**: iOS completes downloads and relaunches the app in the background
   to deliver results
4. **App Resumed**: The plugin reconciles state and emits completion events

### Running the Example App

Open `ios/DownloadManagerExample/DownloadManagerExample.xcodeproj` in Xcode,
select a simulator or device, and run.

### Testing the Network Policy

No SIM needed, but a real device is — Low Data Mode cannot be set on a simulator.
Toggle it under Settings → Wi-Fi → the ⓘ beside your network → Low Data Mode, which
trips `allowsConstrainedNetworkAccess`. The example app's "Allow metered networks"
toggle sets `allowMetered` on the downloads it creates.

With the toggle off, a download holds at zero bytes until Low Data Mode is off.
Turning it on mid-transfer stalls the download without leaving `inProgress`; it
continues by itself once off again.

Worth repeating on new iOS majors: pause a restricted download, turn Low Data Mode on,
then resume — it should stay stalled. Resume goes through
`downloadTask(withResumeData:)`, which carries the policy in undocumented resume data,
so this is the check that catches a silent regression.

### Background Downloads in Tauri Apps

Background downloads work automatically in Tauri apps: when the app resumes, all delegate
callbacks are delivered and state is reconciled.

**Note**: Tauri does not expose the `AppDelegate` hook for
`handleEventsForBackgroundURLSession`, so iOS is never told that background event
processing finished. It may then keep the app running longer than necessary, skip the
app-switcher snapshot, or deprioritize future background execution. Downloads themselves
are unaffected — iOS delivers every pending callback when the app resumes.

If Tauri exposes `AppDelegate` hooks in the future, add:

```swift
import DownloadManagerKit

func application(_ application: UIApplication,
                 handleEventsForBackgroundURLSession identifier: String,
                 completionHandler: @escaping () -> Void) {
   DownloadManager.shared.setBackgroundCompletionHandler(completionHandler)
}
```

## Download Store Schema

Desktop, Android, and iOS persist `downloads.json` with this envelope:

```json
{
   "version": 1,
   "downloads": []
}
```

`version` is an integer file-format revision, independent of the plugin version.
`downloads` contains the platform's existing persisted records. Both fields are
required, including for an empty store. Readers validate the envelope and version
before decoding records, and ignore unknown fields in supported v1 documents.

Old bare arrays, malformed documents, and unsupported versions use the existing
load-error path: startup continues with an empty store. Loading does not rewrite
the file, but a later save can overwrite it. Atomic writes protect against
interrupted writes; preserving rejected files is separate work under
[#64](https://github.com/silvermine/tauri-plugin-download/issues/64).

When adopting schema v1, remove the old development `downloads.json` from the
effective store directory, or clear the development app's data. If `store_dir` is
configured, clean up that directory rather than assuming the platform default.
There is no migration from the old array format. Future schema changes will add
migrations and version-specific record types when needed.

On iOS, the default store directory is Application Support. Older development
builds used Documents; files there are not imported automatically. Clean up the
effective directory used by the build being tested. iOS records retain their
optional `resumeDataPath`; the schema change does not move resume data.

### Store Tests

```bash
cargo test -p download-manager --lib store::tests
cd android
./gradlew :lib:test
./gradlew :lib:connectedDebugAndroidTest
```

The last command requires a connected Android device or emulator. It exercises
the real `AtomicFile`, directory creation, backup recovery, and loading rejected
documents. Kotlin JVM tests cover schema validation and record round trips.

On macOS, run the Swift package tests (also run by the macOS CI job):

```bash
swift test --package-path ios/DownloadManagerKit
```

These cover the native store and schema, including iOS resume-data fields. They
do not replace testing the full plugin on an iOS simulator or device.

## Development Standards

This project follows the
[Silvermine standardization](https://github.com/silvermine/standardization)
guidelines. Key standards include:

   * **EditorConfig**: Consistent editor settings across the team
   * **Markdownlint**: Markdown linting for documentation
   * **Commitlint**: Conventional commit message format
   * **Code Style**: 3-space indentation, LF line endings

Run every check:

```bash
npm run standards
```

## License

MIT

## Contributing

Contributions are welcome! Please follow the established coding standards and commit
message conventions.
