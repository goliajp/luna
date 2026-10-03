//! The Vm-specific addresses in a trace's machine code: where they sit, and
//! how the code another Vm installs gets that Vm's addresses written over
//! them (see `super::image`).

use super::*;

/// Where relocation `n` sits in a function's code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Site {
    /// Byte offset of the first byte the address is written in.
    pub(crate) at: u32,
    /// Index into the trace's relocation list.
    pub(crate) n: u32,
    pub(crate) form: Form,
}

/// How an address is encoded at a [`Site`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Form {
    /// Eight little-endian bytes (Cranelift's `Abs8`, x86-64 `movabs`).
    Abs8,
    /// AArch64 `movz` and three `movk`, one 16-bit piece each.
    MovzMovk,
}

/// Writes `v` at `site` of `code`.
pub(crate) fn patch(code: &mut [u8], site: Site, v: i64) {
    let at = site.at as usize;
    match site.form {
        Form::Abs8 => code[at..at + 8].copy_from_slice(&v.to_le_bytes()),
        Form::MovzMovk => {
            for k in 0..4 {
                let p = at + 4 * k;
                let w = u32::from_le_bytes(code[p..p + 4].try_into().expect("four bytes"));
                let piece = ((v as u64 >> (16 * k)) & 0xffff) as u32;
                let w = (w & !(0xffff << 5)) | (piece << 5);
                code[p..p + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
    }
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
/// into `module` (see [`take_sites`]).
pub(crate) fn note_sites<M: Module>(module: &M, ctx: &cranelift_codegen::Context) {
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
                at: r.offset,
                n,
                form: Form::Abs8,
            });
        }
        Some((cc.code_buffer().len(), sites))
    });
    LAST_SITES.with(|s| *s.borrow_mut() = sites);
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
