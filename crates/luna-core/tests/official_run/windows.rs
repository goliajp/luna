//! 5.1 files.lua on Windows, against what PUC 5.1.5 built with MSVC does
//! with it there. Windows removes and renames no open file, so PUC stops at
//! files.lua:59 (`os.rename` of the file `io.input` still holds);
//! files_win.lua, the same file with the two open files closed first, runs
//! on to :245, where the library's `"line"` buffering is full buffering.
//! Both run with the Vm's files kept as that library keeps them, and must
//! end with the error PUC ended with (the temporary file's name aside).

use luna_core::version::LuaVersion;

/// What PUC printed on standard error for `name`, when it was recorded.
pub(super) fn recording(version: LuaVersion, name: &str) -> Option<&'static str> {
    if !cfg!(windows) || version != LuaVersion::Lua51 {
        return None;
    }
    match name {
        "files.lua" => Some(include_str!("../official/windows/files.5.1.txt")),
        "files_win.lua" => Some(include_str!("../official/windows/files_win.5.1.txt")),
        _ => None,
    }
}

/// The error message, with a temporary file's path in it replaced.
fn normalize(msg: &str) -> String {
    let Some(at) = msg.find("\\Temp\\s") else {
        return msg.to_string();
    };
    let start = msg[..at].rfind(": ").map_or(0, |i| i + 2);
    let end = msg[at..].find(": ").map_or(msg.len(), |i| at + i);
    format!("{}<tmpname>{}", &msg[..start], &msg[end..])
}

/// `None` when luna's run of `name` ended as PUC's did; otherwise what
/// differs.
pub(super) fn compare(name: &str, recorded: &str, error: Option<&str>) -> Option<String> {
    let first = recorded.lines().next().expect("a recorded error line");
    let want = first.split_once(".exe: ").map_or(first, |(_, m)| m);
    let got = error.map(|e| e.strip_prefix("runtime: ").unwrap_or(e));
    match got {
        Some(g) if normalize(g) == normalize(want) => None,
        _ => Some(format!(
            "{name}: PUC on Windows ended with {want:?}, luna with {got:?}"
        )),
    }
}
