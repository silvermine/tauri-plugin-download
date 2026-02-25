const COMMANDS: &[&str] = &[
   "create",
   "list",
   "get",
   "start",
   "cancel",
   "pause",
   "resume",
   "is_native",
   "registerListener",
];

fn main() {
   tauri_plugin::Builder::new(COMMANDS)
      .android_path("android")
      .ios_path("ios")
      .build();
}
