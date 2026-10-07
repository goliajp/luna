//! `crt_text/ccs.lua`: 5.1 passes an `io.open` mode to `fopen` as it is,
//! and the MSVC C library reads a `ccs=` in it as a Unicode text mode:
//! files written and read as UTF-8 or UTF-16LE, with byte order marks.
//! Against what PUC 5.1.5 built with MSVC printed for each case, and how
//! its process ended: some cases end it, with an invalid argument to the
//! library or a read past a buffer. Each case runs in a process of its
//! own, this test binary run again for that case alone.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = include_str!("../crt_text/ccs.lua");
const PUC: &str = include_str!("../crt_text/ccs.5.1.txt");
/// `crt_text/ccsbad.lua`: the UTF-8 mode on bytes that are not UTF-8 (U+FFFD
/// as `MultiByteToWideChar` puts it, a read that ends in a character it
/// cannot complete, four continuation bytes) and on lone surrogates written.
const BAD_SCRIPT: &str = include_str!("../crt_text/ccsbad.lua");
const BAD_PUC: &str = include_str!("../crt_text/ccsbad.5.1.txt");
const CASE_VAR: &str = "LUNA_CCS_CASE";
const SCRIPT_VAR: &str = "LUNA_CCS_SCRIPT";
const OUT_MARK: &str = "CCS-OUT\t";

/// The case of the child process: run it and print its line.
#[test]
fn ccs_case_child() {
    let Ok(case) = std::env::var(CASE_VAR) else {
        return;
    };
    let script = match std::env::var(SCRIPT_VAR).as_deref() {
        Ok("bad") => BAD_SCRIPT,
        _ => SCRIPT,
    };
    let dir = std::env::temp_dir().join(format!("luna-ccs-{}-{case}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the work dir");
    let src = format!(
        "DIR = [==[{}/]==]
         arg = {{ '{case}' }}
         local OUT = {{}}
         print = function(...)
           local t = {{}}
           for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end
           OUT[#OUT + 1] = table.concat(t, '\\t')
         end
         do {script}
         end
         return table.concat(OUT, '\\n')",
        dir.display().to_string().replace('\\', "/")
    );
    let mut vm = Vm::new(LuaVersion::Lua51);
    vm.set_crt_text_mode(true);
    let out = match vm.eval(&src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("case {case}: the script returned {other:?}"),
        },
        Err(e) => panic!("case {case}: {}", vm.error_text(&e)),
    };
    drop(vm);
    let _ = std::fs::remove_dir_all(&dir);
    println!("{OUT_MARK}{out}");
}

/// The exit code a process ends with, as the parent sees it: Unix keeps
/// its low byte only.
fn seen(code: u32) -> i32 {
    if cfg!(windows) {
        code as i32
    } else {
        (code & 0xff) as i32
    }
}

#[test]
fn ccs_modes_as_in_puc_51_built_with_msvc() {
    run_cases("ccs", PUC, 162);
}

#[test]
fn ccs_utf8_mode_on_bad_bytes_as_in_puc_51_built_with_msvc() {
    run_cases("bad", BAD_PUC, 53);
}

/// Run each case of `script` in a child and hold it to the recording.
fn run_cases(script: &str, want: &str, count: usize) {
    let mut lines = want.lines().peekable();
    let exe = std::env::current_exe().expect("the test binary");
    let mut case = 0;
    while let Some(first) = lines.next() {
        case += 1;
        let (line, exit) = if first.starts_with("exit ") {
            (None, first)
        } else {
            (Some(first), lines.next().expect("an exit line"))
        };
        let code = u32::from_str_radix(exit.rsplit(' ').next().expect("a code"), 16)
            .expect("a hexadecimal code");
        let out = std::process::Command::new(&exe)
            .args([
                "crt_ccs::ccs_case_child",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CASE_VAR, case.to_string())
            .env(SCRIPT_VAR, script)
            .output()
            .expect("run the case");
        let got = String::from_utf8_lossy(&out.stdout);
        // the harness may have begun the line
        let got_line = got
            .lines()
            .find_map(|l| l.split_once(OUT_MARK).map(|(_, out)| out));
        assert_eq!(
            out.status.code(),
            Some(seen(code)),
            "case {case}: exit\n{got}"
        );
        if code == 0 {
            assert_eq!(got_line, line, "case {case}");
        }
    }
    assert_eq!(case, count, "case count");
}
