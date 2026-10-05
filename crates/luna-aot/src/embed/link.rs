//! Target-aware object emission and the final gcc-style link.

use std::fs;
use std::path::Path;

use luna_core::version::LuaVersion;

use object::write::{Object, Symbol, SymbolSection};
use object::{SectionKind, SymbolKind, SymbolScope};

use super::AotError;
use super::msvc_link::link_aot_binary_msvc;
use super::target::{TargetLibc, TargetOs, TargetSpec};
use crate::{BYTECODE_END_SYMBOL, BYTECODE_SECTION_NAME, BYTECODE_START_SYMBOL};

/// Target-aware variant of [`write_bytecode_object`]. Emits a `.o`
/// using the target's format/arch/endian rather than the host's.
pub(super) fn write_bytecode_object_for(
    bytecode: &[u8],
    out: &Path,
    target: &TargetSpec,
) -> Result<(), AotError> {
    let mut obj = Object::new(target.format, target.arch, target.endian);

    let section_id = obj.add_section(
        Vec::new(),
        BYTECODE_SECTION_NAME.as_bytes().to_vec(),
        SectionKind::ReadOnlyData,
    );
    let _start_offset = obj.append_section_data(section_id, bytecode, 1);

    let start_name = BYTECODE_START_SYMBOL.to_string();
    let end_name = BYTECODE_END_SYMBOL.to_string();
    let _start_sym = obj.add_symbol(Symbol {
        name: start_name.into_bytes(),
        value: 0,
        size: 0,
        kind: SymbolKind::Data,
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Section(section_id),
        flags: object::SymbolFlags::None,
    });
    let _end_sym = obj.add_symbol(Symbol {
        name: end_name.into_bytes(),
        value: bytecode.len() as u64,
        size: 0,
        kind: SymbolKind::Data,
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Section(section_id),
        flags: object::SymbolFlags::None,
    });

    let bytes = obj
        .write()
        .map_err(|e| AotError::Object(format!("Object::write: {e}")))?;
    fs::write(out, bytes)?;
    Ok(())
}

/// C definitions that make sure every section the deploy walker reads
/// exists in the link image even when no trace `.o` is linked.
fn section_placeholders(target: &TargetSpec) -> &'static str {
    // Guarantee the `luna_strkey_idx` section exists in the link
    // image even when the binary linked zero AOT trace `.o`s. Without
    // a defining input the bracket symbols `__start_luna_strkey_idx` /
    // `__stop_luna_strkey_idx` (or the Mach-O `section$start$...`
    // equivalents) are undefined and the link fails. Defining an
    // empty placeholder lets the deploy resolver see `start == end`
    // and short-circuit cleanly.
    //
    // The placeholder uses a `static` zero-length array marked
    // `used` so the C compiler emits the section header even though
    // nothing references it. On Mach-O the `section` attribute
    // takes a `"__SEG,__SECT"` pair; we use `__DATA,luna_strkey_idx`,
    // the same segment the lowerer puts its entries in. On ELF the
    // `section` attribute takes just the section name.
    // Also guarantee the `luna_trace_meta` section exists in the
    // link image when zero trace `.o`s linked in (small / non-loopy
    // sources where the warmup recorder didn't close any traces).
    // Same placeholder pattern as `luna_strkey_idx`.
    //
    // **Sized at 48 bytes** (matching `AotTraceIndexEntry::SIZE`) and
    // 8-byte aligned so when a real AOT-emitted trace .o lands in the
    // same section, the linker's section merge concatenates the
    // entries without misaligning anyone. The placeholder bytes are
    // all-zero; the deploy walker sees a single entry whose `fn_ptr`
    // is NULL and skips it (via the `entry.fn_ptr.is_null()` guard).
    //
    // `aligned(8)` is needed because the section's other entries
    // carry pointer relocations at offsets 24 / 32 — a misaligned
    // placeholder shifts those into bad lanes.
    // Strkey idx placeholder: sized to a full `IndexEntry` (16 bytes,
    // 8-byte aligned) so the deploy resolver's
    // `start + N * sizeof::<IndexEntry>` iteration lines up with
    // any real trace-emitted entries that follow. A `[1]`-sized
    // placeholder would land 7 bytes of zero pad between itself and
    // the first trace entry (the trace lower sets `align(8)`), which
    // mis-aligns the divide-by-16 entry count: the walker sees the
    // placeholder as half an entry and misses the real one by 8
    // bytes. Sizing the placeholder to 16 means the section
    // is exactly N+1 entries for N real traces; the placeholder's
    // zero-valued `bytes_ptr` short-circuits via the resolver's
    // `entry.bytes_ptr.is_null()` guard.
    // Also guarantee the `luna_inline_chnx` section exists when the
    // binary linked zero depth>0-inlined-cmp trace `.o`s. Same shape as the strkey idx
    // placeholder (16 bytes = one IndexEntry-sized slot) so the deploy
    // resolver's `start + N * sizeof::<IndexEntry>` walk lines up with
    // any real trace-emitted entries. Zero `bytes_ptr` field short-
    // circuits via the resolver's null guard.
    //
    // Mach-O sectname max is 16 chars; `luna_inline_chnx` is 15
    // (matches `luna_strkey_idx` sizing). Windows COFF short name cap
    // is 8 — `.lt_chai` mirrors `.lt_skix` / `.lt_meta`. Both names
    // must match the lowerer's section choice in
    // `emit_chain_ptr_arg` and the deploy resolver's bracket /
    // section-walker needles. `luna_proto_idx` / `.lt_prix` hold the
    // proto slots of inlined calls (`emit_proto_arg`), placeheld the same
    // way.
    match target.os {
        TargetOs::MacOs => {
            "__attribute__((used, section(\"__DATA,luna_strkey_idx\"), aligned(8)))\n\
             static const char luna_strkey_idx_placeholder[16] = {0};\n\
             __attribute__((used, section(\"__DATA,luna_trace_meta\"), aligned(8)))\n\
             static const char luna_trace_meta_placeholder[48] = {0};\n\
             __attribute__((used, section(\"__DATA,luna_inline_chnx\"), aligned(8)))\n\
             static const char luna_inline_chnx_placeholder[16] = {0};\n\
             __attribute__((used, section(\"__DATA,luna_proto_idx\"), aligned(8)))\n\
             static const char luna_proto_idx_placeholder[16] = {0};\n"
        }
        TargetOs::Linux => {
            "__attribute__((used, section(\"luna_strkey_idx\"), aligned(8)))\n\
             static const char luna_strkey_idx_placeholder[16] = {0};\n\
             __attribute__((used, section(\"luna_trace_meta\"), aligned(8)))\n\
             static const char luna_trace_meta_placeholder[48] = {0};\n\
             __attribute__((used, section(\"luna_inline_chnx\"), aligned(8)))\n\
             static const char luna_inline_chnx_placeholder[16] = {0};\n\
             __attribute__((used, section(\"luna_proto_idx\"), aligned(8)))\n\
             static const char luna_proto_idx_placeholder[16] = {0};\n"
        }
        // Windows COFF.
        //
        // PE/COFF section name headers are fixed 8 bytes
        // (`IMAGE_SECTION_HEADER::Name`), so we use deliberately
        // short names: `.lt_skix` for the strkey index (mirrors
        // `luna_strkey_idx`), `.lt_meta` for trace meta. Each is
        // exactly 8 bytes including the leading `.`, matching the
        // 8-byte PE section name field byte-for-byte without truncation
        // or string-table fallback (the COFF string-table mechanism
        // for long names is an object-file feature only — `link.exe`
        // / `lld-link` drop the long-name table when producing the
        // final PE image).
        //
        // Mirror placeholders so the sections exist even when the
        // binary linked zero AOT trace `.o`s — same shape as the
        // Mach-O / ELF placeholders above. The deploy walker
        // (`luna-runtime-helpers::windows_section::find_section`)
        // sees the placeholder bytes via the runtime PE-header
        // parse and short-circuits on the all-zero entry.
        //
        // MinGW's gcc accepts the `__attribute__((section(...)))`
        // syntax verbatim with the section name as-is (no leading
        // `__DATA,` prefix — that's Mach-O specific). For MSVC we
        // emit the equivalent `#pragma section` + `__declspec(allocate(...))`
        // form so the same placeholder data lands in the same
        // `.lt_skix` / `.lt_meta` sections regardless of toolchain;
        // the deploy walker (`luna-runtime-helpers::windows_section`)
        // looks up by section name and is toolchain-agnostic.
        //
        // clang-cl accepts both syntaxes (`__attribute__((section()))`
        // and the MSVC `__declspec(allocate())` form), but `cl.exe`
        // only accepts the MSVC form — so we emit the MSVC form for
        // both, which keeps a single source path covering both drivers.
        TargetOs::Windows if target.is_msvc() => {
            // `#pragma section` declares the section + its
            // characteristics (R = readable). The 8-byte alignment
            // matches the MinGW arm so the deploy walker's pointer
            // arithmetic over the section is identical across
            // toolchains.
            "#pragma section(\".lt_skix\", read)\n\
             __declspec(allocate(\".lt_skix\")) __declspec(align(8))\n\
             static const char luna_strkey_idx_placeholder[16] = {0};\n\
             #pragma section(\".lt_meta\", read)\n\
             __declspec(allocate(\".lt_meta\")) __declspec(align(8))\n\
             static const char luna_trace_meta_placeholder[48] = {0};\n\
             #pragma section(\".lt_chai\", read)\n\
             __declspec(allocate(\".lt_chai\")) __declspec(align(8))\n\
             static const char luna_inline_chnx_placeholder[16] = {0};\n\
             #pragma section(\".lt_prix\", read)\n\
             __declspec(allocate(\".lt_prix\")) __declspec(align(8))\n\
             static const char luna_proto_idx_placeholder[16] = {0};\n"
        }
        TargetOs::Windows => {
            "__attribute__((used, section(\".lt_skix\"), aligned(8)))\n\
             static const char luna_strkey_idx_placeholder[16] = {0};\n\
             __attribute__((used, section(\".lt_meta\"), aligned(8)))\n\
             static const char luna_trace_meta_placeholder[48] = {0};\n\
             __attribute__((used, section(\".lt_chai\"), aligned(8)))\n\
             static const char luna_inline_chnx_placeholder[16] = {0};\n\
             __attribute__((used, section(\".lt_prix\"), aligned(8)))\n\
             static const char luna_proto_idx_placeholder[16] = {0};\n"
        }
    }
}

/// Target-aware variant of [`write_aot_cmain_object`]. Generates the
/// same C source but invokes the target-specific cc driver so the
/// produced `.o` has the right ABI.
pub(super) fn write_aot_cmain_object_for(
    out: &Path,
    target: &TargetSpec,
    version: LuaVersion,
) -> Result<(), AotError> {
    let placeholder = section_placeholders(target);
    let dialect = dialect_code(version);

    let c_src = format!(
        r#"#include <stddef.h>
#include <stdint.h>

extern uint8_t __luna_bytecode_start[];
extern uint8_t __luna_bytecode_end[];
extern int luna_aot_run_dialect(const uint8_t *bytecode, size_t len, uint32_t dialect);

{placeholder}

int main(int argc, char **argv) {{
    (void)argc; (void)argv;
    size_t len = (size_t)(__luna_bytecode_end - __luna_bytecode_start);
    return luna_aot_run_dialect(__luna_bytecode_start, len, {dialect});
}}
"#
    );

    let mut c_path = out.to_path_buf();
    c_path.set_extension("c");
    fs::write(&c_path, c_src)?;

    // MSVC needs `clang-cl` / `cl.exe` (different flag shape: `/c` +
    // `/Fo:` vs gcc-style `-c` + `-o`). All other targets keep the
    // existing gcc-style cc driver path.
    let mut cmd = if target.is_msvc() {
        let Some(mut cl) = target.msvc_cc_command()? else {
            return Err(AotError::Link(format!(
                "no MSVC C compiler found for target {} — on a Windows host, \
                 install Visual Studio or the Build Tools with the \"Desktop \
                 development with C++\" workload (`cl.exe` is found without a \
                 Developer Command Prompt); on any host, LLVM's `clang-cl` and \
                 `lld-link` on PATH with an `xwin splat` sysroot named by \
                 LUNA_AOT_MSVC_SYSROOT (or left in cargo-xwin's cache) also \
                 work. Override with `CC=...` to point at a custom driver.",
                target.triple
            )));
        };
        // `clang-cl` / `cl.exe`: `/c` compile-only, `/Fo:<obj>` output
        // (one token, so a path with spaces survives).
        cl.arg("/c");
        cl.arg(format!("/Fo:{}", out.display()));
        cl.arg("/nologo");
        // the dynamic CRT, as the Rust staticlib is built against it; cl's
        // default static CRT (/MT) pulls libcmt.lib into the same link
        cl.arg("/MD");
        if cfg!(windows) {
            cl.arg(&c_path);
        } else {
            // only clang-cl runs off Windows. `--` ends its options: an
            // absolute Unix path such as `/Users/...` would otherwise
            // parse as the `/U` option
            cl.arg(format!("--target={}", target.triple));
            cl.arg("--").arg(&c_path);
        }
        cl
    } else {
        let mut cmd = target.cc_command();
        cmd.arg("-c").arg(&c_path).arg("-o").arg(out);
        cmd
    };
    let status = cmd
        .output()
        .map_err(|e| AotError::Link(format!("spawn cc for target {}: {e}", target.triple)))?;
    if !status.status.success() {
        return Err(AotError::Link(format!(
            "cc -c {} (target {}) failed (exit {:?}):\n{}",
            c_path.display(),
            target.triple,
            status.status.code(),
            String::from_utf8_lossy(&status.stderr)
        )));
    }
    Ok(())
}

/// Target-aware variant of [`link_aot_binary`]. Picks the cc driver,
/// per-OS lib set, and (for Windows) the MinGW vs MSVC path.
///
/// `traces_obj`: optional AOT-trace mcode `.o`
/// emitted by [`harvest_and_emit_aot_traces`]. When `Some`, the linker
/// pulls in the trace mcode + the `luna_trace_meta` / `luna_trace_blob`
/// data sections that the deploy walker reads at startup. When `None`,
/// the binary runs through interp + runtime-JIT fallback only.
pub(super) fn link_aot_binary_for(
    bytecode_obj: &Path,
    cmain_obj: &Path,
    traces_obj: Option<&Path>,
    staticlib: &Path,
    out_path: &Path,
    target: &TargetSpec,
) -> Result<(), AotError> {
    // MSVC has a completely different linker surface (`link.exe` /
    // `lld-link.exe`: `/OUT:foo.exe`, `/SUBSYSTEM:CONSOLE`, `.lib`
    // system libs, no `-l` flag). Route through a dedicated path;
    // everything else (Mach-O, ELF, MinGW PE-COFF) shares the gcc-style cc-driver path below.
    if target.is_msvc() {
        return link_aot_binary_msvc(
            bytecode_obj,
            cmain_obj,
            traces_obj,
            staticlib,
            out_path,
            target,
        );
    }

    let mut cmd = target.cc_command();

    // Object files first (they reference symbols defined in the
    // staticlib). Order matters for some traditional Unix linkers
    // (`ld` resolves left-to-right; modern `lld` is order-independent
    // but we keep the canonical order for portability).
    cmd.arg(cmain_obj).arg(bytecode_obj);
    if let Some(traces) = traces_obj {
        // Trace mcode `.o`. Placed after the
        // bytecode object (which references the AOT trace `luna_aot_
        // trace_*` symbols via its bracketed meta section's
        // relocations) so resolution flows correctly under traditional
        // left-to-right ld.
        cmd.arg(traces);
    }
    cmd.arg(staticlib);

    // Per-OS lib set — what `rustc --print native-static-libs` reports
    // for a `crate-type = ["staticlib"]` on each platform that pulls
    // std. The macOS + linux sets match the host path verbatim so the
    // cross arm links exactly what the host build does.
    match target.os {
        TargetOs::MacOs => {
            cmd.args(["-framework", "CoreFoundation"]);
            cmd.args(["-framework", "Security"]);
            cmd.arg("-liconv");
        }
        TargetOs::Linux => {
            cmd.arg("-lpthread");
            cmd.arg("-ldl");
            cmd.arg("-lm");
            // glibc-only libs; musl ships these symbols inside libc
            // so naming them here would be a `cannot find -lgcc_s` /
            // `cannot find -lutil` failure on Alpine.
            if target.libc != TargetLibc::Musl {
                cmd.arg("-lrt");
                cmd.arg("-lgcc_s");
                cmd.arg("-lutil");
            }
        }
        TargetOs::Windows => {
            // MinGW: rust stdlib's std::sys::windows shim needs these.
            // The set comes from `rustc --print native-static-libs --target=x86_64-pc-windows-gnu`
            // run on a probe staticlib in CI; we replicate the typical
            // dependency list here.
            cmd.arg("-luserenv");
            cmd.arg("-lkernel32");
            cmd.arg("-lws2_32");
            cmd.arg("-lbcrypt");
            cmd.arg("-ladvapi32");
            cmd.arg("-lntdll");
            // MinGW gcc adds its own startup; nothing more needed.
        }
    }

    cmd.arg("-o").arg(out_path);

    let output = cmd
        .output()
        .map_err(|e| AotError::Link(format!("spawn cc for target {}: {e}", target.triple)))?;
    if !output.status.success() {
        return Err(AotError::Link(format!(
            "cc link failed (target {}, exit {:?}):\ncommand: {:?}\nstderr:\n{}",
            target.triple,
            output.status.code(),
            cmd,
            String::from_utf8_lossy(&output.stderr),
        )));
    }
    Ok(())
}

/// The number `luna_aot_run_dialect` in `luna-runtime-helpers` maps back to
/// `version` (its `dialect_code`). The two crates do not depend on each
/// other, so the table is written twice; the per-dialect end-to-end tests
/// fail if they disagree.
fn dialect_code(version: LuaVersion) -> u32 {
    match version {
        LuaVersion::Lua51 => 51,
        LuaVersion::Lua52 => 52,
        LuaVersion::Lua53 => 53,
        LuaVersion::Lua54 => 54,
        LuaVersion::Lua55 => 55,
        LuaVersion::MacroLua => 254,
    }
}
