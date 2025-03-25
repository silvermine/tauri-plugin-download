# tauri-plugin-download

State-driven, resumable download API for Tauri 2.x apps.

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
| iOS      | ✓         |

## Installation

Note: These steps are an interim workaround until the plugin can be published to npm/crates.io.

### Rust

Add the `tauri-plugin-download` crate to your `Cargo.toml`:

```toml
[dependencies]
tauri-plugin-download = { git = "https://github.com/silvermine/tauri-plugin-download.git" }
```

### TypeScript

Install the TypeScript bindings via npm:

```bash
npm install github:@silvermine/tauri-plugin-download
```

Add `postInstall` step to your `package.json` to build the bindings:

```json
"postinstall": "cd ./node_modules/@silvermine/tauri-plugin-download && npm install && npm run build"
```

## Usage

### Rust

Initialize the plugin in your `tauri::Builder`:

```rust
fn main() {
   tauri::Builder::default()
      .plugin(tauri_plugin_download::init())
      .run(tauri::generate_context!())
      .expect("error while running tauri application");
}
```

### TypeScript

#### Create a download

```ts
import { create } from "tauri-plugin-download";

async function createDownload() {
   const key = "file.zip";
   const url = "https://example.com/file.zip";
   const path = "/path/to/save/file.zip";

   const download = await create(key, url, path);
   console.log(`Created '${download.key}':${download.url}`)
}
```

#### List downloads

```ts
import { list } from "tauri-plugin-download";

async function listDownloads() {
   const downloads = await list();
   for (let download of downloads) {
      console.log(`Found '${download.key}':${download.url} [${download.state}, ${download.progress}%]`)
   }
}
```

#### Get a download

```ts
import { get } from "tauri-plugin-download";

async function getDownload() {
   const download = await get("file.zip");
   console.log(`Found '${download.key}':${download.url} [${download.state}, ${download.progress}%]`)
}
```

#### Start, pause, resume or cancel a download

```ts
import { get } from "tauri-plugin-download";

async function getDownloadAndUpdate() {
   const download = await get("file.zip");
   download.start();   // Start download
   download.pause();   // Pause download
   download.resume();  // Resume download
   download.cancel();  // Cancel download
}
```

#### Listen for progress notifications

```ts
import { get } from "tauri-plugin-download";

async function getDownloadAndListen() {
   const download = await get("file.zip");
   const unlisten = await download.listen((updatedDownload) => {
      console.log(`'${download.key}':${download.progress}%`)
   });

   // To stop listening
   unlisten();
}
```

### Examples

Check out the [examples/tauri-app](examples/tauri-app) directory for a working example of how to use this plugin.

## How do I contribute?

We genuinely appreciate external contributions. See [our extensive
documentation](https://github.com/silvermine/silvermine-info#contributing) on how to
contribute.

## License

This software is released under the MIT license. See [the license file](LICENSE) for more
details.
