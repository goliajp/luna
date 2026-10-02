//! Opt-in (`LUNA_OFFICIAL_BYTE_DIFF`) comparison of what a file prints on luna and on PUC.

use super::*;

/// Byte-diff stdout capture preamble. Opt-in via
/// `LUNA_OFFICIAL_BYTE_DIFF=1` env var.
/// Redirects `_G.print` and `_G.io.write` to append to a global
/// buffer `_G.__luna_official_stdout` which the harness reads back
/// after the chunk runs. Mirrors `crates/luna-core/tests/it/diff_puc.rs`
/// pattern.
///
/// Only applied when the env var is set — default path is
/// unchanged so existing assert-coverage semantics are preserved.
/// When enabled, the harness also spawns PUC per file, byte-diffs the
/// two buffers and tags divergences `[STDOUT-DIVERGE]`.
pub(super) const BYTE_DIFF_PREAMBLE: &[u8] = b"do _G.__luna_official_stdout='' _G.print=function(...) local t={} local n=select('#',...) for i=1,n do t[i]=tostring(select(i,...)) end _G.__luna_official_stdout=_G.__luna_official_stdout..table.concat(t,'\\t')..'\\n' end _G.io.write=function(...) local t={} local n=select('#',...) for i=1,n do t[i]=tostring(select(i,...)) end _G.__luna_official_stdout=_G.__luna_official_stdout..table.concat(t) end end ";

/// Postamble that emits the captured buffer to
/// real stdout bracketed by sentinel markers. `io.stdout:write`
/// bypasses the shadowed `_G.io.write` function because file-
/// handle methods use the C write directly (not the redefined
/// Lua function).
///
/// On the PUC side, the harness extracts the buffer between the
/// markers from the subprocess stdout. On the luna side, the
/// buffer is read directly via `read_byte_diff_stdout` from Vm
/// globals — the postamble output goes to the test process stdout
/// (unused).
pub(super) const BYTE_DIFF_POSTAMBLE: &[u8] = b" io.stdout:write('\\n===LUNA_BYTE_DIFF_START===\\n') io.stdout:write(_G.__luna_official_stdout or '') io.stdout:write('\\n===LUNA_BYTE_DIFF_END===\\n')";

/// Sentinel markers used by `BYTE_DIFF_POSTAMBLE`
/// to bracket the captured buffer in PUC's subprocess stdout.
pub(super) const BYTE_DIFF_START_MARKER: &[u8] = b"===LUNA_BYTE_DIFF_START===\n";

pub(super) const BYTE_DIFF_END_MARKER: &[u8] = b"\n===LUNA_BYTE_DIFF_END===\n";

/// Allowlist of files where the byte-diff preamble
/// interferes with the file's own tests (introspects `print` /
/// `io.write` C-function status via `debug.upvaluejoin(print, ...)`
/// which must fail on a C function; our wrapper is a Lua function).
///
/// Files in this list SKIP the byte-diff preamble entirely (default
/// `LUNA_OFFICIAL_BYTE_DIFF=1` path). They still run the assert-
/// counter wrapper as usual; only the print/io.write shadowing is
/// omitted.
///
/// Scoping cap: ≤5/dialect
/// Current counts (max 3/dialect) comfortably within budget:
///
/// - Lua5.1: `attrib.lua`, `files.lua`
/// - Lua5.2/5.3/5.4: `calls.lua`, `closure.lua`, `files.lua`
/// - Lua5.5: `calls.lua`, `closure.lua`
///
/// Each entry documented with the specific offending assertion.
pub(super) const BYTE_DIFF_ALLOWLIST: &[(LuaVersion, &str)] = &[
    // 5.5 — `assert(not pcall(debug.upvaluejoin, print, 1, ...))`
    // at closure.lua:275; similar shape in calls.lua.
    (LuaVersion::Lua55, "calls.lua"),
    (LuaVersion::Lua55, "closure.lua"),
    // 5.4 — same shape + files.lua:88 relies on original io.write
    // for a specific write-error path. bwcoercion.lua captures
    // `local print = print` at line 5 and derefs it in a way our
    // Lua-function wrapper trips (error attributed to line 79 = EOF).
    (LuaVersion::Lua54, "calls.lua"),
    (LuaVersion::Lua54, "closure.lua"),
    (LuaVersion::Lua54, "files.lua"),
    (LuaVersion::Lua54, "bwcoercion.lua"),
    // 5.4 tracegc.lua uses file-scope local variables read via
    // top-level references that leak past `return` — under our
    // function-wrapped body they become inaccessible.
    (LuaVersion::Lua54, "tracegc.lua"),
    // 5.3 — same shape as 5.4.
    (LuaVersion::Lua53, "calls.lua"),
    (LuaVersion::Lua53, "closure.lua"),
    (LuaVersion::Lua53, "files.lua"),
    // 5.2 — same shape as 5.4.
    (LuaVersion::Lua52, "calls.lua"),
    (LuaVersion::Lua52, "closure.lua"),
    (LuaVersion::Lua52, "files.lua"),
    // 5.1 — attrib.lua:68 checks print's setfenv behavior;
    // files.lua:33 similar io.write assumption as 5.4.
    (LuaVersion::Lua51, "attrib.lua"),
    (LuaVersion::Lua51, "files.lua"),
];

/// Returns `true` when the given (version, file)
/// pair is on `BYTE_DIFF_ALLOWLIST`. Byte-diff preamble skipped
/// for allowlisted files.
#[allow(dead_code)]
pub(super) fn byte_diff_should_skip(version: LuaVersion, name: &str) -> bool {
    BYTE_DIFF_ALLOWLIST
        .iter()
        .any(|(v, n)| *v == version && *n == name)
}

/// Read the byte-diff stdout capture buffer set by
/// `BYTE_DIFF_PREAMBLE`. Returns `None` when the global is absent
/// (the env var was off, or the chunk errored before the preamble
/// installed the buffer). Bytes come out of the Lua string
/// unchanged — no re-encoding.
#[allow(dead_code)]
pub(super) fn read_byte_diff_stdout(vm: &mut Vm) -> Option<Vec<u8>> {
    let k = Value::Str(vm.heap.intern(b"__luna_official_stdout"));
    let globals = vm.globals();
    match globals.get(k) {
        Value::Str(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    }
}

/// Resolve the per-dialect PUC interpreter path.
/// Mirrors `crates/luna-core/tests/it/diff_puc.rs::puc_bin_for`.
/// Returns `None` when the env var is unset for a non-5.5 dialect;
/// 5.5 falls back to `PUC_LUA` env then bare `lua5.5` in PATH.
#[allow(dead_code)]
pub(super) fn puc_bin_for_version(version: LuaVersion) -> Option<String> {
    let env_key = match version {
        LuaVersion::Lua51 => "PUC_LUA_51",
        LuaVersion::Lua52 => "PUC_LUA_52",
        LuaVersion::Lua53 => "PUC_LUA_53",
        LuaVersion::Lua54 => "PUC_LUA_54",
        LuaVersion::Lua55 => "PUC_LUA_55",
        // MacroLua is a compat variant that inherits 5.4 semantics
        // (per version.rs comment); byte-diff against PUC 5.4.
        LuaVersion::MacroLua => "PUC_LUA_54",
    };
    if let Ok(b) = std::env::var(env_key) {
        return Some(b);
    }
    if matches!(version, LuaVersion::Lua55) {
        return Some(std::env::var("PUC_LUA").unwrap_or_else(|_| "lua5.5".to_string()));
    }
    None
}

/// Canonicalize the byte-diff buffer to strip
/// impl-defined output that PUC and luna print differently for
/// legitimate reasons:
///
/// 1. **Hex addresses in `tostring(table/function/userdata/thread)`**
///    → `<ADDR>` placeholder. PUC and luna both format these as
///    `<type>: 0x[0-9a-f]+` but the actual address differs per run.
///
/// 2. **Chunk-name in `debug.getinfo(...).source` and similar** →
///    `[string "..."]` bodies normalized to `[string "<CHUNK>"]`.
///    The bracketed body captures the *first line* of the source,
///    which for our preambled chunks starts with `do _G.__luna_
///    assert_total...` — that's a harness artifact, not the file's
///    real content.
///
/// This is intentionally byte-level scanning (no regex crate) to
/// keep the harness in the tests dir without adding a dev-dep to
/// luna-core.
///
/// `BYTE_DIFF_ALLOWLIST` handles files where even canonicalization
/// leaves legitimate divergence (gc.lua memory counts, sort.lua
/// randomseed pointers when seeded differently, etc.).
#[allow(dead_code)]
pub(super) fn canonicalize_byte_diff(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Hex-address scrub: `0x` followed by 1+ hex chars → `0x<ADDR>`.
        // Only when preceded by `: ` (matches the `type: 0xADDR` shape).
        if i + 2 <= bytes.len() && &bytes[i..i + 2] == b"0x" {
            let prev_is_type_sep = i >= 2 && &bytes[i - 2..i] == b": ";
            if prev_is_type_sep {
                let mut j = i + 2;
                while j < bytes.len() && bytes[j].is_ascii_hexdigit() {
                    j += 1;
                }
                if j > i + 2 {
                    out.extend_from_slice(b"0x<ADDR>");
                    i = j;
                    continue;
                }
            }
        }
        // Chunk-name normalization: `[string "..."]` → `[string "<CHUNK>"]`.
        // Match the opening `[string "` and scan for the closing `"]`.
        const OPEN: &[u8] = b"[string \"";
        const CLOSE: &[u8] = b"\"]";
        if i + OPEN.len() <= bytes.len() && &bytes[i..i + OPEN.len()] == OPEN {
            // Find the CLOSE marker. If missing (truncated / malformed),
            // fall through to normal copy.
            let after_open = i + OPEN.len();
            if let Some(off) = bytes[after_open..]
                .windows(CLOSE.len())
                .position(|w| w == CLOSE)
            {
                out.extend_from_slice(b"[string \"<CHUNK>\"]");
                i = after_open + off + CLOSE.len();
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Extract the byte-diff buffer content from PUC's
/// subprocess stdout, using the sentinel markers emitted by
/// `BYTE_DIFF_POSTAMBLE`. Returns `None` when either marker is
/// missing (postamble was skipped, e.g. early `os.exit(0)` or
/// PUC errored before postamble ran) — the byte-diff comparison
/// then reports as "no PUC data" rather than divergence.
#[allow(dead_code)]
pub(super) fn extract_byte_diff_from_puc_stdout(bytes: &[u8]) -> Option<Vec<u8>> {
    let s = bytes
        .windows(BYTE_DIFF_START_MARKER.len())
        .position(|w| w == BYTE_DIFF_START_MARKER)?;
    let start = s + BYTE_DIFF_START_MARKER.len();
    let rest = &bytes[start..];
    let e = rest
        .windows(BYTE_DIFF_END_MARKER.len())
        .position(|w| w == BYTE_DIFF_END_MARKER)?;
    Some(rest[..e].to_vec())
}

/// Spawn PUC on the given source file and capture
/// stdout as raw bytes. `source` is passed via stdin (matching
/// diff_puc.rs's `-` invocation). Returns `None` when the binary is
/// missing (dev-machine friendliness). PUC-side errors (non-zero
/// exit / stderr) surface as `Err` so the harness can decide whether
/// to allowlist or fail — different files legitimately error at the
/// PUC layer (e.g. `attrib.lua` when the sub-package section can't
/// write `libs/P1/`).
///
/// Bytes come back unchanged — canonicalization (source-path
/// normalization, hex-address scrub) is a separate pass in
/// `canonicalize_byte_diff`.
#[allow(dead_code)]
pub(super) fn run_official_on_puc(bin: &str, source: &[u8]) -> Option<Result<Vec<u8>, String>> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = match Command::new(bin)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return None, // binary missing
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(source);
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return Some(Err(format!("PUC wait failed: {e}"))),
    };
    if !out.status.success() {
        return Some(Err(format!(
            "PUC non-zero exit (status={:?} stderr={})",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Some(Ok(out.stdout))
}

#[cfg(test)]
mod byte_diff_tests {
    use super::*;

    #[test]
    fn canonicalize_hex_address_after_type_sep() {
        let input = b"result is table: 0xdeadbeef and function: 0x123abc";
        let out = canonicalize_byte_diff(input);
        assert_eq!(
            &out[..],
            b"result is table: 0x<ADDR> and function: 0x<ADDR>"
        );
    }

    #[test]
    fn canonicalize_leaves_hex_without_type_sep() {
        // 0x prefix NOT preceded by ": " should pass through unchanged
        // (e.g. numeric literals in output).
        let input = b"count=0xff bytes";
        let out = canonicalize_byte_diff(input);
        assert_eq!(&out[..], b"count=0xff bytes");
    }

    #[test]
    fn canonicalize_chunk_name() {
        let input = b"[string \"do _G.__luna_assert_total=0 ...\"]:5: something";
        let out = canonicalize_byte_diff(input);
        assert_eq!(&out[..], b"[string \"<CHUNK>\"]:5: something");
    }

    #[test]
    fn canonicalize_preserves_regular_text() {
        let input = b"hello world 42 no addresses here";
        let out = canonicalize_byte_diff(input);
        assert_eq!(&out[..], input);
    }

    #[test]
    fn extract_byte_diff_roundtrip() {
        let payload = b"line 1\nline 2\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(b"some prefix output\n");
        buf.extend_from_slice(BYTE_DIFF_START_MARKER);
        buf.extend_from_slice(payload);
        buf.extend_from_slice(BYTE_DIFF_END_MARKER);
        buf.extend_from_slice(b"trailer\n");
        let extracted = extract_byte_diff_from_puc_stdout(&buf).expect("markers present");
        assert_eq!(extracted, payload);
    }

    #[test]
    fn extract_byte_diff_missing_marker() {
        let buf = b"no markers here at all";
        assert!(extract_byte_diff_from_puc_stdout(buf).is_none());
    }
}
