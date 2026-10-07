//! Non-ASCII file names and environment values on Windows, as the MSVC C
//! library's narrow functions take them: through the ANSI code page, so a
//! name given in UTF-8 reaches the system as the code page reads those
//! bytes (`é` as `Ã©` under code page 1252), and a value comes back in the
//! code page, `?` for a character it has none for. `crt_text/names.lua`
//! was run by PUC 5.1.5 to 5.5.0 built with MSVC on windows-latest (code
//! page 1252), with the files and variables made here; on a machine with
//! another code page the test says so and stops. Its own binary: it sets
//! environment variables.
#![cfg(windows)]

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = include_str!("crt_text/names.lua");
const PUC_51: &str = include_str!("crt_text/names.5.1.txt");
const PUC: &str = include_str!("crt_text/names.5.4.txt");
/// The files left in the directory, as code points, `l-`, `r-`, `w-` and
/// `x-` ones only.
const DIR_AFTER: &str = include_str!("crt_text/names.dir.txt");

/// A line with what differs between runs taken out: a function's address,
/// and a temporary name's directory and number.
fn normalize(line: &str) -> String {
    if let Some(at) = line.find("function: ") {
        return format!("{}function", &line[..at]);
    }
    if let Some(rest) = line.strip_prefix("tmpname\t") {
        let mark = rest
            .rfind("\\92s")
            .expect("a tmpnam name after the directory");
        // the number counts up through the process: PUC's run was the
        // first name, this test's each dialect's
        let name = &rest[mark + 4..];
        assert!(
            name.split_once('.').is_some_and(|(_, n)| !n.is_empty()),
            "{line}"
        );
        return "tmpname\t<dir>\\92s<pid>.<n>".to_string();
    }
    line.to_string()
}

fn code_points(name: &std::ffi::OsStr) -> String {
    use std::os::windows::ffi::OsStrExt;
    name.encode_wide()
        .map(|u| format!("{u:04X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn names_and_environment_go_through_the_ansi_code_page_as_in_puc() {
    if luna_core::stdio::os_bytes(std::ffi::OsStr::new("é€")) != [0xE9, 0x80] {
        eprintln!("the ANSI code page is not 1252: the recording does not apply");
        return;
    }
    // what the recording's run had around it
    // SAFETY: the test binary's only thread sets them before any Vm runs
    unsafe {
        std::env::set_var("NONASCII", "é€");
        std::env::set_var("NONASCII_RI", "日");
        std::env::set_var("Vé", "ve");
    }
    let dir = std::env::temp_dir().join(format!("luna-names-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the work dir");
    for n in ["é.txt", "€.txt", "日.txt", "ÿ.txt"] {
        for p in ["x-", "l-"] {
            std::fs::write(dir.join(format!("{p}{n}")), "return 1").expect("make a file");
        }
    }
    let mut prefix = luna_core::stdio::os_bytes(dir.as_os_str());
    prefix.push(b'\\');
    let dialects = [
        (LuaVersion::Lua51, PUC_51),
        (LuaVersion::Lua52, PUC),
        (LuaVersion::Lua53, PUC),
        (LuaVersion::Lua54, PUC),
        (LuaVersion::Lua55, PUC),
    ];
    for (v, want) in dialects {
        // the files the previous dialect renamed and removed, back in place
        for n in ["é.txt", "€.txt", "日.txt", "ÿ.txt"] {
            for p in ["x-", "l-"] {
                std::fs::write(dir.join(format!("{p}{n}")), "return 1").expect("make a file");
            }
        }
        for e in std::fs::read_dir(&dir).expect("list the work dir") {
            let e = e.expect("an entry");
            let name = e.file_name();
            if name.to_string_lossy().starts_with("r-") {
                std::fs::remove_file(e.path()).expect("remove a renamed file");
            }
        }
        let mut src = b"DIR = \"".to_vec();
        for &b in &prefix {
            src.extend_from_slice(format!("\\{b}").as_bytes());
        }
        src.extend_from_slice(
            b"\"
             local OUT = {}
             print = function(...)
               local t = {}
               for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end
               OUT[#OUT + 1] = table.concat(t, '\\t')
             end
             do ",
        );
        src.extend_from_slice(SCRIPT.as_bytes());
        src.extend_from_slice(b"\n end\n return table.concat(OUT, '\\n') .. '\\n'");
        let mut vm = Vm::new(v);
        vm.set_crt_text_mode(true);
        let f = vm
            .load(&src, b"=names")
            .unwrap_or_else(|e| panic!("{v:?}: {e}"));
        let out = match vm.call_value(Value::Closure(f), &[]) {
            Ok(r) => match r.first() {
                Some(Value::Str(s)) => s.as_bytes().to_vec(),
                other => panic!("{v:?}: the script returned {other:?}"),
            },
            Err(e) => panic!("{v:?}: {}", vm.error_text(&e)),
        };
        drop(vm);
        // the messages name the files with the directory in front
        let got = String::from_utf8_lossy(&out).replace(&*String::from_utf8_lossy(&prefix), "");
        let want = want.replace("\r\n", "\n");
        for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
            assert_eq!(normalize(g), normalize(w), "{v:?}, line {}", i + 1);
        }
        assert_eq!(
            got.lines().count(),
            want.lines().count(),
            "{v:?}: line count"
        );
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .expect("list the work dir")
            .map(|e| code_points(&e.expect("an entry").file_name()))
            .filter(|s| {
                ["006C 002D", "0072 002D", "0077 002D", "0078 002D"]
                    .iter()
                    .any(|p| s.starts_with(p))
            })
            .collect();
        left.sort();
        let want_dir: Vec<String> = DIR_AFTER.lines().map(str::to_string).collect();
        assert_eq!(left, want_dir, "{v:?}: the files left");
    }
    std::fs::remove_dir_all(&dir).expect("remove the work dir");
}
