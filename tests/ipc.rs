//! Exercises the registered plugin commands through Tauri's JSON IPC boundary.
#![cfg(desktop)]

use serde_json::{Value, json};
use tauri::{Listener, WebviewWindow, test::MockRuntime};

/// Owns the app and temporary store for an isolated IPC caller.
struct Fixture {
   _app: tauri::App<MockRuntime>,
   window: WebviewWindow<MockRuntime>,
   directory: tempfile::TempDir,
}

impl Fixture {
   /// Builds the real plugin with a mock webview runtime.
   fn new() -> Self {
      let directory = tempfile::tempdir().unwrap();
      let store = directory.path().join("store");
      let downloads = directory.path().join("downloads");
      let mut context = tauri::test::mock_context(tauri::test::noop_assets());
      // mock_context has no consumer capabilities. Permit only the public commands
      // exercised here; the plugin's registered handler still performs dispatch.
      for command in [
         "create",
         "get",
         "list",
         "start",
         "resume",
         "pause",
         "cancel",
         "is_native",
      ] {
         context.runtime_authority_mut().__allow_command(
            format!("plugin:download|{command}"),
            tauri::utils::acl::ExecutionContext::Local,
         );
      }
      let app = tauri::test::mock_builder()
         .plugin(
            tauri_plugin_download::Builder::new()
               .on_setup(move |_, config| {
                  config.store_dir(store);
                  config.download_dirs([downloads]);
                  Ok(())
               })
               .build(),
         )
         .build(context)
         .unwrap();
      let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
         .build()
         .unwrap();
      Self {
         _app: app,
         window,
         directory,
      }
   }

   /// Names an allowed destination without creating a file there.
   fn path(&self, name: &str) -> String {
      self
         .directory
         .path()
         .join("downloads")
         .join(name)
         .to_str()
         .unwrap()
         .to_owned()
   }

   /// Serializes a caller's payload and captures the command's JSON response.
   fn invoke(&self, command: &str, body: Value) -> Result<Value, Value> {
      tauri::test::get_ipc_response(
         &self.window,
         tauri::webview::InvokeRequest {
            cmd: format!("plugin:download|{command}"),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: if cfg!(windows) {
               "http://tauri.localhost"
            } else {
               "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_owned(),
         },
      )
      .map(|response| response.deserialize().unwrap())
   }
}

#[test]
fn ipc_create_get_list_and_cancel_preserve_options_and_events() {
   let fixture = Fixture::new();
   let (sender, receiver) = std::sync::mpsc::channel();
   let listener = fixture
      .window
      .listen("tauri-plugin-download:changed", move |event| {
         sender
            .send(serde_json::from_str::<Value>(event.payload()).unwrap())
            .unwrap();
      });
   for (index, options, allowed) in [
      (0, json!({"allowMetered": false}), false),
      (1, json!({"allowMetered": true}), true),
      (2, json!({}), true),
   ] {
      let path = fixture.path(&format!("file-{index}"));
      let created = fixture
         .invoke(
            "create",
            json!({"path": path, "url": "https://example.com/file", "options": options}),
         )
         .unwrap();
      assert_eq!(created["download"]["options"]["allowMetered"], allowed);
      assert_eq!(created["expectedStatus"], "idle");
      assert_eq!(created["isExpectedStatus"], true);
      let event = receiver
         .recv_timeout(std::time::Duration::from_secs(3))
         .unwrap();
      assert_eq!(event, created["download"]);
      assert_eq!(
         fixture.invoke("get", json!({"path": path})).unwrap(),
         created["download"]
      );
      assert_eq!(
         fixture.invoke("list", json!({})).unwrap(),
         json!([created["download"]])
      );
      let paused = fixture.invoke("pause", json!({"path": path})).unwrap();
      assert_eq!(paused["expectedStatus"], "paused");
      assert_eq!(paused["isExpectedStatus"], false);
      let canceled = fixture.invoke("cancel", json!({"path": path})).unwrap();
      assert_eq!(canceled["download"]["status"], "canceled");
      assert_eq!(
         receiver
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap(),
         canceled["download"]
      );
      assert_eq!(
         fixture.invoke("get", json!({"path": path})).unwrap(),
         Value::Null
      );
   }
   fixture.window.unlisten(listener);
   assert_eq!(fixture.invoke("is_native", json!({})).unwrap(), false);
}

#[test]
fn ipc_commands_reject_missing_downloads_and_out_of_scope_paths() {
   let fixture = Fixture::new();
   for command in ["start", "resume", "pause", "cancel"] {
      let error = fixture
         .invoke(command, json!({"path": fixture.path("missing")}))
         .unwrap_err();
      assert_eq!(error["code"], "download not found", "{command}");
   }
   let outside = fixture.directory.path().join("outside");
   for command in ["create", "start", "resume"] {
      let error = fixture
         .invoke(
            command,
            json!({"path": outside, "url": "https://example.com/file"}),
         )
         .unwrap_err();
      assert_eq!(error["code"], "invalid input", "{command}");
      assert_eq!(
         error["message"],
         "Path Error: path must be inside a download directory"
      );
   }
   for args in [
      json!({}),
      json!({"path": 42}),
      json!({"path": fixture.path("invalid"), "url": "https://example.com/file", "options": {"allowMetered": "false"}}),
   ] {
      assert!(fixture.invoke("create", args).is_err());
   }
   let created = fixture
      .invoke(
         "create",
         json!({"path": fixture.path("default"), "url": "https://example.com/file"}),
      )
      .unwrap();
   assert_eq!(created["download"]["options"]["allowMetered"], true);
}

#[test]
fn ipc_start_pause_resume_and_cancel_reach_the_manager() {
   use std::io::{Read, Write};
   use std::time::{Duration, Instant};

   let fixture = Fixture::new();
   let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
   listener.set_nonblocking(true).unwrap();
   let url = format!("http://{}/file", listener.local_addr().unwrap());
   let (requests, received) = std::sync::mpsc::channel();
   let (release, released) = std::sync::mpsc::channel();
   // Hold each response body so pause/resume cannot race a completed transfer.
   let server = std::thread::spawn(move || {
      for _ in 0..2 {
         let deadline = Instant::now() + Duration::from_secs(10);
         let mut socket = loop {
            match listener.accept() {
               Ok((socket, _)) => break socket,
               Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                  assert!(
                     Instant::now() < deadline,
                     "Download never contacted fixture"
                  );
                  std::thread::sleep(Duration::from_millis(10));
               }
               Err(error) => panic!("accept: {error}"),
            }
         };
         // Windows can inherit the listener's nonblocking mode. Header reads
         // below must block, bounded by the read timeout, on every platform.
         socket.set_nonblocking(false).unwrap();
         socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
         let mut request = Vec::new();
         while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
         }
         socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\n")
            .unwrap();
         requests.send(()).unwrap();
         released.recv_timeout(Duration::from_secs(10)).unwrap();
      }
   });
   let path = fixture.path("transfer");
   fixture
      .invoke("create", json!({"path": path, "url": url}))
      .unwrap();
   let started = fixture.invoke("start", json!({"path": path})).unwrap();
   assert_eq!(started["download"]["status"], "inProgress");
   received.recv_timeout(Duration::from_secs(5)).unwrap();
   let paused = fixture.invoke("pause", json!({"path": path})).unwrap();
   assert_eq!(paused["download"]["status"], "paused");
   release.send(()).unwrap();
   let resumed = fixture.invoke("resume", json!({"path": path})).unwrap();
   assert_eq!(resumed["download"]["status"], "inProgress");
   received.recv_timeout(Duration::from_secs(5)).unwrap();
   let canceled = fixture.invoke("cancel", json!({"path": path})).unwrap();
   assert_eq!(canceled["download"]["status"], "canceled");
   release.send(()).unwrap();
   server.join().unwrap();
   assert_eq!(
      fixture.invoke("get", json!({"path": path})).unwrap(),
      Value::Null
   );
}
