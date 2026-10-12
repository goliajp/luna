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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
pub(crate) struct Func {
    id: u32,
    bytes: Box<[u8]>,
    rels: Box<[Rel]>,
}

/// A self-recursive chunk's ring (see `chunk_lower::SelfCalls`): the
/// body to lay out `copies` times, each copy's self calls pointing at the
/// next and the last copy's at the stub, whose call goes to the first.
#[derive(Clone, Copy, Debug)]
struct Ring {
    body: u32,
    stub: u32,
    copies: u32,
}

/// Ids of a ring's copies in the laid-out function list.
const RING_ID: u32 = 1 << 30;

/// The functions of a compiled chunk as they are laid out in code memory,
/// with the entry's id: what a Vm runs, and what it hands to the engine
/// to share.
#[derive(Clone)]
pub(crate) struct Layout {
    funcs: Box<[Func]>,
    entry: u32,
    /// the string constant each `Target::Const` relocation holds
    strs: Box<[i64]>,
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
    ring: Option<Ring>,
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

/// Starts capturing the code of the chunk about to be compiled.
pub(crate) fn begin_capture() {
    CAPTURE.with(|c| *c.borrow_mut() = Some(Capture::default()));
}

/// The functions `body` and `stub` of the chunk being compiled form a
/// ring of `copies` (see [`Ring`]).
pub(crate) fn note_ring(body: FuncId, stub: FuncId, copies: u32) {
    CAPTURE.with(|c| {
        if let Some(cap) = c.borrow_mut().as_mut() {
            cap.ring = Some(Ring {
                body: body.as_u32(),
                stub: stub.as_u32(),
                copies,
            });
        }
    });
}

/// Drops the capture `begin_capture` started: the chunk was not compiled.
pub(crate) fn drop_capture() {
    CAPTURE.with(|c| *c.borrow_mut() = None);
}

/// Ends the capture `begin_capture` started, once `module` has finalized
/// the chunk whose entry is `entry_id`: the chunk's functions as laid out
/// for running and sharing (`None` when the code cannot be moved), and
/// whether the chunk is a ring. A ring's copies are made here; a chunk
/// without one is laid out as compiled.
pub(crate) fn end_capture(module: &JITModule, entry_id: FuncId) -> (Option<Layout>, bool) {
    let Some(cap) = CAPTURE.with(|c| c.borrow_mut().take()) else {
        return (None, false);
    };
    let is_ring = cap.ring.is_some();
    (layout_of(module, cap, entry_id), is_ring)
}

fn layout_of(module: &JITModule, cap: Capture, entry_id: FuncId) -> Option<Layout> {
    if cap.unshareable {
        return None;
    }
    let mut funcs: Vec<Func> = cap
        .funcs
        .iter()
        .map(|(id, len, rels)| {
            let p = module.get_finalized_function(FuncId::from_u32(*id));
            // SAFETY: `p..p + len` is the function the module finalized,
            // mapped while the module lives
            let bytes = unsafe { std::slice::from_raw_parts(p, *len) };
            Func {
                id: *id,
                bytes: bytes.into(),
                rels: rels.clone().into(),
            }
        })
        .collect();
    if let Some(ring) = cap.ring {
        let body = funcs.iter().position(|f| f.id == ring.body)?;
        let body = funcs.remove(body);
        for f in &mut funcs {
            if f.id == ring.stub {
                retarget(f, ring.body, RING_ID);
            }
        }
        for i in 0..ring.copies {
            let mut copy = body.clone();
            copy.id = RING_ID + i;
            let next = if i + 1 == ring.copies {
                ring.stub
            } else {
                RING_ID + i + 1
            };
            retarget(&mut copy, ring.body, next);
            funcs.push(copy);
        }
    }
    Some(Layout {
        funcs: funcs.into(),
        entry: entry_id.as_u32(),
        strs: cap.strs.into(),
    })
}

/// Points `f`'s calls of the function `from` at `to`.
fn retarget(f: &mut Func, from: u32, to: u32) {
    for r in f.rels.iter_mut() {
        if r.target == Target::Local(from) {
            r.target = Target::Local(to);
        }
    }
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

/// Compiles `proto` with the method JIT; with `share`, also the image
/// another Vm of the engine can adopt.
pub(crate) fn compile(
    proto: Gc<Proto>,
    pre53: bool,
    float_only: bool,
    share: bool,
) -> Option<(JitHandle, Option<Box<dyn FnOnce(u64) -> ChunkImage>>)> {
    let handle = try_compile_int_chunk(proto, pre53, float_only)?;
    let image = share
        .then(|| handle.layout.clone())
        .flatten()
        .and_then(|layout| {
            // each string by its index among the function's constants
            let k_of = |n: u32| {
                let live = *layout.strs.get(n as usize)?;
                proto
                .consts
                .iter()
                .position(
                    |c| matches!(c, luna_core::runtime::Value::Str(s) if s.as_ptr() as i64 == live),
                )
                .map(|k| k as u32)
            };
            let funcs: Vec<Func> = layout
                .funcs
                .iter()
                .map(|f| {
                    let mut f = f.clone();
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
            let entry = layout.entry;
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
    let base = place_funcs(&img.funcs, img.entry, proto, &mut cs.baseline_code)?;
    cs.chunks_adopted += 1;
    Some((base, img.meta))
}

/// A chunk's own layout placed in `arena` for the Vm that compiled it:
/// the address of its entry.
pub(crate) fn place_layout(
    layout: &Layout,
    proto: &Proto,
    arena: &mut super::trace::CodeArena,
) -> Option<*const u8> {
    place_funcs(&layout.funcs, layout.entry, proto, arena)
}

/// `img`'s code for `proto` of this Vm, placed in `arena`: its functions
/// laid out one after another, calls between them pointed at their new
/// places, and the string constants this Vm's. `None` when `proto` is not
/// of `img`'s content or the code cannot be placed.
fn place_funcs(
    funcs: &[Func],
    entry: u32,
    proto: &Proto,
    arena: &mut super::trace::CodeArena,
) -> Option<*const u8> {
    let mut at = Vec::with_capacity(funcs.len());
    let mut code: Vec<u8> = Vec::new();
    for f in funcs.iter() {
        code.resize(code.len().next_multiple_of(16), 0);
        at.push(code.len());
        code.extend_from_slice(&f.bytes);
    }
    for (k, f) in funcs.iter().enumerate() {
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
                    let t = funcs.iter().position(|g| g.id == id)?;
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
    let e = funcs.iter().position(|f| f.id == entry)?;
    Some(base.wrapping_add(at[e]))
}
