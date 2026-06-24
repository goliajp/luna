//! `luna-aot` binary entry point. Dispatches to [`luna_aot::cli::run`].

fn main() -> std::process::ExitCode {
    luna_aot::cli::run()
}
