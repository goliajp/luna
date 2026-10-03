//! The method JIT's compiled functions shared between the Vms of one
//! [`crate::Engine`], as `trace::share` shares traces: a Vm that compiled a
//! function hands its machine code to the engine, and another Vm with a
//! function of the same content copies it into its own code memory, with
//! its own addresses written where the code held the first Vm's.

use super::storage::CraneliftJitStorage;
use super::trace::image::Content;
use super::*;
use cranelift_codegen::binemit::Reloc;
use std::sync::Arc;

/// What a relocation in a chunk's code refers to.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// Another of the chunk's functions.
    Local(u32),
    /// A string constant of the function: relocation `n` while capturing,
    /// the constant's index in the image.
    Const(u32),
}

#[derive(Clone, Debug)]
struct Rel {
    at: u32,
    kind: Reloc,
    target: Target,
    addend: i64,
}

#[derive(Clone, Debug)]
struct Func {
    id: u32,
    bytes: Box<[u8]>,
    rels: Box<[Rel]>,
}

/// The settings a function was compiled under: shared only between Vms
/// where they agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ChunkKey {
    version: u8,
    pre53: bool,
    float_only: bool,
    content: u64,
}

/// One compiled function, shareable between Vms.
pub(crate) struct ChunkImage {
    pub(crate) id: u64,
    content: Content,
    funcs: Box<[Func]>,
    entry: u32,
    meta: ChunkMeta,
    pub(crate) size: usize,
}

/// A function being compiled for a Vm that shares its code: the string
/// constants its code holds, by relocation, and its functions' code.
#[derive(Default)]
struct Capture {
    /// The string each relocation holds.
    strs: Vec<i64>,
    funcs: Vec<(u32, usize, Vec<Rel>)>,
    /// Something in the code would not survive a move to another address.
    unshareable: bool,
}

thread_local! {
    static CAPTURE: std::cell::RefCell<Option<Capture>> = const { std::cell::RefCell::new(None) };
    static CHUNK_CODEGEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Functions the method JIT compiled to machine code on this thread.
#[doc(hidden)]
pub fn chunk_codegen_count() -> u64 {
    CHUNK_CODEGEN.with(|c| c.get())
}

pub(crate) fn count_codegen() {
    CHUNK_CODEGEN.with(|c| c.set(c.get() + 1));
}

/// The address of `key`, a string constant of the function being compiled:
/// a relocation when the code is being captured for sharing.
pub(crate) fn str_arg<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    key: Gc<LuaStr>,
) -> Value {
    let live = key.as_ptr() as i64;
    let n = CAPTURE.with(|c| {
        let mut c = c.borrow_mut();
        let cap = c.as_mut()?;
        Some(match cap.strs.iter().position(|&x| x == live) {
            Some(n) => n,
            None => {
                cap.strs.push(live);
                cap.strs.len() - 1
            }
        })
    });
    let Some(n) = n else {
        return bcx.ins().iconst(types::I64, live);
    };
    super::trace::reloc::set_value(n, super::trace::RelocKind::Str, live);
    let id = module
        .declare_data(
            &super::trace::reloc_symbol(n),
            Linkage::Import,
            false,
            false,
        )
        .expect("declaring a relocation symbol");
    let gv = module.declare_data_in_func(id, bcx.func);
    bcx.ins().symbol_value(types::I64, gv)
}

/// Notes the relocations of function `id`, which `ctx` just compiled into
/// `module`, when the code is being captured.
pub(crate) fn note<M: Module>(module: &M, ctx: &cranelift_codegen::Context, id: FuncId) {
    CAPTURE.with(|c| {
        let mut c = c.borrow_mut();
        let Some(cap) = c.as_mut() else {
            return;
        };
        let Some(cc) = ctx.compiled_code() else {
            cap.unshareable = true;
            return;
        };
        let mut rels = Vec::new();
        for r in cc.buffer.relocs() {
            use cranelift_codegen::FinalizedRelocTarget as T;
            use cranelift_codegen::ir::ExternalName;
            let name = match &r.target {
                T::ExternalName(ExternalName::User(u)) => &ctx.func.params.user_named_funcs()[*u],
                // a library routine: an absolute address, the same in every
                // Vm of the process, unless the code reaches it relatively
                T::ExternalName(_) if r.kind == Reloc::Abs8 => continue,
                _ => {
                    cap.unshareable = true;
                    return;
                }
            };
            let target = if name.namespace == 1 {
                let decl = module
                    .declarations()
                    .get_data_decl(cranelift_module::DataId::from_u32(name.index));
                match decl
                    .name
                    .as_deref()
                    .and_then(|s| s.strip_prefix("__luna_reloc_"))
                    .and_then(|s| s.parse::<u32>().ok())
                {
                    Some(n) if r.kind == Reloc::Abs8 => Target::Const(n),
                    _ => {
                        cap.unshareable = true;
                        return;
                    }
                }
            } else {
                let f = cranelift_module::FuncId::from_u32(name.index);
                if module.declarations().get_function_decl(f).linkage == Linkage::Import {
                    // a helper: the same address in every Vm, if absolute
                    if r.kind != Reloc::Abs8 {
                        cap.unshareable = true;
                        return;
                    }
                    continue;
                }
                Target::Local(name.index)
            };
            rels.push(Rel {
                at: r.offset,
                kind: r.kind,
                target,
                addend: r.addend,
            });
        }
        cap.funcs.push((id.as_u32(), cc.code_buffer().len(), rels));
    });
}

/// Compiles `proto` with the method JIT, capturing its code for sharing
/// when `capture`.
pub(crate) fn compile(
    proto: Gc<Proto>,
    pre53: bool,
    float_only: bool,
    capture: bool,
) -> Option<(JitHandle, Option<Box<dyn FnOnce(u64) -> ChunkImage>>)> {
    if capture {
        CAPTURE.with(|c| *c.borrow_mut() = Some(Capture::default()));
    }
    let handle = try_compile_int_chunk(proto, pre53, float_only);
    let cap = CAPTURE.with(|c| c.borrow_mut().take());
    let handle = handle?;
    let cap = cap.filter(|c| !c.unshareable);
    let image = cap.and_then(|cap| {
        // the chunk's entry is the function defined last (its checked
        // entry, or the body when it needs none)
        let entry = cap.funcs.last()?.0;
        let funcs: Vec<Func> = cap
            .funcs
            .iter()
            .map(|(id, len, rels)| {
                let p = handle
                    ._module
                    .get_finalized_function(cranelift_module::FuncId::from_u32(*id));
                // SAFETY: `p..p + len` is the function the handle's module
                // finalized, mapped while the handle lives
                let bytes = unsafe { std::slice::from_raw_parts(p, *len) };
                Some(Func {
                    id: *id,
                    bytes: bytes.into(),
                    rels: rels.clone().into(),
                })
            })
            .collect::<Option<_>>()?;
        // each string by its index among the function's constants
        let k_of = |n: u32| {
            let live = *cap.strs.get(n as usize)?;
            proto
                .consts
                .iter()
                .position(
                    |c| matches!(c, luna_core::runtime::Value::Str(s) if s.as_ptr() as i64 == live),
                )
                .map(|k| k as u32)
        };
        let funcs: Vec<Func> = funcs
            .into_iter()
            .map(|mut f| {
                let mut rels = f.rels.to_vec();
                for r in &mut rels {
                    if let Target::Const(n) = r.target {
                        r.target = Target::Const(k_of(n)?);
                    }
                }
                f.rels = rels.into();
                Some(f)
            })
            .collect::<Option<_>>()?;
        let meta = ChunkMeta {
            num_args: handle.num_args,
            returns_one: handle.returns_one,
            arg_float_mask: handle.arg_float_mask,
            arg_table_mask: handle.arg_table_mask,
            ret_is_float: handle.ret_is_float,
            ret_is_table: handle.ret_is_table,
        };
        let content = Content::of(&proto);
        let f: Box<dyn FnOnce(u64) -> ChunkImage> = Box::new(move |id| {
            let size = std::mem::size_of::<ChunkImage>()
                + content.bytes.len()
                + funcs
                    .iter()
                    .map(|f| f.bytes.len() + 32 * f.rels.len())
                    .sum::<usize>();
            ChunkImage {
                id,
                content,
                funcs: funcs.into(),
                entry,
                meta,
                size,
            }
        });
        Some(f)
    });
    Some((handle, image))
}

fn key(cs: &CraneliftJitStorage, pre53: bool, float_only: bool, content: u64) -> Option<ChunkKey> {
    Some(ChunkKey {
        version: cs.version? as u8,
        pre53,
        float_only,
        content,
    })
}

/// Hands the function `make` builds the image of, just compiled for
/// `proto`, to the engine of the Vm `cs` belongs to.
pub(crate) fn publish(
    cs: &mut CraneliftJitStorage,
    proto: &Proto,
    pre53: bool,
    float_only: bool,
    make: Box<dyn FnOnce(u64) -> ChunkImage>,
) {
    let Some(engine) = cs.engine.clone() else {
        return;
    };
    let img = make(engine.next_id());
    let Some(key) = key(cs, pre53, float_only, img.content.hash) else {
        return;
    };
    debug_assert!(img.content.matches(proto));
    engine.cache().insert_chunk(key, Arc::new(img));
}

/// The code of a function of `proto`'s content another Vm of the engine
/// compiled, installed in this Vm's code memory.
pub(crate) fn adopt(
    cs: &mut CraneliftJitStorage,
    proto: &Proto,
    pre53: bool,
    float_only: bool,
) -> Option<(*const u8, ChunkMeta)> {
    let engine = cs.engine.clone()?;
    let content = Content::of(proto);
    let key = key(cs, pre53, float_only, content.hash)?;
    let img = {
        let cache = engine.cache();
        cache
            .chunks
            .get(&key)?
            .iter()
            .find(|i| i.content == content)?
            .clone()
    };
    let r = install(&img, proto, &mut cs.baseline_code)?;
    cs.chunks_adopted += 1;
    Some(r)
}

/// `img`'s code for `proto` of this Vm, placed in `arena`: its functions
/// laid out one after another, calls between them pointed at their new
/// places, and the string constants this Vm's. `None` when `proto` is not
/// of `img`'s content or the code cannot be placed.
fn install(
    img: &Arc<ChunkImage>,
    proto: &Proto,
    arena: &mut super::trace::CodeArena,
) -> Option<(*const u8, ChunkMeta)> {
    let mut at = Vec::with_capacity(img.funcs.len());
    let mut code: Vec<u8> = Vec::new();
    for f in img.funcs.iter() {
        code.resize(code.len().next_multiple_of(16), 0);
        at.push(code.len());
        code.extend_from_slice(&f.bytes);
    }
    for (k, f) in img.funcs.iter().enumerate() {
        for r in f.rels.iter() {
            let p = at[k] + r.at as usize;
            match r.target {
                Target::Const(c) => match proto.consts.get(c as usize)? {
                    luna_core::runtime::Value::Str(s) => {
                        code[p..p + 8].copy_from_slice(&(s.as_ptr() as i64).to_le_bytes());
                    }
                    _ => return None,
                },
                Target::Local(id) => {
                    let t = img.funcs.iter().position(|g| g.id == id)?;
                    let d = at[t] as i64 + r.addend - p as i64;
                    match r.kind {
                        Reloc::X86CallPCRel4 | Reloc::X86PCRel4 => {
                            code[p..p + 4].copy_from_slice(&i32::try_from(d).ok()?.to_le_bytes());
                        }
                        Reloc::Arm64Call => {
                            let w = u32::from_le_bytes(code[p..p + 4].try_into().ok()?);
                            let imm = ((d >> 2) as u32) & 0x03ff_ffff;
                            code[p..p + 4]
                                .copy_from_slice(&((w & !0x03ff_ffff) | imm).to_le_bytes());
                        }
                        _ => return None,
                    }
                }
            }
        }
    }
    let base = arena.place(&code).ok()?;
    let e = img.funcs.iter().position(|f| f.id == img.entry)?;
    Some((base.wrapping_add(at[e]), img.meta))
}
