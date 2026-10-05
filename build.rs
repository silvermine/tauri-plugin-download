const COMMANDS: &[&str] = &[
   "create",
   "list",
   "get",
   "start",
   "cancel",
   "pause",
   "resume",
   "is_native",
   "register_listener",
   "remove_listener",
];

fn main() {
   tauri_plugin::Builder::new(COMMANDS)
      .android_path("android")
      .ios_path("ios")
      .build();
}
