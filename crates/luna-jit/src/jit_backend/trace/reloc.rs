//! The Vm-specific addresses in a trace's machine code: where they sit, and
//! how the code another Vm installs gets that Vm's addresses written over
//! them (see `super::image`).

use super::*;

/// Where relocation `n` sits in a function's code: eight little-endian
/// bytes holding the address (Cranelift's `Abs8`, the immediate of an
/// x86-64 `movabs`, an AArch64 literal-pool entry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Site {
    pub(crate) at: u32,
    /// Index into the trace's relocation list.
    pub(crate) n: u32,
}

/// Writes `v` at `site` of `code`.
pub(crate) fn patch(code: &mut [u8], site: Site, v: i64) {
    let at = site.at as usize;
    code[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Machine code taken from a compiled trace: what another Vm copies.
#[derive(Clone, Debug)]
pub(crate) struct Code {
    pub(crate) bytes: Box<[u8]>,
    pub(crate) sites: Box<[Site]>,
}

impl Code {
    /// The code with relocation `n` holding `vals[n]`.
    pub(crate) fn relocated(&self, vals: &[i64]) -> Vec<u8> {
        let mut out = self.bytes.to_vec();
        for &s in self.sites.iter() {
            patch(&mut out, s, vals[s.n as usize]);
        }
        out
    }
}

thread_local! {
    /// The values the relocation symbols of the Cranelift module being
    /// finalized on this thread resolve to (see [`resolve_symbol`]).
    static VALUES: std::cell::RefCell<Vec<(RelocKind, i64)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Where the relocations sit in the function Cranelift compiled last on
    /// this thread, and its length; `None` when its code cannot move to
    /// another address.
    static LAST_SITES: std::cell::RefCell<Option<(usize, Vec<Site>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Makes the relocation symbols of the next module finalized on this
/// thread resolve to the addresses in `vals`.
pub(crate) fn set_values(vals: &[(RelocKind, i64)]) {
    VALUES.with(|v| {
        let mut v = v.borrow_mut();
        v.clear();
        v.extend_from_slice(vals);
    });
}

/// Makes relocation symbol `n` resolve to `live` (of `kind`).
pub(crate) fn set_value(n: usize, kind: RelocKind, live: i64) {
    VALUES.with(|v| {
        let mut v = v.borrow_mut();
        if v.len() <= n {
            v.resize(n + 1, (kind, 0));
        }
        v[n] = (kind, live);
    });
}

/// The relocations [`set_values`] set last on this thread.
pub(crate) fn values() -> Vec<(RelocKind, i64)> {
    VALUES.with(|v| v.borrow().clone())
}

/// The address a relocation symbol `name` stands for (see [`set_values`]).
pub(crate) fn resolve_symbol(name: &str) -> Option<*const u8> {
    let n: usize = name.strip_prefix("__luna_reloc_")?.parse().ok()?;
    VALUES.with(|v| v.borrow().get(n).map(|&(_, x)| x as usize as *const u8))
}

/// Notes where the relocations sit in the function `ctx` just compiled
/// into `module` behind `lead` bytes (see [`take_sites`]).
fn note_sites<M: Module>(module: &M, ctx: &cranelift_codegen::Context, lead: u32) {
    let sites = ctx.compiled_code().and_then(|cc| {
        let mut sites = Vec::new();
        for r in cc.buffer.relocs() {
            use cranelift_codegen::FinalizedRelocTarget as T;
            use cranelift_codegen::binemit::Reloc;
            use cranelift_codegen::ir::ExternalName;
            let ExternalName::User(u) = (match &r.target {
                T::ExternalName(n) => n,
                // an absolute address inside the function does not move
                // with a copy of it
                T::Func(_) => return None,
            }) else {
                // a library routine Cranelift calls: an absolute address in
                // the process, the same for every Vm, if absolute
                if r.kind == Reloc::Abs8 {
                    continue;
                }
                return None;
            };
            if r.kind != Reloc::Abs8 || r.addend != 0 {
                return None;
            }
            let name = &ctx.func.params.user_named_funcs()[*u];
            if name.namespace != 1 {
                // a helper: the same address in every Vm
                continue;
            }
            let id = cranelift_module::DataId::from_u32(name.index);
            let decl = module.declarations().get_data_decl(id);
            let n = decl
                .name
                .as_deref()
                .and_then(|s| s.strip_prefix("__luna_reloc_"))
                .and_then(|s| s.parse::<u32>().ok())?;
            sites.push(Site {
                at: r.offset + lead,
                n,
            });
        }
        Some((lead as usize + cc.code_buffer().len(), sites))
    });
    LAST_SITES.with(|s| *s.borrow_mut() = sites);
}

/// The byte a trace's loop head starts at a multiple of.
const LOOP_ALIGN: u32 = 16;

/// Compiles `ctx` into function `id` of `module` with no-ops in front, so
/// that its loop head (the target of its last back edge) starts at a
/// multiple of [`LOOP_ALIGN`]: where the loop starts in the fetch blocks
/// changed a table loop's speed by 4% (`tbl`).
pub(crate) fn define_aligned<M: Module>(
    module: &mut M,
    id: FuncId,
    ctx: &mut cranelift_codegen::Context,
) -> Option<()> {
    ctx.compile(module.isa(), &mut Default::default()).ok()?;
    let cc = ctx.compiled_code()?;
    let head = cc
        .bb_edges
        .iter()
        .filter(|e| e.1 <= e.0)
        .max_by_key(|e| e.0);
    let lead = head.map_or(0, |&(_, to)| (LOOP_ALIGN - to % LOOP_ALIGN) % LOOP_ALIGN);
    let mut bytes = Vec::with_capacity(lead as usize + cc.code_buffer().len());
    nops(&mut bytes, lead as usize);
    bytes.extend_from_slice(cc.code_buffer());
    let relocs: Vec<_> = (cc.buffer.relocs().iter())
        .map(|r| {
            let mut m = cranelift_module::ModuleReloc::from_mach_reloc(r, &ctx.func, id);
            m.offset += lead;
            m
        })
        .collect();
    let align = u64::from(cc.buffer.alignment.max(LOOP_ALIGN));
    module
        .define_function_bytes(id, align, &bytes, &relocs)
        .ok()?;
    super::code_dump::note_len(bytes.len());
    note_sites(module, ctx, lead);
    Some(())
}

/// `n` bytes of no-ops, `n` a multiple of the instruction size.
fn nops(out: &mut Vec<u8>, n: usize) {
    #[cfg(target_arch = "aarch64")]
    for _ in 0..n / 4 {
        out.extend_from_slice(&0xd503_201f_u32.to_le_bytes());
    }
    #[cfg(target_arch = "x86_64")]
    {
        // the multi-byte forms of `nop` Intel recommends, by length
        const NOPS: [&[u8]; 8] = [
            &[0x90],
            &[0x66, 0x90],
            &[0x0f, 0x1f, 0x00],
            &[0x0f, 0x1f, 0x40, 0x00],
            &[0x0f, 0x1f, 0x44, 0x00, 0x00],
            &[0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00],
            &[0x0f, 0x1f, 0x80, 0x00, 0x00, 0x00, 0x00],
            &[0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
        ];
        let mut left = n;
        while left > 0 {
            let k = left.min(NOPS.len());
            out.extend_from_slice(NOPS[k - 1]);
            left -= k;
        }
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    out.resize(out.len() + n, 0);
}

/// What [`note_sites`] noted last on this thread.
pub(crate) fn take_sites() -> Option<(usize, Vec<Site>)> {
    LAST_SITES.with(|s| s.borrow_mut().take())
}

/// A copy of the `len` bytes of finalized code at `entry`, with `sites`.
///
/// # Safety
///
/// `entry..entry + len` is mapped, readable code.
pub(crate) unsafe fn copy_code(entry: *const u8, len: usize, sites: Vec<Site>) -> Code {
    // SAFETY: forwarded from the caller
    let bytes = unsafe { std::slice::from_raw_parts(entry, len) };
    Code {
        bytes: bytes.into(),
        sites: sites.into(),
    }
}
