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
   // Both the unit and integration test executables link Tauri's desktop mock,
   // which needs Common Controls v6 before the test process can start.
   // rustc-link-arg-tests excludes the lib's unit-test harness, so use link-arg.
   // This package is an rlib; these flags do not reach consuming applications.
   if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
      && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
   {
      println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
      println!(
         "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' \
          name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
          processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
      );
   }

   tauri_plugin::Builder::new(COMMANDS)
      .android_path("android")
      .ios_path("ios")
      .build();
}
