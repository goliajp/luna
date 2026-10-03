//! Whether this host has what building an AOT binary for itself needs.

use std::process::Command;

fn on_path(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success() || o.status.code().is_some())
        .unwrap_or(false)
}

/// `cargo` builds the runtime staticlib; off Windows the link goes
/// through `cc`. On Windows luna-aot finds the MSVC tools in the Visual
/// Studio install itself, and a host that linked this test binary has
/// them, so a missing tool there fails the test instead of skipping it.
pub fn host_can_link() -> bool {
    on_path("cargo") && (cfg!(windows) || on_path("cc"))
}
