//! C hosts of the C API. Each program in `tests/capi/` is compiled against
//! luna's headers for every dialect it has a recording of, linked to the C
//! API's shared library, run, and its output compared line by line with
//! what the same program printed when built against PUC Lua:
//! `tests/capi/expected/<name>.<5x>.out` (standard output), and when they
//! exist `<name>.<5x>.err` (standard error) and `<name>.<5x>.status` (`0`,
//! `1` or `abort`; `0` when absent). The programs are built with the
//! compatibility flags PUC's makefiles use.

use std::path::{Path, PathBuf};
use std::process::Command;

const DIALECTS: [(&str, &str); 5] = [
    ("51", "lua5.1"),
    ("52", "lua5.2"),
    ("53", "lua5.3"),
    ("54", "lua5.4"),
    ("55", "lua5.5"),
];

const DEFINES: [&str; 3] = ["LUA_COMPAT_ALL", "LUA_COMPAT_5_2", "LUA_COMPAT_5_3"];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `target/<profile>`, where cargo put the C API's shared library.
fn lib_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("test executable");
    exe.parent()
        .and_then(Path::parent)
        .expect("tests run from target/<profile>/deps")
        .to_path_buf()
}

/// Build the C API's shared library next to the test executable once per
/// run: `cargo test` builds the crate as an rlib only.
fn ensure_lib() -> Result<(), String> {
    static BUILT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let lib = lib_dir();
            let profile = match lib.file_name().and_then(|n| n.to_str()) {
                Some("debug") => "dev".to_string(),
                Some(p) => p.to_string(),
                None => return Err("no profile directory".to_string()),
            };
            let target = env!("LUNA_BUILD_TARGET");
            let mut target_dir = lib.parent().expect("target/<profile>").to_path_buf();
            let cross = target_dir.file_name().and_then(|n| n.to_str()) == Some(target);
            if cross {
                target_dir.pop();
            }
            let mut cmd = Command::new(option_env!("CARGO").unwrap_or("cargo"));
            cmd.args(["build", "--lib", "-p", "luna-jit", "--profile", &profile])
                .arg("--manifest-path")
                .arg(crate_dir().join("Cargo.toml"))
                .arg("--target-dir")
                .arg(&target_dir);
            if cross {
                cmd.args(["--target", target]);
            }
            let out = cmd.output().map_err(|e| format!("cargo: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).into_owned())
            }
        })
        .clone()
}

fn compiler() -> cc::Tool {
    let target = env!("LUNA_BUILD_TARGET");
    cc::Build::new()
        .cargo_metadata(false)
        .cargo_warnings(false)
        .opt_level(0)
        .debug(false)
        .target(target)
        .host(target)
        .get_compiler()
}

/// Compile `tests/capi/<name>.c` against the headers in `include/<inc>`.
fn build(name: &str, v: &str, inc: &str) -> Result<PathBuf, String> {
    let tool = compiler();
    let lib = lib_dir();
    let out_dir = lib.join("capi-hosts");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let exe = out_dir.join(format!("{name}-{v}{}", std::env::consts::EXE_SUFFIX));
    let src = crate_dir().join("tests/capi").join(format!("{name}.c"));
    let include = crate_dir().join("include").join(inc);
    ensure_lib()?;
    let mut cmd = tool.to_command();
    if tool.is_like_msvc() {
        cmd.arg("/nologo")
            .arg("/w")
            .arg(format!("/I{}", include.display()));
        cmd.arg(format!("/I{}", crate_dir().join("tests/capi").display()));
        for d in DEFINES {
            cmd.arg(format!("/D{d}"));
        }
        cmd.arg(&src).arg(format!("/Fe{}", exe.display()));
        cmd.arg(format!("/Fo{}\\", out_dir.display()));
        cmd.arg("/link").arg(lib.join("luna_jit.dll.lib"));
    } else {
        cmd.arg("-w").arg("-I").arg(&include);
        cmd.arg("-I").arg(crate_dir().join("tests/capi"));
        for d in DEFINES {
            cmd.arg(format!("-D{d}"));
        }
        cmd.arg(&src).arg("-o").arg(&exe);
        cmd.arg("-L").arg(&lib).arg("-lluna_jit");
        if !cfg!(windows) {
            cmd.arg(format!("-Wl,-rpath,{}", lib.display()));
        }
        if cfg!(target_os = "linux") {
            cmd.arg("-lm");
        }
    }
    let out = cmd
        .output()
        .map_err(|e| format!("{:?}: {e}", tool.path()))?;
    if !out.status.success() {
        return Err(format!(
            "compiling {name} for {v} failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(exe)
}

/// How a process ended, as the recordings write it.
fn status_text(st: std::process::ExitStatus) -> String {
    match st.code() {
        // Windows' abort() exits with 3, or with STATUS_STACK_BUFFER_OVERRUN
        // (0xC0000409) when the C runtime ends the process with __fastfail
        Some(3 | -1_073_740_791) if cfg!(windows) => "abort".to_string(),
        Some(c) => c.to_string(),
        None => "abort".to_string(),
    }
}

fn read_expected(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.replace("\r\n", "\n"))
}

/// Line by line differences, or `None` when equal.
fn diff(what: &str, want: &str, got: &str) -> Option<String> {
    if want == got {
        return None;
    }
    let mut out = format!("{what} differs:\n");
    let (w, g): (Vec<_>, Vec<_>) = (want.lines().collect(), got.lines().collect());
    for i in 0..w.len().max(g.len()) {
        let (a, b) = (w.get(i), g.get(i));
        if a != b {
            out.push_str(&format!(
                "  line {}:\n    PUC:  {:?}\n    luna: {:?}\n",
                i + 1,
                a,
                b
            ));
        }
    }
    Some(out)
}

/// Build and run `tests/capi/<name>.c` for every dialect with a
/// recording, and fail with every difference from PUC's output.
pub fn check(name: &str) {
    let expected = crate_dir().join("tests/capi/expected");
    let mut failures = Vec::new();
    let mut ran = 0;
    for (v, inc) in DIALECTS {
        let Some(want_out) = read_expected(&expected.join(format!("{name}.{v}.out"))) else {
            continue;
        };
        ran += 1;
        let exe = match build(name, v, inc) {
            Ok(e) => e,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        let mut cmd = Command::new(&exe);
        if cfg!(windows) {
            // the shared library is found next to the program or on PATH
            let path = std::env::var_os("PATH").unwrap_or_default();
            let mut dirs = vec![lib_dir()];
            dirs.extend(std::env::split_paths(&path));
            cmd.env("PATH", std::env::join_paths(dirs).expect("PATH"));
        }
        let out = cmd.output().expect("run the C host");
        let got_out = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
        let got_err = String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n");
        if let Some(d) = diff(&format!("{name} {v} stdout"), &want_out, &got_out) {
            failures.push(d);
        }
        if let Some(want_err) = read_expected(&expected.join(format!("{name}.{v}.err")))
            && let Some(d) = diff(&format!("{name} {v} stderr"), &want_err, &got_err)
        {
            failures.push(d);
        }
        let want_status = read_expected(&expected.join(format!("{name}.{v}.status")))
            .map_or_else(|| "0".to_string(), |s| s.trim().to_string());
        let got_status = status_text(out.status);
        if want_status != got_status {
            failures.push(format!(
                "{name} {v}: ended with {got_status}, PUC with {want_status}; stderr:\n{got_err}"
            ));
        }
    }
    assert!(ran > 0, "no recordings for {name} in tests/capi/expected");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn pcall_handler() {
    check("pcall_handler");
}

#[test]
fn error_unwinding() {
    check("error_unwind");
}

#[test]
fn core_stack() {
    check("core_stack");
}

#[test]
fn c_stack_recursion() {
    check("cstack_recursion");
}
