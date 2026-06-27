//! AST → bytecode compiler. Register model follows PUC lparser/lcode:
//! locals pin the low registers, temporaries grow from `freereg`, constants
//! are deduplicated, forward jumps are patch lists (plain Vecs instead of
//! PUC's in-code jump chains). Function nesting is a stack of `Level`s;
//! upvalue resolution walks it (PUC singlevaraux).
//!
//! Slice 3 state: calls, closures, upvalues, varargs (5.5 table semantics),
//! generic `for`, multret, tail calls. Still pending (slice 5): goto/labels,
//! `<close>`, `global` declarations.

use std::collections::HashMap;

use crate::frontend::ast::{
    self, AttribName, BinOp, Block, Chunk, Expr, ExprId, FuncBody, Stat, StatId, TableField, UnOp,
    block_uses_vararg,
};
use crate::frontend::error::SyntaxError;
use crate::numeric::Num;
use crate::runtime::heap::{GcHeader, ObjTag};
use crate::runtime::{Gc, Heap, LuaStr, Proto, UpvalDesc, Value};
use crate::version::LuaVersion;
use crate::vm::isa::{Inst, MAX_BX, MAX_SJ, Op};

/// Lower an [`Chunk`] into a [`Proto`] (luna bytecode) for the
/// given dialect. The interned source name is attached to the proto for
/// error messages and `debug.getinfo`.
pub fn compile_chunk(
    ast: &Chunk,
    version: LuaVersion,
    source_name: &[u8],
    heap: &mut Heap,
) -> Result<Gc<Proto>, SyntaxError> {
    let source = heap.intern(source_name);
    let mut c = Compiler {
        ast,
        heap,
        version,
        source,
        levels: Vec::new(),
        last_line: 0,
        force_line: None,
        str_cache: HashMap::new(),
    };
    let mut main = Level::new(0, true, 0);
    main.upvals.push(UpvalDesc {
        in_stack: false,
        index: 0,
        name: "_ENV".into(),
        read_only: false,
    });
    c.levels.push(main);
    c.enter_block(false);
    c.stat_block(&ast.block)?;
    c.leave_block()?;
    // the implicit final return belongs to the chunk's last line (PUC), so a
    // line hook / activelines see it there rather than on the last statement
    c.last_line = ast.end_line;
    c.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
    let lvl = c.levels.pop().expect("main level");
    Ok(c.heap.adopt_proto(lvl.into_proto(source, 0, 0)))
}

const MAX_REGS: u32 = 254;
/// PUC `LUAI_MAXUPVAL`: the per-function upvalue cap. 5.1 set this to 60;
/// 5.2+ raised it to 255 because the bytecode encoding gained the room.
/// Errors raised at this boundary use the standard "too many upvalues
/// (limit is …) in function at line …" format (errors.lua 5.4 :765 /
/// :775; 5.1 :238 walks 70 inner closures and expects to wall at line 3).
fn max_upvals(version: LuaVersion) -> u32 {
    if version <= LuaVersion::Lua51 {
        60
    } else {
        255
    }
}
/// PUC `MAXVARS`: the per-function active-locals cap.
const MAX_LOCALS: u32 = 200;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ConstKey {
    Int(i64),
    Float(u64),
    Str(*mut LuaStr),
}

/// Per-target plan for `assign_stat`'s two-phase store (snapshot first, then
/// emit RHS, then stores) so a later store cannot reorder around an earlier
/// one's table/key reads (PUC manual §3.3.3).
enum LhsPlan {
    Name(ExprId),
    Indexed { obj: u32, key: SetKey },
}

enum SetKey {
    /// String constant index for OP_SetField (k ≤ 0xFF).
    Field(u32),
    /// Small integer literal for OP_SetI (0..=255).
    Int(u32),
    /// Any other key, pinned in a register for OP_SetTable.
    Reg(u32),
}

struct LocalVar {
    name: Box<str>,
    reg: u32,
    read_only: bool,
    captured: bool,
    /// a named vararg (`...t`) bound as a *virtual* view: `t[k]`/`t.n` reads
    /// compile to OP_VARGIDX (no table). Set only when the pre-scan proved the
    /// vararg is never written / never escapes / is not `_ENV`.
    vararg_virtual: bool,
    /// pc at which the variable became visible (for debug LocVar records)
    start_pc: u32,
}

/// One entry in the function's ordered active-variable sequence used for
/// goto/label scope checks. Mirrors PUC's `actvar` list: every local AND every
/// `global` declaration appends one, so a goto that jumps over either lands
/// "into its scope". `reg` is `Some` only for real locals (used to compute the
/// CLOSE register floor); `name` is `None` for a `global *` marker (reported as
/// `'*'` in scope errors).
struct AVar {
    name: Option<Box<str>>,
    reg: Option<u32>,
}

struct BlockCx {
    first_local: usize,
    /// index into `Level::avars` at block entry (goto-scope truncation point)
    first_avar: usize,
    reg_floor: u32,
    is_loop: bool,
    breaks: Vec<usize>,
    /// visible labels defined in this block
    labels: Vec<LabelDef>,
    /// forward gotos not yet matched to a label
    gotos: Vec<GotoRef>,
    /// explicit `global` declarations in this block (name, read_only)
    gdecls: Vec<(Box<str>, bool)>,
    /// `global [attrib] *` in this block: Some(read_only)
    collective: Option<bool>,
    /// any to-be-closed local declared in this block
    has_tbc: bool,
    /// this block is in the scope of a to-be-closed variable (an explicit
    /// <close> local, or a generic-for's implicit closing value): suppresses
    /// tail calls so the function returns to run __close. Tracked separately
    /// from `has_tbc` so it doesn't perturb CLOSE-instruction emission.
    tbc_scope: bool,
}

struct LabelDef {
    name: Box<str>,
    pc: usize,
    /// source line of the label (for "already defined on line N")
    line: u32,
    /// locals active at the label (trailing labels use the block floor)
    nactive: usize,
}

struct GotoRef {
    name: Box<str>,
    jmp_pc: usize,
    line: u32,
    nactive: usize,
}

enum VarKind {
    Local(u32),
    Upval(u32),
    /// global access; read_only from 5.5 declarations
    Global {
        read_only: bool,
    },
}

/// Where an expression's value currently lives.
enum Exp {
    Nil,
    True,
    False,
    Int(i64),
    Float(f64),
    Const(u32),
    /// value sits in a register (local or materialized temp)
    Reg(u32),
    /// instruction at index has an unassigned A (destination pending)
    Reloc(usize),
    /// comparison not yet materialized
    Cmp {
        op: Op,
        l: u32,
        r: u32,
    },
    /// open multi-result producer (CALL/VARARG) at `pc`, results from `base`
    Open {
        pc: usize,
        base: u32,
    },
}

struct Level {
    code: Vec<Inst>,
    lines: Vec<u32>,
    consts: Vec<Value>,
    const_map: HashMap<ConstKey, u32>,
    locals: Vec<LocalVar>,
    /// ordered active-variable sequence (locals + global decls) for goto scope
    avars: Vec<AVar>,
    blocks: Vec<BlockCx>,
    freereg: u32,
    max_stack: u32,
    upvals: Vec<UpvalDesc>,
    protos: Vec<Gc<Proto>>,
    /// completed local-variable debug records (flushed on scope exit)
    locvars: Vec<crate::runtime::LocVar>,
    num_params: u8,
    is_vararg: bool,
    /// Mirrors PUC `(vararg table)` locvar emission: true only for an explicit
    /// anonymous `(...)` parlist (NOT a main chunk's implicit vararg).
    has_vararg_table_pseudo: bool,
    /// PUC 5.1 LUAI_COMPAT_VARARG: the hidden `arg` table local was reserved.
    /// The runtime populates it on entry; see Proto::has_compat_vararg_arg.
    has_compat_vararg_arg: bool,
    #[allow(dead_code)]
    line_defined: u32,
    /// PUC `fs->lasttarget` equivalent: the highest pc that is the destination
    /// of any patched jump (forward jump landing here, backward jump-back to a
    /// previously saved pc, ForLoop / TForLoop back-edge, or a defined label).
    /// `None` is PUC's sentinel `-1` — no target has been recorded yet.
    ///
    /// Read by peephole passes (see `prev_emit_is_safe_peephole_site`) that
    /// want to know whether the just-emitted instruction at pc `here() - 1`
    /// can be modified in place: it is safe only when that pc is NOT itself a
    /// jump destination, i.e. `last_target < here() - 1` or `last_target ==
    /// None`. The A4''' Reloc-landing peephole (deferred follow-up) is the
    /// first planned consumer; this field is currently exposed but never read
    /// for code generation, so its addition is behaviour-neutral.
    ///
    /// Maintained monotonically (only advances upward) by `mark_target(pc)`,
    /// called from every code path that turns some `pc` into a jump landing
    /// point.
    last_target: Option<usize>,
}

impl Level {
    fn new(num_params: u8, is_vararg: bool, line_defined: u32) -> Level {
        Level {
            code: Vec::new(),
            lines: Vec::new(),
            consts: Vec::new(),
            const_map: HashMap::new(),
            locals: Vec::new(),
            avars: Vec::new(),
            blocks: Vec::new(),
            freereg: num_params as u32,
            max_stack: (num_params as u32).max(2),
            upvals: Vec::new(),
            protos: Vec::new(),
            locvars: Vec::new(),
            num_params,
            is_vararg,
            has_vararg_table_pseudo: false,
            has_compat_vararg_arg: false,
            line_defined,
            last_target: None,
        }
    }

    fn into_proto(self, source: Gc<LuaStr>, line_defined: u32, last_line_defined: u32) -> Proto {
        let env_upval_idx = self
            .upvals
            .iter()
            .take(u8::MAX as usize)
            .position(|u| &*u.name == "_ENV")
            .map_or(u8::MAX, |i| i as u8);
        Proto {
            hdr: GcHeader::new(ObjTag::Proto),
            code: self.code.into_boxed_slice(),
            consts: self.consts.into_boxed_slice(),
            protos: self.protos.into_boxed_slice(),
            upvals: self.upvals.into_boxed_slice(),
            num_params: self.num_params,
            is_vararg: self.is_vararg,
            has_vararg_table_pseudo: self.has_vararg_table_pseudo,
            has_compat_vararg_arg: self.has_compat_vararg_arg,
            max_stack: self.max_stack as u8,
            lines: self.lines.into_boxed_slice(),
            source,
            line_defined,
            last_line_defined,
            locvars: self.locvars.into_boxed_slice(),
            cache: std::cell::Cell::new(None),
            jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
            env_upval_idx,
            trace_hot_count: std::cell::Cell::new(0),
            call_hot_count: std::cell::Cell::new(0),
            trace_discard_count: std::cell::Cell::new(0),
            trace_gave_up: std::cell::Cell::new(false),
            traces: std::cell::RefCell::new(Vec::new()),
        }
    }
}

struct Compiler<'a> {
    ast: &'a Chunk,
    heap: &'a mut Heap,
    version: LuaVersion,
    source: Gc<LuaStr>,
    levels: Vec<Level>,
    last_line: u32,
    /// When `Some(line)`, every `emit` ignores `last_line` and attributes the
    /// new instruction to `line` instead. PUC infix discharges its left
    /// operand *after* the operator token (so the GET emitted for `b[1]` in
    /// `b[1] + …` lands on the operator's line, not the line of `b[1]`);
    /// luna parses the lhs ahead of time and so cannot defer the emit, but
    /// pinning the line here gives the same trace.
    force_line: Option<u32>,
    /// Compile-time literal interning (PUC's `luaX_newstring` cache): identical
    /// string literals anywhere in the chunk — short *or* long — share one
    /// object, so e.g. `string.format("%p", ...)` reports equal addresses for
    /// equal constants. The runtime interner only dedups short strings.
    str_cache: HashMap<Box<[u8]>, Gc<LuaStr>>,
}

impl<'a> Compiler<'a> {
    // ---- infrastructure ----

    fn l(&mut self) -> &mut Level {
        self.levels.last_mut().expect("no level")
    }

    fn lr(&self) -> &Level {
        self.levels.last().expect("no level")
    }

    fn err(&self, line: u32, msg: impl Into<String>) -> SyntaxError {
        SyntaxError {
            line,
            msg: msg.into().into_bytes(),
        }
    }

    fn emit(&mut self, i: Inst) -> usize {
        let line = self.force_line.unwrap_or(self.last_line);
        let l = self.l();
        l.code.push(i);
        l.lines.push(line);
        l.code.len() - 1
    }

    fn emit_jump(&mut self) -> usize {
        self.emit(Inst::isj(Op::Jmp, 0))
    }

    fn here(&self) -> usize {
        self.lr().code.len()
    }

    /// PUC ≤5.3's OP_JMP used an 18-bit sBx field, capping reachable code
    /// at ~131k instructions. luna's bytecode widens that to 24-bit sJ
    /// (~16M), but the 5.3 test gate (constructs.lua :308) literally
    /// generates a 262144-instruction `while` to probe the limit. Match
    /// the older dialects' tighter cap so the error fires when the suite
    /// expects it. 5.4+ keeps the wider luna bound.
    fn jump_cap(&self) -> u64 {
        if self.version <= LuaVersion::Lua53 {
            (1u64 << 17) - 1
        } else {
            MAX_SJ as u64
        }
    }

    /// Patch a pending forward jump emitted earlier at `pc` so that it lands
    /// at the current `here()` position, and mark `here()` as a jump target
    /// (see `Level::last_target`). Mirrors PUC `luaK_patchtohere`.
    ///
    /// This is the canonical "this jump lands at the next instruction we are
    /// about to emit" hook; every patch-pending-forward-jump call site routes
    /// through it so that the jump-target tracker stays consistent. There is
    /// no separate `patch_jump` variant that elides the mark — patching a
    /// forward jump to a position that is not yet a target is meaningless.
    fn patch_to_here(&mut self, pc: usize) -> Result<(), SyntaxError> {
        let target = self.here();
        let off = target as i64 - pc as i64 - 1;
        if off.unsigned_abs() > self.jump_cap() {
            return Err(self.err(self.last_line, "control structure too long"));
        }
        self.l().code[pc].set_sj(off as i32);
        self.mark_target(target);
        Ok(())
    }

    fn jump_back(&mut self, target: usize) -> Result<(), SyntaxError> {
        let off = target as i64 - self.here() as i64 - 1;
        if off.unsigned_abs() > self.jump_cap() {
            return Err(self.err(self.last_line, "control structure too long"));
        }
        self.emit(Inst::isj(Op::Jmp, off as i32));
        // The back-edge lands at `target`, which was captured upstream
        // (typically `let top = self.here()` before a loop header). Mark it
        // so a future peephole pass sees that pc as occupied.
        self.mark_target(target);
        Ok(())
    }

    /// Record that `pc` is now a jump destination. Monotonic; advances
    /// `last_target` only when `pc` exceeds the recorded maximum. Mirrors the
    /// effect of PUC `luaK_getlabel` (which sets `fs->lasttarget = fs->pc`).
    fn mark_target(&mut self, pc: usize) {
        let l = self.l();
        match l.last_target {
            None => l.last_target = Some(pc),
            Some(t) if pc > t => l.last_target = Some(pc),
            _ => {}
        }
    }

    /// PUC-equivalent query for "is the instruction at `here() - 1` safe to
    /// peephole-retarget?". Returns `false` either when no instruction has
    /// been emitted yet (vacuous), or when the just-emitted pc is itself a
    /// recorded jump destination.
    ///
    /// Currently unused at codegen sites — exposed so the deferred A4'''
    /// Reloc-landing attack (see `.dev/rfcs/v2.0-pi-phase11-a4-prime-rfc.md`
    /// §4) can wire its gate without touching the tracker subsystem again.
    #[allow(dead_code)]
    fn prev_emit_is_safe_peephole_site(&self) -> bool {
        let here = self.here();
        if here == 0 {
            return false;
        }
        match self.lr().last_target {
            None => true,
            Some(t) => t < here - 1,
        }
    }

    /// PUC `errorlimit`: render the "too many … (limit is …) in <where>"
    /// message a per-function-cap check raises. `where` is "main function"
    /// for the chunk's top-level proto and "function at line N" for every
    /// nested function — N is the proto's `line_defined`. errors.lua :766/:775
    /// check the line number substring.
    fn limit_err(&self, what: &str, limit: u32) -> SyntaxError {
        self.limit_err_at(self.levels.len() - 1, what, limit)
    }

    /// PUC's "too many X" errors attribute to the *level* whose budget got
    /// exhausted (`L->ci`'s `func`'s `linedefined`), which is not necessarily
    /// the currently-being-compiled function when an upvalue cascade walks up
    /// the lexical chain. 5.1 errors.lua :238's 70 nested closures fill foo1's
    /// upval cap when foo70 first references a70, and PUC reports
    /// "...function at line 3" — foo1's start.
    fn limit_err_at(&self, li: usize, what: &str, limit: u32) -> SyntaxError {
        let line_defined = self.levels[li].line_defined;
        let where_ = if line_defined == 0 {
            "main function".to_string()
        } else {
            format!("function at line {line_defined}")
        };
        self.err(
            self.last_line,
            format!("too many {what} (limit is {limit}) in {where_}"),
        )
    }

    fn reserve(&mut self, n: u32) -> Result<u32, SyntaxError> {
        let line = self.last_line;
        let l = self.l();
        let base = l.freereg;
        l.freereg += n;
        if l.freereg > MAX_REGS {
            return Err(self.err(line, "function or expression needs too many registers"));
        }
        if l.freereg > l.max_stack {
            l.max_stack = l.freereg;
        }
        Ok(base)
    }

    fn set_freereg(&mut self, r: u32) {
        let l = self.l();
        l.freereg = r;
        if r > l.max_stack {
            l.max_stack = r;
        }
    }

    fn const_idx(&mut self, key: ConstKey, v: Value) -> u32 {
        let l = self.l();
        if let Some(&i) = l.const_map.get(&key) {
            return i;
        }
        let i = l.consts.len() as u32;
        l.consts.push(v);
        l.const_map.insert(key, i);
        i
    }

    fn str_const(&mut self, bytes: &[u8]) -> u32 {
        // intern the literal once per chunk so identical constants share an
        // object (heap.intern alone only dedups short strings)
        let s = match self.str_cache.get(bytes) {
            Some(s) => *s,
            None => {
                let s = self.heap.intern(bytes);
                self.str_cache.insert(bytes.into(), s);
                s
            }
        };
        self.const_idx(ConstKey::Str(s.as_ptr()), Value::Str(s))
    }

    fn load_const(&mut self, reg: u32, c: u32) {
        if c <= MAX_BX {
            self.emit(Inst::iabx(Op::LoadK, reg, c));
        } else {
            self.emit(Inst::iabx(Op::LoadKx, reg, 0));
            self.emit(Inst::iax(Op::ExtraArg, c));
        }
    }

    /// Rewrite the wanted-results field (C) of an open CALL/VARARG.
    fn patch_wanted(&mut self, pc: usize, wanted_plus1: u32) {
        let i = self.l().code[pc];
        self.l().code[pc] = Inst((i.0 & 0x00FF_FFFF) | (wanted_plus1 << 24));
    }

    /// Rewrite the destination (A) field of a pending instruction.
    fn patch_dest(&mut self, pc: usize, reg: u32) {
        let i = self.l().code[pc];
        self.l().code[pc] = Inst(i.0 & !(0xFF << 7) | (reg << 7));
    }

    // ---- scopes & names ----

    fn enter_block(&mut self, is_loop: bool) {
        let floor = self.lr().freereg;
        let first = self.lr().locals.len();
        let first_avar = self.lr().avars.len();
        self.l().blocks.push(BlockCx {
            first_local: first,
            first_avar,
            reg_floor: floor,
            is_loop,
            breaks: Vec::new(),
            labels: Vec::new(),
            gotos: Vec::new(),
            gdecls: Vec::new(),
            collective: None,
            has_tbc: false,
            tbc_scope: false,
        });
    }

    fn leave_block(&mut self) -> Result<(), SyntaxError> {
        let b = self.l().blocks.pop().expect("block underflow");
        let captured = self.lr().locals[b.first_local..].iter().any(|l| l.captured);
        // record debug LocVar entries for the locals leaving scope here
        let end_pc = self.lr().code.len() as u32;
        let leaving: Vec<crate::runtime::LocVar> = self.lr().locals[b.first_local..]
            .iter()
            .map(|l| crate::runtime::LocVar {
                name: l.name.clone(),
                reg: l.reg,
                start_pc: l.start_pc,
                end_pc,
            })
            .collect();
        self.l().locvars.extend(leaving);
        self.l().locals.truncate(b.first_local);
        self.l().avars.truncate(b.first_avar);
        self.set_freereg(b.reg_floor);
        if captured || b.has_tbc {
            self.emit(Inst::iabc(Op::Close, b.reg_floor, 0, 0, false));
        }
        for pc in b.breaks {
            self.patch_to_here(pc)?;
        }
        // propagate unmatched gotos to the enclosing block (the label may
        // appear after this block); a goto leaving a block with captured or
        // to-be-closed locals must run CLOSE first — route it through a
        // trampoline
        if !b.gotos.is_empty() {
            let mut gotos = b.gotos;
            if captured || b.has_tbc {
                // each distinct label name gets its own close trampoline
                let mut names: Vec<Box<str>> = Vec::new();
                for g in &gotos {
                    if !names.contains(&g.name) {
                        names.push(g.name.clone());
                    }
                }
                let skip = self.emit_jump();
                let mut routed = Vec::with_capacity(names.len());
                for name in names {
                    let tramp = self.here();
                    self.emit(Inst::iabc(Op::Close, b.reg_floor, 0, 0, false));
                    let new_jmp = self.emit_jump();
                    for g in gotos.iter().filter(|g| g.name == name) {
                        let off = tramp as i64 - g.jmp_pc as i64 - 1;
                        if off.unsigned_abs() > MAX_SJ as u64 {
                            return Err(self.err(g.line, "control structure too long"));
                        }
                        self.l().code[g.jmp_pc].set_sj(off as i32);
                    }
                    // every goto in this batch lands at `tramp` (start of the
                    // per-name close trampoline)
                    self.mark_target(tramp);
                    let line = gotos
                        .iter()
                        .find(|g| g.name == name)
                        .map(|g| g.line)
                        .expect("goto exists");
                    routed.push(GotoRef {
                        name,
                        jmp_pc: new_jmp,
                        line,
                        nactive: b.first_avar,
                    });
                }
                self.patch_to_here(skip)?;
                gotos = routed;
            }
            let cap = self.lr().avars.len();
            // PUC `movegotosout` matches each propagated goto against the
            // *immediate* enclosing block's already-defined labels (its
            // `findlabel`) — a forward `goto name` resolved entirely after the
            // block closed would otherwise sit unresolved forever once luna
            // stopped searching ancestor blocks at goto time. math.lua 5.4
            // :995 (`::doagain::` defined ahead, `goto doagain` issued from
            // inside a nested `if`) is the prototypical case. Only the
            // immediate parent is consulted; deeper ancestors are reached
            // later as the parent itself leaves. When the goto jumps over a
            // captured local declared *between* the parent's label and the
            // goto (PUC `luaK_patchclose`), the resolution routes through a
            // CLOSE-and-jump trampoline so those upvalues are properly closed
            // — goto.lua 5.4 :203's foo() backward `goto l1` exercises this.
            let mut unresolved = Vec::with_capacity(gotos.len());
            for g in gotos {
                let target = self.lr().blocks.last().and_then(|p| {
                    p.labels
                        .iter()
                        .rev()
                        .find(|l| l.name == g.name)
                        .map(|l| (l.pc, l.nactive))
                });
                match target {
                    Some((pc, label_nactive)) => {
                        let needs_close = g.nactive > label_nactive
                            && self.reg_floor_from_avar(label_nactive).is_some();
                        let dest = if needs_close {
                            let skip = self.emit_jump();
                            let tramp = self.here();
                            if let Some(floor) = self.reg_floor_from_avar(label_nactive) {
                                self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
                            }
                            let to_label = pc as i64 - self.here() as i64 - 1;
                            if to_label.unsigned_abs() > MAX_SJ as u64 {
                                return Err(self.err(g.line, "control structure too long"));
                            }
                            self.emit(Inst::isj(Op::Jmp, to_label as i32));
                            self.patch_to_here(skip)?;
                            tramp as i64
                        } else {
                            pc as i64
                        };
                        let off = dest - g.jmp_pc as i64 - 1;
                        if off.unsigned_abs() > MAX_SJ as u64 {
                            return Err(self.err(g.line, "control structure too long"));
                        }
                        self.l().code[g.jmp_pc].set_sj(off as i32);
                        // dest is either a backward label.pc or a trampoline
                        // pc — both are jump destinations.
                        self.mark_target(dest as usize);
                    }
                    None => unresolved.push(g),
                }
            }
            match self.l().blocks.last_mut() {
                Some(parent) => {
                    for mut g in unresolved {
                        g.nactive = g.nactive.min(cap);
                        parent.gotos.push(g);
                    }
                }
                None if !unresolved.is_empty() => {
                    let g = &unresolved[0];
                    return Err(self.err(
                        g.line,
                        format!(
                            "no visible label '{}' for <goto> at line {}",
                            g.name, g.line
                        ),
                    ));
                }
                None => {}
            }
        }
        Ok(())
    }

    /// Define a label here; match pending forward gotos.
    fn define_label(&mut self, name: &str, line: u32, trailing: bool) -> Result<(), SyntaxError> {
        let here = self.here();
        // active-var count (locals + global decls) at the label: a trailing
        // label sits at the block base (its locals are already out of scope)
        let nactive = if trailing {
            self.lr().blocks.last().expect("no block").first_avar
        } else {
            self.lr().avars.len()
        };
        // PUC 5.2/5.3 `checkrepeated` scoped the duplicate check to the
        // current block — so an inner block could shadow an outer label of
        // the same name (goto.lua 5.2/5.3 :71's `do goto l3; ::l3:: end`
        // alongside an outer `::l3::`). PUC 5.4 widened `findlabel` to scan
        // every label in the function (`fs->firstlabel..n`), making any
        // same-name redeclaration anywhere in the function an error
        // (goto.lua 5.4 :16 `::l1:: do ::l1:: end`). Pick the right scope
        // based on the dialect.
        let dup = if self.version <= LuaVersion::Lua53 {
            self.lr()
                .blocks
                .last()
                .and_then(|b| b.labels.iter().find(|l| &*l.name == name))
                .map(|l| l.line)
        } else {
            self.lr()
                .blocks
                .iter()
                .flat_map(|b| b.labels.iter())
                .find(|l| &*l.name == name)
                .map(|l| l.line)
        };
        if let Some(prev_line) = dup {
            return Err(self.err(
                line,
                format!("label '{name}' already defined on line {prev_line}"),
            ));
        }
        let b = self.lr().blocks.last().expect("no block");
        let first_avar = b.first_avar;
        // match pending gotos of this block
        let mut pending = std::mem::take(&mut self.l().blocks.last_mut().expect("no block").gotos);
        let mut kept = Vec::new();
        for g in pending.drain(..) {
            if &*g.name == name {
                if nactive > g.nactive {
                    // the goto jumps into the scope of the declaration sitting
                    // at its active-var boundary; a `global *` marker has no
                    // name and is reported as '*'. PUC 5.4 says "…scope of
                    // local 'X'"; 5.5 dropped the kind prefix (since `global`
                    // markers can sit on the same chain). luna versions the
                    // wording so the per-dialect test gates can both match.
                    let lname = match &self.lr().avars[g.nactive].name {
                        Some(n) => n.to_string(),
                        None => "*".to_string(),
                    };
                    let kind = if self.version >= LuaVersion::Lua55 {
                        String::new()
                    } else {
                        "local ".to_string()
                    };
                    return Err(self.err(
                        g.line,
                        format!(
                            "<goto {name}> at line {} jumps into the scope of {kind}'{lname}'",
                            g.line
                        ),
                    ));
                }
                let off = here as i64 - g.jmp_pc as i64 - 1;
                if off.unsigned_abs() > MAX_SJ as u64 {
                    return Err(self.err(g.line, "control structure too long"));
                }
                self.l().code[g.jmp_pc].set_sj(off as i32);
            } else {
                kept.push(g);
            }
        }
        let blk = self.l().blocks.last_mut().expect("no block");
        blk.gotos = kept;
        blk.labels.push(LabelDef {
            name: name.into(),
            pc: here,
            line,
            nactive: nactive.max(first_avar),
        });
        // every defined label is a jump destination: pending gotos just got
        // patched to land at `here`, AND backward gotos resolved by
        // `goto_stat` lookup against this label will jump here too.
        self.mark_target(here);
        Ok(())
    }

    /// Compile `goto name`: backward jump if a label is visible, else a
    /// pending forward reference in the current block.
    fn goto_stat(&mut self, name: &str, line: u32) -> Result<(), SyntaxError> {
        self.last_line = line;
        // PUC's `gotostat` scans only the *current* block for an
        // already-defined backward label; unresolved gotos enter the pending
        // list and percolate outward on each `leave_block`, so an inner
        // block's later `::name::` (or the enclosing block's existing one)
        // gets matched at scope exit. Searching all ancestor blocks here would
        // make `do goto l; ::l:: end` lock onto the outer `::l::` before the
        // inner one even gets defined — goto.lua 5.2/5.3 :71 specifically
        // exercises that shadow.
        let found: Option<(usize, usize)> = self
            .lr()
            .blocks
            .last()
            .and_then(|b| b.labels.iter().rev().find(|l| &*l.name == name))
            .map(|l| (l.pc, l.nactive));
        if let Some((pc, nactive)) = found {
            // jumping back discards locals declared after the label
            if let Some(floor) = self.reg_floor_from_avar(nactive) {
                self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
            }
            self.jump_back(pc)?;
            return Ok(());
        }
        let jmp = self.emit_jump();
        let nactive = self.lr().avars.len();
        self.l()
            .blocks
            .last_mut()
            .expect("no block")
            .gotos
            .push(GotoRef {
                name: name.into(),
                jmp_pc: jmp,
                line,
                nactive,
            });
        Ok(())
    }

    /// 5.5 global-declaration resolution: explicit declaration > innermost
    /// collective `global *` > implicit chunk default (void once any
    /// declaration is in scope).
    fn resolve_global_kind(&mut self, name: &str, line: u32) -> Result<VarKind, SyntaxError> {
        let mut innermost_collective: Option<bool> = None;
        let mut any_decl = false;
        for lvl in self.levels.iter().rev() {
            for b in lvl.blocks.iter().rev() {
                if let Some((_, ro)) = b.gdecls.iter().rev().find(|(n, _)| &**n == name) {
                    return Ok(VarKind::Global { read_only: *ro });
                }
                if innermost_collective.is_none()
                    && let Some(ro) = b.collective
                {
                    innermost_collective = Some(ro);
                }
                any_decl |= !b.gdecls.is_empty() || b.collective.is_some();
            }
        }
        if let Some(ro) = innermost_collective {
            return Ok(VarKind::Global { read_only: ro });
        }
        if any_decl {
            return Err(self.err(line, format!("variable '{name}' not declared")));
        }
        Ok(VarKind::Global { read_only: false })
    }

    /// The innermost block needs a CLOSE on its back-edge/exit paths when it
    /// captured locals or declared to-be-closed ones.
    fn block_captured(&self) -> bool {
        let b = self.lr().blocks.last().expect("no block");
        b.has_tbc || self.lr().locals[b.first_local..].iter().any(|l| l.captured)
    }

    fn block_floor(&self) -> u32 {
        self.lr().blocks.last().expect("no block").reg_floor
    }

    fn declare_local(&mut self, name: &str, reg: u32, read_only: bool) -> Result<(), SyntaxError> {
        // PUC `new_localvar` calls `checklimit(fs, …, MAXVARS, "local variables")`
        // before recording the slot — luna counts active avars (skip global
        // markers and any pending vararg pseudo) to model the same cap.
        let active = self.lr().avars.iter().filter(|a| a.reg.is_some()).count() as u32;
        if active >= MAX_LOCALS {
            return Err(self.limit_err("local variables", MAX_LOCALS));
        }
        let start_pc = self.lr().code.len() as u32;
        self.l().locals.push(LocalVar {
            name: name.into(),
            reg,
            read_only,
            captured: false,
            vararg_virtual: false,
            start_pc,
        });
        self.l().avars.push(AVar {
            name: Some(name.into()),
            reg: Some(reg),
        });
        Ok(())
    }

    /// Append a `global` declaration marker to the active-variable sequence so
    /// a goto jumping over it lands "into its scope" (PUC's `new_varkind` +
    /// `nactvar++`). `name` is `None` for a `global *` collective marker.
    fn declare_global_marker(&mut self, name: Option<&str>) {
        self.l().avars.push(AVar {
            name: name.map(|n| n.into()),
            reg: None,
        });
    }

    /// Register floor to CLOSE when discarding locals declared at/after the
    /// given avar index (the first real local in that suffix), if any.
    fn reg_floor_from_avar(&self, avar_idx: usize) -> Option<u32> {
        self.lr().avars[avar_idx..].iter().find_map(|a| a.reg)
    }

    fn resolve_name(&mut self, name: &str) -> Result<VarKind, SyntaxError> {
        let top = self.levels.len() - 1;
        self.resolve_at(top, name)
    }

    fn resolve_at(&mut self, li: usize, name: &str) -> Result<VarKind, SyntaxError> {
        // innermost binding among this level's locals and explicit `global`
        // declarations (PUC's single `actvar` list): an inner `global X`
        // shadows an enclosing local X (and vice-versa). A `global *` marker
        // has no name and never matches here — collective scope is the
        // unbound-name fallback handled by resolve_global_kind.
        if let Some(av) = self.levels[li]
            .avars
            .iter()
            .rev()
            .find(|a| a.name.as_deref() == Some(name))
            && av.reg.is_none()
        {
            return Ok(VarKind::Global { read_only: false });
        }
        if let Some(idx) = self.levels[li]
            .locals
            .iter()
            .rposition(|l| &*l.name == name)
        {
            return Ok(VarKind::Local(self.levels[li].locals[idx].reg));
        }
        if li < self.levels.len() - 1 || li == 0 {
            // upvalue cache applies at every level; main level has _ENV
            if let Some(ui) = self.levels[li].upvals.iter().position(|u| &*u.name == name) {
                return Ok(VarKind::Upval(ui as u32));
            }
        } else if let Some(ui) = self.levels[li].upvals.iter().position(|u| &*u.name == name) {
            return Ok(VarKind::Upval(ui as u32));
        }
        if li == 0 {
            return Ok(VarKind::Global { read_only: false });
        }
        match self.resolve_at(li - 1, name)? {
            VarKind::Global { .. } => Ok(VarKind::Global { read_only: false }),
            VarKind::Local(reg) => {
                let mut read_only = false;
                if let Some(idx) = self.levels[li - 1]
                    .locals
                    .iter()
                    .rposition(|l| l.reg == reg && &*l.name == name)
                {
                    self.levels[li - 1].locals[idx].captured = true;
                    read_only = self.levels[li - 1].locals[idx].read_only;
                }
                let ui = self.levels[li].upvals.len() as u32;
                if ui >= max_upvals(self.version) {
                    return Err(self.limit_err_at(li, "upvalues", max_upvals(self.version)));
                }
                self.levels[li].upvals.push(UpvalDesc {
                    in_stack: true,
                    index: reg as u8,
                    name: name.into(),
                    read_only,
                });
                Ok(VarKind::Upval(ui))
            }
            VarKind::Upval(pidx) => {
                let read_only = self.levels[li - 1].upvals[pidx as usize].read_only;
                let ui = self.levels[li].upvals.len() as u32;
                if ui >= max_upvals(self.version) {
                    return Err(self.limit_err_at(li, "upvalues", max_upvals(self.version)));
                }
                self.levels[li].upvals.push(UpvalDesc {
                    in_stack: false,
                    index: pidx as u8,
                    name: name.into(),
                    read_only,
                });
                Ok(VarKind::Upval(ui))
            }
        }
    }

    fn local_is_read_only(&self, reg: u32) -> Option<&str> {
        self.lr()
            .locals
            .iter()
            .rev()
            .find(|l| l.reg == reg)
            .filter(|l| l.read_only)
            .map(|l| &*l.name)
    }

    // ---- expressions ----

    fn expr(&mut self, id: ExprId) -> Result<Exp, SyntaxError> {
        match self.ast.expr(id) {
            Expr::Nil => Ok(Exp::Nil),
            Expr::True => Ok(Exp::True),
            Expr::False => Ok(Exp::False),
            Expr::Int(i) => Ok(Exp::Int(*i)),
            Expr::Float(f) => Ok(Exp::Float(*f)),
            Expr::Str(s) => {
                let s = s.clone();
                Ok(Exp::Const(self.str_const(&s)))
            }
            Expr::Name(n) => {
                self.last_line = n.line;
                let text = n.text.clone();
                self.name_expr(&text)
            }
            Expr::Paren(inner) => {
                // parentheses truncate multiple results to exactly one
                let e = self.expr(*inner)?;
                if let Exp::Open { .. } = e {
                    Ok(Exp::Reg(self.exp_to_anyreg(e)?))
                } else {
                    Ok(e)
                }
            }
            Expr::Index { obj, key } => self.index_expr(*obj, *key),
            Expr::UnOp { op, operand, line } => {
                let (op, operand, line) = (*op, *operand, *line);
                self.unop(op, operand, line)
            }
            Expr::BinOp { op, lhs, rhs, line } => {
                let (op, lhs, rhs, line) = (*op, *lhs, *rhs, *line);
                self.binop(op, lhs, rhs, line)
            }
            Expr::Table { line, .. } => {
                let line = *line;
                self.table_ctor(id, line)
            }
            Expr::Vararg => self.vararg_expr(),
            Expr::Call { .. } | Expr::MethodCall { .. } => self.call_expr(id),
            Expr::Function(body) => {
                let body = body.clone();
                self.function_exp(&body, false)
            }
        }
    }

    fn name_expr(&mut self, name: &str) -> Result<Exp, SyntaxError> {
        match self.resolve_name(name)? {
            VarKind::Local(reg) => Ok(Exp::Reg(reg)),
            VarKind::Upval(u) => Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetUpval,
                0,
                u,
                0,
                false,
            )))),
            VarKind::Global { .. } => {
                // declaration check (5.5): undeclared names error under a
                // strict regime; reads are fine for const globals
                let line = self.last_line;
                self.resolve_global_kind(name, line)?;
                self.global_access(name)
            }
        }
    }

    /// `_ENV[name]` with `_ENV` resolved through the scope chain (it can be
    /// shadowed by a local or captured as an upvalue).
    /// True when `_ENV` itself has been pulled into a `global` declaration: any
    /// global access then needs `_ENV._ENV`, which is itself global — an error
    /// (PUC's `buildglobal` rejects a VGLOBAL environment).
    fn env_is_global(&self) -> bool {
        self.levels.iter().rev().any(|lvl| {
            lvl.blocks
                .iter()
                .any(|b| b.gdecls.iter().any(|(n, _)| &**n == "_ENV"))
        })
    }

    /// Emit the runtime "already defined" guard for a defining `global` write:
    /// reads the current value of the global and errors (OP_ERRNNIL) if it is
    /// not nil. Only `global x = ...` and `global function x` use this.
    fn emit_global_redef_check(&mut self, name: &str) -> Result<(), SyntaxError> {
        let saved = self.lr().freereg;
        let e = self.global_access(name)?;
        let r = self.exp_to_anyreg(e)?;
        let c = self.str_const(name.as_bytes());
        let bx = if c < MAX_BX { c + 1 } else { 0 };
        self.emit(Inst::iabx(Op::ErrNNil, r, bx));
        self.set_freereg(saved);
        Ok(())
    }

    fn global_access(&mut self, name: &str) -> Result<Exp, SyntaxError> {
        if self.env_is_global() {
            return Err(self.err(
                self.last_line,
                format!("_ENV is global when accessing variable '{name}'"),
            ));
        }
        let c = self.str_const(name.as_bytes());
        match self.resolve_name("_ENV")? {
            VarKind::Upval(u) if c <= 0xFF => Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetTabUp,
                0,
                u,
                c,
                true,
            )))),
            VarKind::Local(r) if c <= 0xFF => Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetField,
                0,
                r,
                c,
                true,
            )))),
            env => {
                // rare: huge constant index — go through registers
                let er = self.reserve(2)?;
                match env {
                    VarKind::Upval(u) => {
                        self.emit(Inst::iabc(Op::GetUpval, er, u, 0, false));
                    }
                    VarKind::Local(r) => {
                        self.emit(Inst::iabc(Op::Move, er, r, 0, false));
                    }
                    VarKind::Global { .. } => unreachable!("_ENV always resolves"),
                }
                self.load_const(er + 1, c);
                self.set_freereg(er);
                Ok(Exp::Reloc(self.emit(Inst::iabc(
                    Op::GetTable,
                    0,
                    er,
                    er + 1,
                    false,
                ))))
            }
        }
    }

    fn vararg_expr(&mut self) -> Result<Exp, SyntaxError> {
        if !self.lr().is_vararg {
            return Err(self.err(self.last_line, "cannot use '...' outside a vararg function"));
        }
        let base = self.reserve(1)?;
        let pc = self.emit(Inst::iabc(Op::Vararg, base, 0, 2, false));
        Ok(Exp::Open { pc, base })
    }

    fn call_expr(&mut self, id: ExprId) -> Result<Exp, SyntaxError> {
        match self.ast.expr(id) {
            Expr::Call { func, args, line } => {
                let (func, args, line) = (*func, args.clone(), *line);
                let base = self.lr().freereg;
                let fe = self.expr(func)?;
                self.set_freereg(base);
                let r = self.exp_to_nextreg(fe)?;
                debug_assert_eq!(r, base);
                let (nfixed, open) = self.args_onto_stack(&args, base + 1)?;
                self.last_line = line;
                let b = if open { 0 } else { nfixed + 1 };
                let pc = self.emit(Inst::iabc(Op::Call, base, b, 2, false));
                self.set_freereg(base + 1);
                Ok(Exp::Open { pc, base })
            }
            Expr::MethodCall {
                obj,
                method,
                args,
                line,
            } => {
                let (obj, method, args, line) = (*obj, method.clone(), args.clone(), *line);
                let base = self.lr().freereg;
                let oe = self.expr(obj)?;
                let o = self.exp_to_anyreg(oe)?;
                self.set_freereg(base);
                self.reserve(2)?;
                let c = self.str_const(method.text.as_bytes());
                self.last_line = line;
                if c <= 0xFF {
                    self.emit(Inst::iabc(Op::SelfOp, base, o, c, true));
                } else if self.version >= LuaVersion::Lua55 {
                    // PUC 5.5 drops back to a plain GETTABLE when the SELF
                    // C-operand can't fit the constant — getobjname then
                    // classifies the call as "field". 5.5 errors.lua :328
                    // bakes the wording in (its comment literally says
                    // "cannot use 'self' opcode").
                    self.emit(Inst::iabc(Op::Move, base + 1, o, 0, false));
                    let kr = self.reserve(1)?;
                    self.load_const(kr, c);
                    self.emit(Inst::iabc(Op::GetTable, base, base + 1, kr, false));
                    self.set_freereg(base + 2);
                } else {
                    // PUC 5.4 `luaK_exp2RK`: load the key into a register and
                    // emit OP_SELF against it (k=false). The SELF tag stays on
                    // the instruction so getobjname classifies the call as
                    // "method" — 5.4 errors.lua :303 exercises this path.
                    let kr = self.reserve(1)?;
                    self.load_const(kr, c);
                    self.set_freereg(base);
                    self.reserve(2)?;
                    self.emit(Inst::iabc(Op::SelfOp, base, o, kr, false));
                }
                let (nfixed, open) = self.args_onto_stack(&args, base + 2)?;
                self.last_line = line;
                let b = if open { 0 } else { nfixed + 2 };
                let pc = self.emit(Inst::iabc(Op::Call, base, b, 2, false));
                self.set_freereg(base + 1);
                Ok(Exp::Open { pc, base })
            }
            _ => unreachable!(),
        }
    }

    /// Stack call arguments at consecutive registers from `argbase`.
    /// Returns (fixed_arg_count, last_is_open).
    fn args_onto_stack(
        &mut self,
        args: &[ExprId],
        argbase: u32,
    ) -> Result<(u32, bool), SyntaxError> {
        for (i, &a) in args.iter().enumerate() {
            let dst = argbase + i as u32;
            if dst >= MAX_REGS {
                // PUC `checkstack` raises "function or expression needs too
                // many registers" once the per-function register cap is hit;
                // a too-wide call site is just one path into it (errors.lua
                // :740 checkmessage "too many registers").
                return Err(self.err(
                    self.last_line,
                    "function or expression needs too many registers",
                ));
            }
            self.set_freereg(dst);
            let last = i == args.len() - 1;
            let e = self.expr(a)?;
            if last && let Exp::Open { pc, base } = e {
                debug_assert_eq!(base, dst);
                self.patch_wanted(pc, 0);
                return Ok((args.len() as u32 - 1, true));
            }
            self.set_freereg(dst);
            let got = self.exp_to_nextreg(e)?;
            debug_assert_eq!(got, dst);
        }
        Ok((args.len() as u32, false))
    }

    // ---- named-vararg materialization pre-scan ----
    //
    // A named vararg `...t` can stay a virtual stack view (no heap table) as
    // long as every use is a read `t[k]` / `t.n`. Any write, bare use (passed,
    // returned, an operand, a method receiver) or capture by a nested function
    // forces a real table. These walkers conservatively return `true` (force)
    // on anything but a read-index of the name.

    fn vararg_forced(&self, block: &ast::Block, name: &str) -> bool {
        block.stats.iter().any(|&s| self.stat_forces(s, name))
    }

    fn block_forces(&self, block: &ast::Block, name: &str) -> bool {
        block.stats.iter().any(|&s| self.stat_forces(s, name))
    }

    fn stat_forces(&self, s: StatId, name: &str) -> bool {
        use ast::Stat::*;
        match self.ast.stat(s) {
            Do(b) => self.block_forces(b, name),
            While { cond, body } => {
                self.expr_forces(*cond, name, false) || self.block_forces(body, name)
            }
            Repeat { body, cond } => {
                self.block_forces(body, name) || self.expr_forces(*cond, name, false)
            }
            If { arms, else_body } => {
                arms.iter().any(|(c, _, b)| {
                    self.expr_forces(*c, name, false) || self.block_forces(b, name)
                }) || else_body
                    .as_ref()
                    .is_some_and(|b| self.block_forces(b, name))
            }
            NumericFor {
                start,
                limit,
                step,
                body,
                ..
            } => {
                self.expr_forces(*start, name, false)
                    || self.expr_forces(*limit, name, false)
                    || step.is_some_and(|e| self.expr_forces(e, name, false))
                    || self.block_forces(body, name)
            }
            GenericFor { exprs, body, .. } => {
                exprs.iter().any(|&e| self.expr_forces(e, name, false))
                    || self.block_forces(body, name)
            }
            Local { exprs, .. } | Global { exprs, .. } => {
                exprs.iter().any(|&e| self.expr_forces(e, name, false))
            }
            GlobalAll { .. } | Break { .. } | Goto(_) | Label(_) => false,
            Assign { targets, exprs } => {
                targets.iter().any(|&t| self.target_forces(t, name))
                    || exprs.iter().any(|&e| self.expr_forces(e, name, false))
            }
            Call(e) => self.expr_forces(*e, name, false),
            // a nested function capturing the name escapes the vararg
            Function { body, .. } | LocalFunction { body, .. } | GlobalFunction { body, .. } => {
                self.mentions_block(&body.block, name)
            }
            Return { exprs, .. } => exprs.iter().any(|&e| self.expr_forces(e, name, false)),
        }
    }

    /// An assignment target: an `Index` write to the name (or assigning to the
    /// name itself) forces materialization.
    fn target_forces(&self, t: ExprId, name: &str) -> bool {
        match self.ast.expr(t) {
            ast::Expr::Index { obj, key } => {
                self.expr_forces(*obj, name, false) || self.expr_forces(*key, name, false)
            }
            ast::Expr::Name(n) => &*n.text == name,
            _ => self.expr_forces(t, name, false),
        }
    }

    /// `is_index_obj` is true when `e` is the object slot of a *read* index — the
    /// one position where a bare reference to the vararg is allowed to stay
    /// virtual.
    fn expr_forces(&self, e: ExprId, name: &str, is_index_obj: bool) -> bool {
        use ast::Expr::*;
        match self.ast.expr(e) {
            Name(n) => &*n.text == name && !is_index_obj,
            Index { obj, key } => {
                self.expr_forces(*obj, name, true) || self.expr_forces(*key, name, false)
            }
            Call { func, args, .. } => {
                self.expr_forces(*func, name, false)
                    || args.iter().any(|&a| self.expr_forces(a, name, false))
            }
            MethodCall { obj, args, .. } => {
                self.expr_forces(*obj, name, false)
                    || args.iter().any(|&a| self.expr_forces(a, name, false))
            }
            BinOp { lhs, rhs, .. } => {
                self.expr_forces(*lhs, name, false) || self.expr_forces(*rhs, name, false)
            }
            UnOp { operand, .. } => self.expr_forces(*operand, name, false),
            Paren(inner) => self.expr_forces(*inner, name, false),
            Table { fields, .. } => fields.iter().any(|f| self.field_forces(f, name)),
            Function(body) => self.mentions_block(&body.block, name),
            Nil | True | False | Vararg | Int(_) | Float(_) | Str(_) => false,
        }
    }

    fn field_forces(&self, f: &ast::TableField, name: &str) -> bool {
        match f {
            ast::TableField::Item(e) => self.expr_forces(*e, name, false),
            ast::TableField::Named(_, e) => self.expr_forces(*e, name, false),
            ast::TableField::Keyed(k, v) => {
                self.expr_forces(*k, name, false) || self.expr_forces(*v, name, false)
            }
        }
    }

    /// Whether `name` appears *anywhere* inside a (nested) block — any mention
    /// means the vararg is captured as an upvalue, forcing materialization.
    fn mentions_block(&self, block: &ast::Block, name: &str) -> bool {
        block.stats.iter().any(|&s| self.mentions_stat(s, name))
    }

    fn mentions_stat(&self, s: StatId, name: &str) -> bool {
        use ast::Stat::*;
        match self.ast.stat(s) {
            Do(b) => self.mentions_block(b, name),
            While { cond, body } => {
                self.mentions_expr(*cond, name) || self.mentions_block(body, name)
            }
            Repeat { body, cond } => {
                self.mentions_block(body, name) || self.mentions_expr(*cond, name)
            }
            If { arms, else_body } => {
                arms.iter()
                    .any(|(c, _, b)| self.mentions_expr(*c, name) || self.mentions_block(b, name))
                    || else_body
                        .as_ref()
                        .is_some_and(|b| self.mentions_block(b, name))
            }
            NumericFor {
                start,
                limit,
                step,
                body,
                ..
            } => {
                self.mentions_expr(*start, name)
                    || self.mentions_expr(*limit, name)
                    || step.is_some_and(|e| self.mentions_expr(e, name))
                    || self.mentions_block(body, name)
            }
            GenericFor { exprs, body, .. } => {
                exprs.iter().any(|&e| self.mentions_expr(e, name))
                    || self.mentions_block(body, name)
            }
            Local { exprs, .. } | Global { exprs, .. } | Return { exprs, .. } => {
                exprs.iter().any(|&e| self.mentions_expr(e, name))
            }
            Assign { targets, exprs } => {
                targets.iter().any(|&e| self.mentions_expr(e, name))
                    || exprs.iter().any(|&e| self.mentions_expr(e, name))
            }
            Call(e) => self.mentions_expr(*e, name),
            Function { body, .. } | LocalFunction { body, .. } | GlobalFunction { body, .. } => {
                self.mentions_block(&body.block, name)
            }
            GlobalAll { .. } | Break { .. } | Goto(_) | Label(_) => false,
        }
    }

    fn mentions_expr(&self, e: ExprId, name: &str) -> bool {
        use ast::Expr::*;
        match self.ast.expr(e) {
            Name(n) => &*n.text == name,
            Index { obj, key } => self.mentions_expr(*obj, name) || self.mentions_expr(*key, name),
            Call { func, args, .. } => {
                self.mentions_expr(*func, name) || args.iter().any(|&a| self.mentions_expr(a, name))
            }
            MethodCall { obj, args, .. } => {
                self.mentions_expr(*obj, name) || args.iter().any(|&a| self.mentions_expr(a, name))
            }
            BinOp { lhs, rhs, .. } => {
                self.mentions_expr(*lhs, name) || self.mentions_expr(*rhs, name)
            }
            UnOp { operand, .. } => self.mentions_expr(*operand, name),
            Paren(inner) => self.mentions_expr(*inner, name),
            Table { fields, .. } => fields.iter().any(|f| match f {
                ast::TableField::Item(e) => self.mentions_expr(*e, name),
                ast::TableField::Named(_, e) => self.mentions_expr(*e, name),
                ast::TableField::Keyed(k, v) => {
                    self.mentions_expr(*k, name) || self.mentions_expr(*v, name)
                }
            }),
            Function(body) => self.mentions_block(&body.block, name),
            Nil | True | False | Vararg | Int(_) | Float(_) | Str(_) => false,
        }
    }

    fn function_exp(&mut self, body: &FuncBody, is_method: bool) -> Result<Exp, SyntaxError> {
        let line = body.line;
        let nparams = body.params.len() + is_method as usize;
        if nparams > 200 {
            return Err(self.err(line, "too many parameters"));
        }
        let is_vararg = !matches!(body.vararg, ast::Vararg::None);
        let mut level = Level::new(nparams as u8, is_vararg, line);
        // PUC 5.5 `parlist`: emit a hidden `(vararg table)` locvar only for
        // an explicit anonymous `(...)` (Named goes through a real local;
        // main chunks set is_vararg implicitly with no pseudo). 5.4 and
        // earlier had no such pseudo — db.lua across versions baselines on
        // the exact `getlocal` shift: 5.5 setlocal(2, 4) = "AAAA" vs
        // 5.4/5.3/5.2 setlocal(2, 3) = "AAAA".
        level.has_vararg_table_pseudo =
            self.version >= LuaVersion::Lua55 && matches!(body.vararg, ast::Vararg::Anonymous);
        // PUC 5.1 attached an `env` slot to *every* Lua function so
        // `setfenv` always had something to rewrite, even for bodies that
        // never touched a global. luna's `_ENV`-upvalue model only captures
        // it on first global access — to keep `setfenv` semantics intact in
        // 5.1, pre-seed the upvalue list with `_ENV` so it inherits the
        // enclosing function's `_ENV` cell (resolve_at will reuse this slot
        // when the body does end up reading a global). 5.2+ keeps the
        // lazy-capture model.
        if self.version == LuaVersion::Lua51 {
            let parent_idx = self.levels.len() - 1;
            let parent_env = self.levels[parent_idx]
                .upvals
                .iter()
                .enumerate()
                .find(|(_, d)| &*d.name == "_ENV")
                .map(|(i, _)| i);
            let env_desc = if let Some(pi) = parent_env {
                let parent_ro = self.levels[parent_idx].upvals[pi].read_only;
                UpvalDesc {
                    in_stack: false,
                    index: pi as u8,
                    name: "_ENV".into(),
                    read_only: parent_ro,
                }
            } else {
                UpvalDesc {
                    in_stack: false,
                    index: 0,
                    name: "_ENV".into(),
                    read_only: false,
                }
            };
            level.upvals.push(env_desc);
        }
        self.levels.push(level);
        self.enter_block(false);
        if is_method {
            self.declare_local("self", 0, false)?;
        }
        for (i, p) in body.params.iter().enumerate() {
            self.declare_local(&p.text, (i + is_method as usize) as u32, false)?;
        }
        if let ast::Vararg::Named(n) = &body.vararg {
            let name = n.text.clone();
            let r = self.reserve(1)?;
            // 5.5: the named vararg table is a read-only local. If the pre-scan
            // proves it is only ever read as `t[k]`/`t.n` (never written, never
            // escaping, not `_ENV`) it stays *virtual* — indexed straight off the
            // stack varargs with no heap table. Otherwise materialize it now.
            let virtual_ok = &*name != "_ENV" && !self.vararg_forced(&body.block, &name);
            if virtual_ok {
                self.declare_local(&name, r, true)?;
                self.l()
                    .locals
                    .last_mut()
                    .expect("just declared")
                    .vararg_virtual = true;
            } else {
                self.emit(Inst::iabc(Op::GetVarg, r, 0, 0, false));
                self.declare_local(&name, r, true)?;
            }
        }
        // PUC 5.1's `LUA_COMPAT_VARARG` reserves the *name* `arg` as a hidden
        // local at index numparams+1 in every vararg function. Whether the
        // slot ends up populated as a table is a separate question that
        // vararg.lua contradicts itself on (`:6` wants `arg` to be a table
        // inside `function f(a, ...)` while `:13` wants `arg == nil` inside
        // `function c12 (...)`); luna leaves the slot at its register-init
        // value (nil) so the `arg == nil` half still passes — db.lua's
        // `setlocal(2, 3, "pera") == "AAAA"` only depends on the *numbering*
        // shifting by 1 to make AAAA land at local index 3, which the name
        // reservation alone delivers.
        // PUC 5.1 LUAI_COMPAT_VARARG: `(...)` functions get a hidden
        // `arg` local UNLESS the body uses `...` directly (lparser.c
        // singlevar: `simpleexp` clears VARARG_NEEDSARG on `TK_DOTS`).
        // vararg.lua relies on this: `function f(a, ...) … arg.n …
        // end` uses `arg` (no `...`) → auto-bound; `function c12 (...)
        // local x = {...}` uses `...` → no auto-`arg` (assert(arg ==
        // nil) sees the GLOBAL arg which was reset to nil at file
        // top).
        if self.version <= LuaVersion::Lua51
            && matches!(body.vararg, ast::Vararg::Anonymous)
            && !block_uses_vararg(self.ast, &body.block)
        {
            let r = self.reserve(1)?;
            self.declare_local("arg", r, false)?;
            self.l().has_compat_vararg_arg = true;
        }
        self.stat_block(&body.block)?;
        self.leave_block()?;
        // PUC attributes the implicit final return to the closing `end` line, so
        // that line shows up in `debug.getinfo(...,"L").activelines`.
        self.last_line = body.end_line;
        self.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
        let lvl = self.levels.pop().expect("function level");
        let source = self.source;
        let proto = self
            .heap
            .adopt_proto(lvl.into_proto(source, line, body.end_line));
        let idx = self.lr().protos.len() as u32;
        if idx > MAX_BX {
            return Err(self.err(line, "too many nested functions"));
        }
        self.l().protos.push(proto);
        // PUC emits OP_CLOSURE with the line of the just-consumed `end` token
        // (luaK_code uses ls->lastline), so the closure-creation line event lands
        // on the function's last line, not its `function` keyword.
        self.last_line = body.end_line;
        Ok(Exp::Reloc(self.emit(Inst::iabx(Op::Closure, 0, idx))))
    }

    /// Materialize into a specific register.
    fn exp_to_reg(&mut self, e: Exp, reg: u32) -> Result<(), SyntaxError> {
        match e {
            Exp::Nil => {
                self.emit(Inst::iabc(Op::LoadNil, reg, 0, 0, false));
            }
            Exp::True => {
                self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
            }
            Exp::False => {
                self.emit(Inst::iabc(Op::LoadFalse, reg, 0, 0, false));
            }
            Exp::Int(i) => {
                if (-65535..=65535).contains(&i) {
                    self.emit(Inst::iasbx(Op::LoadI, reg, i as i32));
                } else {
                    let c = self.const_idx(ConstKey::Int(i), Value::Int(i));
                    self.load_const(reg, c);
                }
            }
            Exp::Float(f) => {
                let as_int = f as i32;
                // bit-compare so the LoadF fast path doesn't fold -0.0 to +0.0
                // (`-0.0 == 0.0` but their bit patterns differ)
                if (-65535..=65535).contains(&as_int) && (as_int as f64).to_bits() == f.to_bits() {
                    self.emit(Inst::iasbx(Op::LoadF, reg, as_int));
                } else {
                    let c = self.const_idx(ConstKey::Float(f.to_bits()), Value::Float(f));
                    self.load_const(reg, c);
                }
            }
            Exp::Const(c) => self.load_const(reg, c),
            Exp::Reg(r) => {
                if r != reg {
                    self.emit(Inst::iabc(Op::Move, reg, r, 0, false));
                }
            }
            Exp::Reloc(pc) => self.patch_dest(pc, reg),
            Exp::Cmp { op, l, r } => {
                self.emit(Inst::iabc(op, l, r, 0, true));
                self.emit(Inst::isj(Op::Jmp, 1));
                self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
                let tpad = self.here();
                self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
                // Jmp(1) above skips the LFalseSkip and lands on the LoadTrue
                // pad — that pc is a jump destination.
                self.mark_target(tpad);
            }
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                if base != reg {
                    self.emit(Inst::iabc(Op::Move, reg, base, 0, false));
                }
            }
        }
        Ok(())
    }

    fn exp_to_nextreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        let reg = self.reserve(1)?;
        self.exp_to_reg(e, reg)?;
        Ok(reg)
    }

    fn exp_to_anyreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        match e {
            Exp::Reg(r) => Ok(r),
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                Ok(base)
            }
            e => self.exp_to_nextreg(e),
        }
    }

    /// Compile a condition; the returned JMP pc is taken when it is FALSE.
    fn cond_jump_false(&mut self, id: ExprId) -> Result<usize, SyntaxError> {
        let saved = self.lr().freereg;
        let e = self.expr(id)?;
        match e {
            Exp::Cmp { op, l, r } => {
                self.emit(Inst::iabc(op, l, r, 0, false));
            }
            e => {
                let r = self.exp_to_anyreg(e)?;
                self.emit(Inst::iabc(Op::Test, r, 0, 0, false));
            }
        }
        self.set_freereg(saved);
        Ok(self.emit_jump())
    }

    fn unop(&mut self, op: UnOp, operand: ExprId, line: u32) -> Result<Exp, SyntaxError> {
        self.last_line = line;
        let e = self.expr(operand)?;
        let saved = self.lr().freereg;
        let (opcode, folded) = match op {
            UnOp::Neg => match e {
                Exp::Int(i) => return Ok(Exp::Int(i.wrapping_neg())),
                Exp::Float(f) => return Ok(Exp::Float(-f)),
                e => (Op::Unm, e),
            },
            UnOp::Not => match e {
                Exp::Nil | Exp::False => return Ok(Exp::True),
                Exp::True | Exp::Int(_) | Exp::Float(_) | Exp::Const(_) => return Ok(Exp::False),
                e => (Op::Not, e),
            },
            UnOp::Len => (Op::Len, e),
            UnOp::BNot => match e {
                Exp::Int(i) => return Ok(Exp::Int(!i)),
                e => (Op::BNot, e),
            },
        };
        let r = self.exp_to_anyreg(folded)?;
        self.set_freereg(saved);
        self.last_line = line;
        Ok(Exp::Reloc(self.emit(Inst::iabc(opcode, 0, r, 0, false))))
    }

    fn binop(
        &mut self,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        line: u32,
    ) -> Result<Exp, SyntaxError> {
        match op {
            BinOp::And | BinOp::Or => return self.and_or(op, lhs, rhs, line),
            BinOp::Concat => return self.concat(lhs, rhs, line),
            _ => {}
        }
        let saved = self.lr().freereg;
        // PUC's `infix` discharges the left operand *after* consuming the
        // operator token (luaK_indexed → luaK_exp2anyreg called from infix),
        // so any GET emitted for the lhs lands on the operator's line, not
        // on the line of the lhs itself (db.lua :193 line-trace family).
        // luna parses the lhs ahead of time; pin the line through
        // `force_line` for the duration of the lhs walk so every emit it
        // performs is attributed to the operator's line, then drop the pin
        // before parsing the rhs (which discharges at its own last-token
        // line, matching PUC).
        let saved_force = self.force_line.replace(line);
        let le = self.expr(lhs)?;
        if let Some(folded) = fold_arith(op, &le, self.ast, rhs) {
            self.force_line = saved_force;
            return Ok(folded);
        }
        let l = self.exp_to_anyreg(le)?;
        self.force_line = saved_force;
        // Protect the left operand's register if it is a fresh temporary at the
        // top of the stack (e.g. a CONCAT result): evaluating the right operand
        // must not reuse and clobber it before the binary op reads it.
        if l >= saved {
            self.set_freereg(l + 1);
        }
        let re = self.expr(rhs)?;
        let r = self.exp_to_anyreg(re)?;
        self.set_freereg(saved);
        // PUC attributes the arith op itself to the operator's line, but
        // leaves `lastline` at the rhs's last token (so a following SETTABUP
        // / SETUPVAL for the assignment lands on the rhs's end line, not the
        // operator). Pin the line for the arith emit, but don't stomp
        // `last_line` permanently.
        let saved_force_arith = self.force_line.replace(line);
        let r_op = (|| -> Result<Exp, SyntaxError> {
            Ok(match op {
                BinOp::Add => self.arith(Op::Add, l, r),
                BinOp::Sub => self.arith(Op::Sub, l, r),
                BinOp::Mul => self.arith(Op::Mul, l, r),
                BinOp::Div => self.arith(Op::Div, l, r),
                BinOp::IDiv => self.arith(Op::IDiv, l, r),
                BinOp::Mod => self.arith(Op::Mod, l, r),
                BinOp::Pow => self.arith(Op::Pow, l, r),
                BinOp::BAnd => self.arith(Op::BAnd, l, r),
                BinOp::BOr => self.arith(Op::BOr, l, r),
                BinOp::BXor => self.arith(Op::BXor, l, r),
                BinOp::Shl => self.arith(Op::Shl, l, r),
                BinOp::Shr => self.arith(Op::Shr, l, r),
                BinOp::Eq => Exp::Cmp { op: Op::Eq, l, r },
                BinOp::Ne => self.negate_cmp(Op::Eq, l, r)?,
                BinOp::Lt => Exp::Cmp { op: Op::Lt, l, r },
                BinOp::Le => Exp::Cmp { op: Op::Le, l, r },
                BinOp::Gt => Exp::Cmp {
                    op: Op::Lt,
                    l: r,
                    r: l,
                },
                BinOp::Ge => Exp::Cmp {
                    op: Op::Le,
                    l: r,
                    r: l,
                },
                BinOp::And | BinOp::Or | BinOp::Concat => unreachable!(),
            })
        })();
        self.force_line = saved_force_arith;
        r_op
    }

    fn arith(&mut self, op: Op, l: u32, r: u32) -> Exp {
        Exp::Reloc(self.emit(Inst::iabc(op, 0, l, r, false)))
    }

    /// `a ~= b`: comparison materialized with inverted k.
    fn negate_cmp(&mut self, op: Op, l: u32, r: u32) -> Result<Exp, SyntaxError> {
        let reg = self.reserve(1)?;
        self.l().freereg -= 1;
        self.emit(Inst::iabc(op, l, r, 0, false));
        self.emit(Inst::isj(Op::Jmp, 1));
        self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
        let tpad = self.here();
        self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
        // Jmp(1) lands on tpad — mark.
        self.mark_target(tpad);
        Ok(Exp::Reg(reg))
    }

    fn and_or(
        &mut self,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        line: u32,
    ) -> Result<Exp, SyntaxError> {
        self.last_line = line;
        let base = self.lr().freereg;
        let le = self.expr(lhs)?;
        // PUC's jumplist for `X and Y` / `X or Y` when X is a comparison:
        // skip materializing X to a bool — the comparison's own conditional
        // jump *is* the short-circuit. For AND, emit the Cmp with k=false so
        // its Jmp fires on FALSE (short-circuit-to-false); for OR, k=true so
        // the Jmp fires on TRUE (short-circuit-to-true). Then compile Y into
        // the result register and patch X's jump to land at the matching pad
        // of Y's materialization. db.lua :603 (count-hook ceiling) needs the
        // 3-op savings vs the legacy `materialize X → Test → Jmp` path.
        if let Exp::Cmp { op: cop, l, r } = le {
            let is_and = matches!(op, BinOp::And);
            // For AND, Jmp on cond==false (k=false). For OR, Jmp on cond==true (k=true).
            self.emit(Inst::iabc(cop, l, r, 0, !is_and));
            let jmp_lhs = self.emit_jump();
            self.set_freereg(base);
            let re = self.expr(rhs)?;
            // RHS shapes that leak freereg += 1 (nested and/or, function call,
            // table ctor) would make the next `reserve(1)` return `base + 1`,
            // tripping the debug_assert and silently emitting `LFalseSkip` /
            // `LoadTrue` at `base + 1` in release — clobbering RHS's
            // temporary. Restore the invariant before reserving the result
            // slot, mirroring the non-Cmp branch below (lines 1786-1796).
            self.set_freereg(base);
            let reg = self.reserve(1)?;
            debug_assert_eq!(reg, base);
            // Materialize RHS into `reg` with the standard Cmp pad shape
            // (Lt + Jmp + LFalseSkip + LoadTrue) so X's short-circuit jump
            // can land on the matching pad slot.
            let (false_pad_pc, true_pad_pc) = match re {
                Exp::Cmp {
                    op: y_op,
                    l: y_l,
                    r: y_r,
                } => {
                    self.emit(Inst::iabc(y_op, y_l, y_r, 0, true));
                    self.emit(Inst::isj(Op::Jmp, 1));
                    let fpad = self.here();
                    self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
                    let tpad = self.here();
                    self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
                    // Jmp(1) skips LFalseSkip → tpad; LHS short-circuit will
                    // also patch into one of these pads below.
                    self.mark_target(tpad);
                    self.mark_target(fpad);
                    (fpad, tpad)
                }
                _ => {
                    // RHS is a regular value: materialize it normally, then
                    // emit an inline false/true pad after a skip jump so the
                    // LHS short-circuit lands on the matching constant.
                    self.set_freereg(reg);
                    self.exp_to_reg(re, reg)?;
                    let jmp_over = self.emit_jump();
                    let fpad = self.here();
                    self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
                    let tpad = self.here();
                    self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
                    self.patch_to_here(jmp_over)?;
                    // LHS short-circuit patches into fpad or tpad below.
                    self.mark_target(fpad);
                    self.mark_target(tpad);
                    (fpad, tpad)
                }
            };
            let target = if is_and { false_pad_pc } else { true_pad_pc };
            let off = target as i64 - jmp_lhs as i64 - 1;
            if off.unsigned_abs() > MAX_SJ as u64 {
                return Err(self.err(line, "control structure too long"));
            }
            self.l().code[jmp_lhs].set_sj(off as i32);
            return Ok(Exp::Reg(reg));
        }
        self.set_freereg(base);
        let reg = self.exp_to_nextreg(le)?;
        debug_assert_eq!(reg, base);
        let k = op == BinOp::Or;
        self.emit(Inst::iabc(Op::Test, reg, 0, 0, k));
        let jmp = self.emit_jump();
        self.set_freereg(reg);
        let re = self.expr(rhs)?;
        self.set_freereg(reg);
        let got = self.exp_to_nextreg(re)?;
        debug_assert_eq!(got, reg);
        self.patch_to_here(jmp)?;
        Ok(Exp::Reg(reg))
    }

    fn concat(&mut self, lhs: ExprId, rhs: ExprId, line: u32) -> Result<Exp, SyntaxError> {
        // PUC `luaK_concat` collapses a right-associative `a..b..c..d..…`
        // chain into a single OP_CONCAT with `b = nargs`. luna previously
        // emitted one OP_CONCAT per binary, building a chain of pair-folds;
        // a 128-operand chain (5.1 big.lua's `rep129(longs)`) then needed
        // dozens of intern+hash rounds over multi-GB intermediates before
        // hitting `concat_pair`'s overflow check. Flatten upfront so the
        // run-side pre-sum can short-circuit the whole expression.
        //
        // Collect the right-associative chain's operands left-to-right.
        let mut operands: Vec<ExprId> = vec![lhs];
        let mut cur = rhs;
        loop {
            match self.ast.expr(cur) {
                Expr::BinOp {
                    op: BinOp::Concat,
                    lhs: l,
                    rhs: r,
                    ..
                } => {
                    operands.push(*l);
                    cur = *r;
                }
                _ => {
                    operands.push(cur);
                    break;
                }
            }
        }
        let base = self.lr().freereg;
        let mut nargs = 0u32;
        for (idx, &eid) in operands.iter().enumerate() {
            self.set_freereg(base + nargs);
            let e = self.expr(eid)?;
            self.set_freereg(base + nargs);
            let r = self.exp_to_nextreg(e)?;
            debug_assert_eq!(r, base + nargs);
            nargs += 1;
            // OP_CONCAT's `b` field is one byte (`b = nargs`), capping the
            // chain at 254 fixed operands. Anything beyond closes the
            // current segment and starts a fresh outer concat with the
            // running result as the new lhs.
            let is_last = idx + 1 == operands.len();
            if !is_last && nargs == 254 {
                self.set_freereg(base);
                self.last_line = line;
                self.emit(Inst::iabc(Op::Concat, base, nargs, 0, false));
                nargs = 1;
            }
        }
        self.set_freereg(base);
        self.last_line = line;
        self.emit(Inst::iabc(Op::Concat, base, nargs, 0, false));
        Ok(Exp::Reg(base))
    }

    /// True when `name` resolves, in the current function, to a named vararg
    /// kept virtual (so `name[k]` indexes the stack varargs via OP_VARGIDX).
    fn vararg_virtual_local(&self, name: &str) -> bool {
        let lvl = self.lr();
        // a more-recent `global name` marker shadows the local
        if let Some(av) = lvl
            .avars
            .iter()
            .rev()
            .find(|a| a.name.as_deref() == Some(name))
            && av.reg.is_none()
        {
            return false;
        }
        lvl.locals
            .iter()
            .rposition(|l| &*l.name == name)
            .is_some_and(|idx| lvl.locals[idx].vararg_virtual)
    }

    fn index_expr(&mut self, obj: ExprId, key: ExprId) -> Result<Exp, SyntaxError> {
        // a read `t[k]` / `t.n` of a virtual named vararg: index the stack
        // varargs directly (OP_VARGIDX), allocating no table.
        if let Expr::Name(n) = self.ast.expr(obj) {
            let n = n.text.clone();
            if self.vararg_virtual_local(&n) {
                let saved = self.lr().freereg;
                let ke = self.expr(key)?;
                let k = self.exp_to_anyreg(ke)?;
                let e = Exp::Reloc(self.emit(Inst::iabc(Op::VargIdx, 0, 0, k, false)));
                self.set_freereg(saved);
                return Ok(e);
            }
        }
        let saved = self.lr().freereg;
        let oe = self.expr(obj)?;
        let o = self.exp_to_anyreg(oe)?;
        let e = match self.ast.expr(key) {
            Expr::Str(s) if s.len() <= 255 => {
                let s = s.clone();
                let c = self.str_const(&s);
                if c <= 0xFF {
                    Exp::Reloc(self.emit(Inst::iabc(Op::GetField, 0, o, c, true)))
                } else {
                    let ke = Exp::Const(c);
                    let k = self.exp_to_nextreg(ke)?;
                    Exp::Reloc(self.emit(Inst::iabc(Op::GetTable, 0, o, k, false)))
                }
            }
            Expr::Int(i) if (0..=255).contains(i) => {
                let c = *i as u32;
                Exp::Reloc(self.emit(Inst::iabc(Op::GetI, 0, o, c, false)))
            }
            _ => {
                let ke = self.expr(key)?;
                let k = self.exp_to_anyreg(ke)?;
                Exp::Reloc(self.emit(Inst::iabc(Op::GetTable, 0, o, k, false)))
            }
        };
        self.set_freereg(saved);
        Ok(e)
    }

    fn table_ctor(&mut self, id: ExprId, line: u32) -> Result<Exp, SyntaxError> {
        let Expr::Table { fields, .. } = self.ast.expr(id) else {
            unreachable!()
        };
        self.last_line = line;
        let treg = self.reserve(1)?;
        let (mut narr, mut nhash) = (0u32, 0u32);
        for f in fields {
            match f {
                TableField::Item(_) => narr += 1,
                _ => nhash += 1,
            }
        }
        self.emit(Inst::iabc(
            Op::NewTable,
            treg,
            narr.min(255),
            nhash.min(255),
            false,
        ));
        const FIELDS_PER_FLUSH: u32 = 50;
        let mut pending = 0u32;
        let mut flushed = 0u32;
        let fields: Vec<TableField> = fields.clone();
        let n_items = fields
            .iter()
            .filter(|f| matches!(f, TableField::Item(_)))
            .count();
        let mut item_idx = 0usize;
        for f in &fields {
            match f {
                TableField::Item(v) => {
                    item_idx += 1;
                    let dst = treg + 1 + pending;
                    if dst >= MAX_REGS {
                        return Err(self.err(line, "constructor too long"));
                    }
                    self.set_freereg(dst);
                    let e = self.expr(*v)?;
                    // last positional item: calls/varargs stay open
                    if item_idx == n_items
                        && let Exp::Open { pc, base } = e
                    {
                        debug_assert_eq!(base, dst);
                        self.patch_wanted(pc, 0);
                        self.setlist_open(treg, flushed)?;
                        pending = 0;
                        continue;
                    }
                    self.set_freereg(dst);
                    let got = self.exp_to_nextreg(e)?;
                    debug_assert_eq!(got, dst);
                    pending += 1;
                    if pending == FIELDS_PER_FLUSH {
                        self.setlist(treg, pending, flushed)?;
                        flushed += pending;
                        pending = 0;
                    }
                }
                TableField::Named(name, v) => {
                    let name = name.clone();
                    let saved = self.lr().freereg;
                    let ve = self.expr(*v)?;
                    let vr = self.exp_to_anyreg(ve)?;
                    let c = self.str_const(name.text.as_bytes());
                    if c <= 0xFF {
                        self.emit(Inst::iabc(Op::SetField, treg, c, vr, true));
                    } else {
                        let kr = self.reserve(1)?;
                        self.load_const(kr, c);
                        self.emit(Inst::iabc(Op::SetTable, treg, kr, vr, false));
                    }
                    self.set_freereg(saved);
                }
                TableField::Keyed(k, v) => {
                    let saved = self.lr().freereg;
                    let ke = self.expr(*k)?;
                    let kr = self.exp_to_anyreg(ke)?;
                    let ve = self.expr(*v)?;
                    let vr = self.exp_to_anyreg(ve)?;
                    self.emit(Inst::iabc(Op::SetTable, treg, kr, vr, false));
                    self.set_freereg(saved);
                }
            }
        }
        if pending > 0 {
            self.setlist(treg, pending, flushed)?;
        }
        self.set_freereg(treg + 1);
        Ok(Exp::Reg(treg))
    }

    fn setlist(&mut self, treg: u32, n: u32, flushed: u32) -> Result<(), SyntaxError> {
        if flushed <= 0xFF {
            self.emit(Inst::iabc(Op::SetList, treg, n, flushed, false));
        } else {
            self.emit(Inst::iabc(Op::SetList, treg, n, 0, true));
            self.emit(Inst::iax(Op::ExtraArg, flushed));
        }
        Ok(())
    }

    /// SETLIST with B=0: take items up to the runtime top.
    fn setlist_open(&mut self, treg: u32, flushed: u32) -> Result<(), SyntaxError> {
        if flushed <= 0xFF {
            self.emit(Inst::iabc(Op::SetList, treg, 0, flushed, false));
        } else {
            self.emit(Inst::iabc(Op::SetList, treg, 0, 0, true));
            self.emit(Inst::iax(Op::ExtraArg, flushed));
        }
        Ok(())
    }

    // ---- statements ----

    fn stat_block(&mut self, b: &Block) -> Result<(), SyntaxError> {
        self.stat_block_inner(b, false)
    }

    /// `until_follows` marks a repeat body: its `until` condition is still in
    /// the scope of the body's locals, so a label at the body's end is NOT
    /// trailing — a goto into it lands in those locals' scope (PUC matches a
    /// label with `block_follow(ls, 0)`, which excludes `until`).
    fn stat_block_inner(&mut self, b: &Block, until_follows: bool) -> Result<(), SyntaxError> {
        for (i, &sid) in b.stats.iter().enumerate() {
            if let Stat::Label(n) = self.ast.stat(sid) {
                // a trailing label (only labels after it) does not enter the
                // scope of the block's locals (continue-style jumps); in a
                // repeat body the trailing `until` keeps the locals alive.
                let trailing = !until_follows
                    && b.stats[i + 1..]
                        .iter()
                        .all(|&s| matches!(self.ast.stat(s), Stat::Label(_)));
                let (name, line) = (n.text.clone(), n.line);
                self.last_line = line;
                self.define_label(&name, line, trailing)?;
                continue;
            }
            self.stat(sid)?;
        }
        Ok(())
    }

    fn block_scoped(&mut self, b: &Block) -> Result<(), SyntaxError> {
        self.enter_block(false);
        self.stat_block(b)?;
        self.leave_block()
    }

    fn stat(&mut self, sid: StatId) -> Result<(), SyntaxError> {
        // attribute this statement's instructions to its own starting line (PUC
        // tracks the current line as each statement begins), so debug line info,
        // activelines, and line hooks are precise even before the first sub-
        // expression sets a finer line.
        let sline = self.ast.stat_line(sid);
        if sline != 0 {
            self.last_line = sline;
        }
        match self.ast.stat(sid) {
            Stat::Do(b) => {
                let b = b.clone();
                self.block_scoped(&b)
            }
            Stat::Local {
                collective,
                names,
                exprs,
            } => {
                let collective = *collective;
                let names: Vec<AttribName> = names.clone();
                let exprs: Vec<ExprId> = exprs.clone();
                self.local_stat(collective, &names, &exprs)
            }
            Stat::Assign { targets, exprs } => {
                let targets: Vec<ExprId> = targets.clone();
                let exprs: Vec<ExprId> = exprs.clone();
                self.assign_stat(&targets, &exprs)
            }
            Stat::If { arms, else_body } => {
                let arms: Vec<(ExprId, u32, Block)> = arms.clone();
                let else_body: Option<Block> = else_body.clone();
                self.if_stat(&arms, else_body.as_ref())
            }
            Stat::While { cond, body } => {
                let (cond, body) = (*cond, body.clone());
                self.while_stat(cond, &body)
            }
            Stat::Repeat { body, cond } => {
                let (body, cond) = (body.clone(), *cond);
                self.repeat_stat(&body, cond)
            }
            Stat::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                let var = var.clone();
                let (start, limit, step) = (*start, *limit, *step);
                let body = body.clone();
                self.numeric_for(&var.text, var.line, start, limit, step, &body)
            }
            Stat::GenericFor {
                vars,
                exprs,
                body,
                expr_line,
            } => {
                let vars = vars.clone();
                let exprs: Vec<ExprId> = exprs.clone();
                let body = body.clone();
                let expr_line = *expr_line;
                self.generic_for(&vars, &exprs, &body, expr_line)
            }
            Stat::Break { line } => {
                self.last_line = *line;
                let Some(loop_floor) = self
                    .lr()
                    .blocks
                    .iter()
                    .rev()
                    .find(|b| b.is_loop)
                    .map(|b| b.reg_floor)
                else {
                    return Err(self.err(*line, "break outside a loop"));
                };
                self.emit(Inst::iabc(Op::Close, loop_floor, 0, 0, false));
                let jmp = self.emit_jump();
                self.l()
                    .blocks
                    .iter_mut()
                    .rev()
                    .find(|b| b.is_loop)
                    .expect("loop block")
                    .breaks
                    .push(jmp);
                Ok(())
            }
            Stat::Return { exprs, line } => {
                let exprs: Vec<ExprId> = exprs.clone();
                self.last_line = *line;
                self.return_stat(&exprs)
            }
            Stat::Call(e) => {
                let e = *e;
                if let Expr::Call { line, .. } | Expr::MethodCall { line, .. } = self.ast.expr(e) {
                    self.last_line = *line;
                }
                let base = self.lr().freereg;
                let ce = self.call_expr(e)?;
                let Exp::Open { pc, .. } = ce else {
                    unreachable!()
                };
                self.patch_wanted(pc, 1); // statement call: zero results
                self.set_freereg(base);
                Ok(())
            }
            Stat::Function { name, body } => {
                let (name, body) = (name.clone(), body.clone());
                self.function_stat(&name, &body)
            }
            Stat::LocalFunction { name, body } => {
                let (name, body) = (name.clone(), body.clone());
                self.last_line = name.line;
                let reg = self.reserve(1)?;
                // declared before the body: the function can call itself
                self.declare_local(&name.text, reg, false)?;
                let f = self.function_exp(&body, false)?;
                self.exp_to_reg(f, reg)?;
                self.set_freereg(reg + 1);
                Ok(())
            }
            Stat::GlobalFunction { name, body } => {
                // `global function f` declares f, then assigns the closure
                let (name, body) = (name.clone(), body.clone());
                self.last_line = name.line;
                self.l()
                    .blocks
                    .last_mut()
                    .expect("no block")
                    .gdecls
                    .push((name.text.clone(), false));
                self.declare_global_marker(Some(&name.text));
                let saved = self.lr().freereg;
                let f = self.function_exp(&body, false)?;
                let r = self.exp_to_anyreg(f)?;
                // `global function f` is a defining write: f must not already
                // exist in the environment (runtime "already defined" check).
                // Pin the redef-check and assignment emits to the name's source
                // line, not the `end` line that `function_exp` just consumed —
                // a chunk with `_ENV = 1` then `global function foo()` should
                // raise on the name's line (errors.lua :521).
                let saved_force = self.force_line.replace(name.line);
                let res = (|| -> Result<(), SyntaxError> {
                    self.emit_global_redef_check(&name.text)?;
                    self.assign_global(&name.text, r)
                })();
                self.force_line = saved_force;
                res?;
                self.set_freereg(saved);
                Ok(())
            }
            Stat::Global {
                collective,
                names,
                exprs,
            } => {
                let (collective, names, exprs) = (*collective, names.clone(), exprs.clone());
                self.global_decl_stat(collective, &names, &exprs)
            }
            Stat::GlobalAll { attrib } => {
                let attrib = *attrib;
                if attrib == Some(ast::Attrib::Close) {
                    return Err(self.err(self.last_line, "global variables cannot be to-be-closed"));
                }
                let ro = attrib == Some(ast::Attrib::Const);
                self.l().blocks.last_mut().expect("no block").collective = Some(ro);
                // a `global *` marker participates in goto-scope checks ('*')
                self.declare_global_marker(None);
                Ok(())
            }
            Stat::Goto(n) => {
                let (name, line) = (n.text.clone(), n.line);
                self.goto_stat(&name, line)
            }
            Stat::Label(_) => unreachable!("labels handled in stat_block"),
        }
    }

    /// 5.5 `global [attrib] name {, name} [= explist]`.
    fn global_decl_stat(
        &mut self,
        collective: Option<ast::Attrib>,
        names: &[AttribName],
        exprs: &[ExprId],
    ) -> Result<(), SyntaxError> {
        // attribute validation happens before any evaluation
        for an in names {
            let attrib = an.attrib.or(collective);
            if attrib == Some(ast::Attrib::Close) {
                return Err(self.err(an.name.line, "global variables cannot be to-be-closed"));
            }
        }
        let declare = |c: &mut Self| {
            for an in names {
                let ro = an.attrib.or(collective) == Some(ast::Attrib::Const);
                c.l()
                    .blocks
                    .last_mut()
                    .expect("no block")
                    .gdecls
                    .push((an.name.text.clone(), ro));
                c.declare_global_marker(Some(&an.name.text));
            }
        };
        if exprs.is_empty() {
            declare(self);
            return Ok(());
        }
        // With an initializer the globals enter scope only AFTER the RHS is
        // evaluated (PUC bumps `nactvar` after the explist), so `global a = a`
        // reads the enclosing `a`, not the global being defined.
        let saved = self.lr().freereg;
        let base = self.explist_adjust(exprs, names.len() as u32)?;
        declare(self);
        // defining write: each target must not already exist (OP_ERRNNIL).
        for (i, an) in names.iter().enumerate() {
            let text = an.name.text.clone();
            self.emit_global_redef_check(&text)?;
            self.assign_global(&text, base + i as u32)?;
        }
        self.set_freereg(saved);
        Ok(())
    }

    fn function_stat(&mut self, name: &ast::FuncName, body: &FuncBody) -> Result<(), SyntaxError> {
        self.last_line = name.base.line;
        let is_method = name.method.is_some();
        let saved = self.lr().freereg;
        let f = self.function_exp(body, is_method)?;
        let freg = self.exp_to_anyreg(f)?;
        if name.path.is_empty() && name.method.is_none() {
            let text = name.base.text.clone();
            self.assign_name(&text, name.base.line, freg)?;
            self.set_freereg(saved);
            return Ok(());
        }
        // function a.b.c:m — walk to the holder, set the final field.
        // PUC attributes every GETFIELD/SETFIELD on the dotted name to the
        // line of the function statement's name (its `function` keyword),
        // not to the `end` token. Mirror that by pinning `force_line` for
        // the whole holder walk + final store so a `nil` base raises an
        // error on the right source line (errors.lua :430).
        let saved_force = self.force_line.replace(name.base.line);
        let res = (|| -> Result<(), SyntaxError> {
            let base_text = name.base.text.clone();
            let be = self.name_expr(&base_text)?;
            let mut holder = self.exp_to_anyreg(be)?;
            let mut fields: Vec<Box<str>> = name.path.iter().map(|n| n.text.clone()).collect();
            if let Some(m) = &name.method {
                fields.push(m.text.clone());
            }
            for f_name in &fields[..fields.len() - 1] {
                let c = self.str_const(f_name.as_bytes());
                if c <= 0xFF {
                    let pc = self.emit(Inst::iabc(Op::GetField, 0, holder, c, true));
                    let dst = self.reserve(1)?;
                    self.patch_dest(pc, dst);
                    holder = dst;
                } else {
                    let kr = self.reserve(1)?;
                    self.load_const(kr, c);
                    let pc = self.emit(Inst::iabc(Op::GetTable, 0, holder, kr, false));
                    self.patch_dest(pc, kr); // reuse the key register
                    holder = kr;
                }
            }
            let last = &fields[fields.len() - 1];
            let c = self.str_const(last.as_bytes());
            if c <= 0xFF {
                self.emit(Inst::iabc(Op::SetField, holder, c, freg, true));
            } else {
                let kr = self.reserve(1)?;
                self.load_const(kr, c);
                self.emit(Inst::iabc(Op::SetTable, holder, kr, freg, false));
            }
            Ok(())
        })();
        self.force_line = saved_force;
        res?;
        self.set_freereg(saved);
        Ok(())
    }

    fn local_stat(
        &mut self,
        collective: Option<ast::Attrib>,
        names: &[AttribName],
        exprs: &[ExprId],
    ) -> Result<(), SyntaxError> {
        let n = names.len() as u32;
        // attribute the initialiser instructions to the statement's own line (PUC
        // tracks the current line as each statement starts) so debug line info,
        // activelines, and line hooks see `local x = ...` on its real line.
        if let Some(first) = names.first() {
            self.last_line = first.name.line;
        }
        let base = self.explist_adjust(exprs, n)?;
        let mut tbc: Option<u32> = None;
        for (i, an) in names.iter().enumerate() {
            let reg = base + i as u32;
            let attrib = an.attrib.or(collective);
            let read_only = attrib.is_some(); // const and close are both read-only
            if attrib == Some(ast::Attrib::Close) {
                if tbc.is_some() {
                    return Err(self.err(
                        an.name.line,
                        "multiple to-be-closed variables in local list",
                    ));
                }
                tbc = Some(reg);
            }
            self.declare_local(&an.name.text, reg, read_only)?;
        }
        if let Some(reg) = tbc {
            self.emit(Inst::iabc(Op::Tbc, reg, 0, 0, false));
            let b = self.l().blocks.last_mut().expect("no block");
            b.has_tbc = true;
            b.tbc_scope = true;
        }
        self.set_freereg(base + n);
        Ok(())
    }

    /// Evaluate an expression list into exactly `want` consecutive registers
    /// starting at the current freereg (nil-padded / truncated; an open last
    /// expression is patched to produce the balance). Returns the base.
    fn explist_adjust(&mut self, exprs: &[ExprId], want: u32) -> Result<u32, SyntaxError> {
        let base = self.lr().freereg;
        // PUC `checkstack`: an open call (`f()`) on the last RHS slot is patched
        // to deliver up to `want` results, bypassing the per-expr `reserve`'s
        // bounds check. errors.lua :721's `local a,a,…(500),a = f()` would
        // otherwise slip past the register cap — guard the target window here.
        if base.saturating_add(want) > MAX_REGS {
            return Err(self.err(
                self.last_line,
                "function or expression needs too many registers",
            ));
        }
        if exprs.is_empty() {
            if want > 0 {
                self.reserve(want)?;
                // PUC 5.1 `luaK_nil`: at function start (pc==0) a LoadNil whose
                // first register is at-or-above nactvar is skipped — locals
                // come in already nil at frame entry, so the op is a no-op.
                // 5.2+ retired the optimization (the LoadNil shows up in the
                // line table either way), so the gate is 5.1-only. db.lua's
                // line-trace tests rely on the suppression — expected line
                // events skip the `local a` line of a chunk that opens with
                // an uninitialized declaration.
                let skip = self.version == LuaVersion::Lua51 && self.lr().code.is_empty();
                if !skip {
                    self.emit(Inst::iabc(Op::LoadNil, base, want - 1, 0, false));
                }
            }
            return Ok(base);
        }
        let n = exprs.len() as u32;
        for (i, &eid) in exprs.iter().enumerate() {
            let dst = base + i as u32;
            if dst >= MAX_REGS {
                return Err(self.err(self.last_line, "too many values in expression list"));
            }
            self.set_freereg(dst);
            let e = self.expr(eid)?;
            let last = i as u32 == n - 1;
            if last && let Exp::Open { pc, base: ob } = e {
                debug_assert_eq!(ob, dst);
                let missing = (want + 1).saturating_sub(n); // results the open expr must provide
                self.patch_wanted(pc, missing + 1);
                self.set_freereg(base + want.max(n - 1));
                return Ok(base);
            }
            self.set_freereg(dst);
            let got = self.exp_to_nextreg(e)?;
            debug_assert_eq!(got, dst);
        }
        if n < want {
            let first = self.reserve(want - n)?;
            self.emit(Inst::iabc(Op::LoadNil, first, want - n - 1, 0, false));
        }
        self.set_freereg(base + want);
        Ok(base)
    }

    fn assign_stat(&mut self, targets: &[ExprId], exprs: &[ExprId]) -> Result<(), SyntaxError> {
        let saved = self.lr().freereg;
        let want = targets.len() as u32;
        // PUC parses every LHS target (left-to-right) before the RHS explist, so
        // a name first seen on the left captures its upvalue index ahead of one
        // first seen on the right (`a = 10 + b` → a is upvalue 1, b is 2). Pre-
        // resolve the target names in that order; resolution is idempotent and
        // only affects upvalue ordering, not register use or evaluation order.
        for &t in targets {
            self.preresolve_target_upvals(t)?;
        }
        // PUC 5.5 manual §3.3.3: "Lua first evaluates all values from the
        // right-hand side and all index expressions and references on the
        // left-hand side, and only then makes the assignments." Snapshot every
        // Index LHS's obj and key into fresh registers *before* the RHS, so a
        // later store cannot see a local mutated by an earlier store
        // (e.g. attrib.lua `i, a[i], a, j, a[j], a[i+j] = j, i, i, b, j, i`).
        // PUC's `check_conflict` only snapshots locals that actually clash;
        // we copy unconditionally — costs one extra MOVE per Index LHS, much
        // simpler than tracking pairwise conflicts and never wrong.
        let mut plans: Vec<LhsPlan> = Vec::with_capacity(targets.len());
        for &t in targets {
            match self.ast.expr(t) {
                Expr::Name(_) => plans.push(LhsPlan::Name(t)),
                Expr::Index { obj, key } => {
                    let (obj, key) = (*obj, *key);
                    let oe = self.expr(obj)?;
                    // A4' (`.dev/rfcs/v2.0-pi-phase11-a4-prime-rfc.md` §2 +
                    // `.dev/rfcs/v2.1-a4-prime-prereq-verdict.md` §3): when
                    // the prereq gate certifies the obj is a non-captured
                    // bare-Name local AND the single RHS contains no
                    // UserOrUnknown call, the unconditional snapshot Move
                    // is provably redundant — reuse the local's register
                    // directly. Otherwise fall back to the snapshot.
                    let o_pinned = if self.assign_stat_can_skip_obj_snapshot(targets, exprs)
                        && matches!(oe, Exp::Reg(_))
                    {
                        match oe {
                            Exp::Reg(r) => r,
                            _ => unreachable!(),
                        }
                    } else {
                        self.exp_to_nextreg(oe)?
                    };
                    // Capture the key the same way `assign_to` does: a small
                    // string or int constant rides inline in OP_SetField /
                    // OP_SetI (so it never depends on a register that could
                    // be mutated by an intervening store), everything else
                    // gets pinned to a fresh register too.
                    let key_kind = match self.ast.expr(key) {
                        Expr::Str(s) if s.len() <= 255 => {
                            let s = s.clone();
                            let c = self.str_const(&s);
                            if c <= 0xFF {
                                SetKey::Field(c)
                            } else {
                                let kr = self.reserve(1)?;
                                self.load_const(kr, c);
                                SetKey::Reg(kr)
                            }
                        }
                        Expr::Int(i) if (0..=255).contains(i) => SetKey::Int(*i as u32),
                        _ => {
                            let ke = self.expr(key)?;
                            let kr = self.exp_to_nextreg(ke)?;
                            SetKey::Reg(kr)
                        }
                    };
                    plans.push(LhsPlan::Indexed {
                        obj: o_pinned,
                        key: key_kind,
                    });
                }
                _ => unreachable!("parser validates assignment targets"),
            }
        }
        let base = self.explist_adjust(exprs, want)?;
        for (i, plan) in plans.into_iter().enumerate() {
            let vreg = base + i as u32;
            match plan {
                LhsPlan::Name(t) => self.assign_to(t, vreg)?,
                LhsPlan::Indexed { obj, key } => match key {
                    SetKey::Field(c) => {
                        self.emit(Inst::iabc(Op::SetField, obj, c, vreg, true));
                    }
                    SetKey::Int(c) => {
                        self.emit(Inst::iabc(Op::SetI, obj, c, vreg, false));
                    }
                    SetKey::Reg(k) => {
                        self.emit(Inst::iabc(Op::SetTable, obj, k, vreg, false));
                    }
                },
            }
        }
        self.set_freereg(saved);
        Ok(())
    }

    /// Allocate upvalue indices for the names referenced by an assignment target,
    /// in source order, so they precede the RHS's (PUC restassign ordering). Only
    /// the lvalue prefix (`Name`, and the object/key of an `Index`) is walked.
    fn preresolve_target_upvals(&mut self, id: ExprId) -> Result<(), SyntaxError> {
        match self.ast.expr(id) {
            Expr::Name(n) => {
                let text = n.text.clone();
                self.resolve_name(&text)?;
            }
            Expr::Index { obj, key } => {
                let (obj, key) = (*obj, *key);
                self.preresolve_target_upvals(obj)?;
                self.preresolve_target_upvals(key)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn assign_name(&mut self, text: &str, line: u32, vreg: u32) -> Result<(), SyntaxError> {
        // PUC `restassign` emits the store with `ls->lastline` (the last token
        // of the rhs), not the line of the lhs name, so a multi-line
        // `a = b[1] \n + \n b[1]` attributes SETTABUP/SETUPVAL to the rhs's
        // end line (db.lua :193 line-trace family). Errors use the explicit
        // `line` param verbatim so a read-only-assign diagnostic still
        // points at the name.
        match self.resolve_name(text)? {
            VarKind::Local(reg) => {
                if let Some(name) = self.local_is_read_only(reg) {
                    let name = name.to_string();
                    return Err(self.err(
                        line,
                        format!("attempt to assign to const variable '{name}'"),
                    ));
                }
                if reg != vreg {
                    self.emit(Inst::iabc(Op::Move, reg, vreg, 0, false));
                }
                Ok(())
            }
            VarKind::Upval(u) => {
                if self.lr().upvals[u as usize].read_only {
                    return Err(self.err(
                        line,
                        format!("attempt to assign to const variable '{text}'"),
                    ));
                }
                self.emit(Inst::iabc(Op::SetUpval, vreg, u, 0, false));
                Ok(())
            }
            VarKind::Global { .. } => {
                let VarKind::Global { read_only } = self.resolve_global_kind(text, line)? else {
                    unreachable!()
                };
                if read_only {
                    return Err(self.err(
                        line,
                        format!("attempt to assign to const variable '{text}'"),
                    ));
                }
                self.assign_global(text, vreg)
            }
        }
    }

    fn assign_global(&mut self, text: &str, vreg: u32) -> Result<(), SyntaxError> {
        if self.env_is_global() {
            return Err(self.err(
                self.last_line,
                format!("_ENV is global when accessing variable '{text}'"),
            ));
        }
        let c = self.str_const(text.as_bytes());
        match self.resolve_name("_ENV")? {
            VarKind::Upval(u) if c <= 0xFF => {
                self.emit(Inst::iabc(Op::SetTabUp, u, c, vreg, true));
                Ok(())
            }
            VarKind::Local(r) if c <= 0xFF => {
                self.emit(Inst::iabc(Op::SetField, r, c, vreg, true));
                Ok(())
            }
            env => {
                let saved = self.lr().freereg;
                let er = self.reserve(2)?;
                match env {
                    VarKind::Upval(u) => {
                        self.emit(Inst::iabc(Op::GetUpval, er, u, 0, false));
                    }
                    VarKind::Local(r) => {
                        self.emit(Inst::iabc(Op::Move, er, r, 0, false));
                    }
                    VarKind::Global { .. } => unreachable!("_ENV always resolves"),
                }
                self.load_const(er + 1, c);
                self.emit(Inst::iabc(Op::SetTable, er, er + 1, vreg, false));
                self.set_freereg(saved);
                Ok(())
            }
        }
    }

    fn assign_to(&mut self, target: ExprId, vreg: u32) -> Result<(), SyntaxError> {
        match self.ast.expr(target) {
            Expr::Name(n) => {
                let (text, line) = (n.text.clone(), n.line);
                self.assign_name(&text, line, vreg)
            }
            Expr::Index { obj, key } => {
                let (obj, key) = (*obj, *key);
                let saved = self.lr().freereg;
                let oe = self.expr(obj)?;
                let o = self.exp_to_anyreg(oe)?;
                match self.ast.expr(key) {
                    Expr::Str(s) if s.len() <= 255 => {
                        let s = s.clone();
                        let c = self.str_const(&s);
                        if c <= 0xFF {
                            self.emit(Inst::iabc(Op::SetField, o, c, vreg, true));
                        } else {
                            let kr = self.reserve(1)?;
                            self.load_const(kr, c);
                            self.emit(Inst::iabc(Op::SetTable, o, kr, vreg, false));
                        }
                    }
                    Expr::Int(i) if (0..=255).contains(i) => {
                        let c = *i as u32;
                        self.emit(Inst::iabc(Op::SetI, o, c, vreg, false));
                    }
                    _ => {
                        let ke = self.expr(key)?;
                        let k = self.exp_to_anyreg(ke)?;
                        self.emit(Inst::iabc(Op::SetTable, o, k, vreg, false));
                    }
                }
                self.set_freereg(saved);
                Ok(())
            }
            _ => unreachable!("parser validates assignment targets"),
        }
    }

    fn if_stat(
        &mut self,
        arms: &[(ExprId, u32, Block)],
        else_body: Option<&Block>,
    ) -> Result<(), SyntaxError> {
        let mut end_jumps = Vec::new();
        for (i, (cond, then_line, body)) in arms.iter().enumerate() {
            let skip = self.cond_jump_false(*cond)?;
            // PUC 5.2/5.3/5.4 attribute BOTH the TEST and the conditional-skip
            // JMP to the `then` keyword's line, because `luaK_goiftrue`
            // emits them after `checknext(TK_THEN)` has advanced
            // `ls->lastline` past the keyword. The result is that a taken
            // if-arm fires a line-hook event for the `then` line between
            // the condition's last instruction and the body's first
            // (5.2/5.3/5.4 db.lua first `test` baselines {2,3,4,7}). PUC
            // 5.5 reorders luaK_goiftrue so the test/jmp keep the condition
            // line (5.5 db.lua expects {2,4,7}).
            if self.version >= LuaVersion::Lua52 && self.version <= LuaVersion::Lua54 {
                self.l().lines[skip] = *then_line;
                // The TEST (or TestSet) instruction sits one slot before the
                // JMP; the same line attribution applies to it.
                if skip > 0 {
                    self.l().lines[skip - 1] = *then_line;
                }
            }
            self.block_scoped(body)?;
            let is_last = i == arms.len() - 1 && else_body.is_none();
            if !is_last {
                end_jumps.push(self.emit_jump());
            }
            self.patch_to_here(skip)?;
        }
        if let Some(eb) = else_body {
            self.block_scoped(eb)?;
        }
        for j in end_jumps {
            self.patch_to_here(j)?;
        }
        Ok(())
    }

    fn while_stat(&mut self, cond: ExprId, body: &Block) -> Result<(), SyntaxError> {
        let top = self.here();
        let exit = self.cond_jump_false(cond)?;
        self.enter_block(true);
        self.stat_block(body)?;
        if self.block_captured() {
            let floor = self.block_floor();
            self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
        }
        self.jump_back(top)?;
        self.leave_block()?;
        self.patch_to_here(exit)?;
        Ok(())
    }

    fn repeat_stat(&mut self, body: &Block, cond: ExprId) -> Result<(), SyntaxError> {
        let top = self.here();
        self.enter_block(true);
        self.stat_block_inner(body, true)?;
        let e = self.expr(cond)?;
        let saved = self.lr().freereg;
        match e {
            Exp::Cmp { op, l, r } => {
                self.emit(Inst::iabc(op, l, r, 0, false));
            }
            e => {
                let r = self.exp_to_anyreg(e)?;
                self.emit(Inst::iabc(Op::Test, r, 0, 0, false));
            }
        }
        self.set_freereg(saved);
        // The condition test above (k = false) lets the *following* jump run
        // when the condition is FALSE (loop again) and skips it when TRUE
        // (exit). With no captured body local we just jump straight back. When
        // a body local is captured, the loop-back path must first CLOSE its
        // upvalues, and the normal-exit path must jump over that close-and-loop
        // tail (PUC `repeatstat`): emitting the CLOSE inline between the test
        // and the back-jump would only skip the CLOSE on exit, not the jump —
        // and so loop forever.
        if self.block_captured() {
            let floor = self.block_floor();
            let cont = self.emit_jump(); // cond FALSE -> close & loop
            let exit = self.emit_jump(); // cond TRUE  -> normal exit
            self.patch_to_here(cont)?;
            self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
            self.jump_back(top)?;
            self.patch_to_here(exit)?;
        } else {
            self.jump_back(top)?;
        }
        self.leave_block()?;
        Ok(())
    }

    fn numeric_for(
        &mut self,
        var: &str,
        line: u32,
        start: ExprId,
        limit: ExprId,
        step: Option<ExprId>,
        body: &Block,
    ) -> Result<(), SyntaxError> {
        self.last_line = line;
        let base = self.lr().freereg;
        self.set_freereg(base);
        let se = self.expr(start)?;
        self.set_freereg(base);
        let s0 = self.exp_to_nextreg(se)?;
        debug_assert_eq!(s0, base);
        let le = self.expr(limit)?;
        self.set_freereg(base + 1);
        let l0 = self.exp_to_nextreg(le)?;
        debug_assert_eq!(l0, base + 1);
        match step {
            Some(st) => {
                let ste = self.expr(st)?;
                self.set_freereg(base + 2);
                let st0 = self.exp_to_nextreg(ste)?;
                debug_assert_eq!(st0, base + 2);
            }
            None => {
                self.set_freereg(base + 2);
                self.reserve(1)?;
                self.emit(Inst::iasbx(Op::LoadI, base + 2, 1));
            }
        }
        self.set_freereg(base + 3);
        self.enter_block(true);
        let var_reg = self.reserve(1)?;
        self.declare_local(var, var_reg, self.version >= LuaVersion::Lua55)?;
        self.last_line = line;
        let prep = self.emit(Inst::iabx(Op::ForPrep, base, 0));
        let body_top = self.here();
        self.stat_block(body)?;
        if self.block_captured() {
            self.emit(Inst::iabc(Op::Close, var_reg, 0, 0, false));
        }
        let loop_pc = self.here();
        let back = loop_pc - body_top + 1;
        if back as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        // PUC attributes FORLOOP (the per-iteration back-edge) to the `for` line,
        // so each loop iteration re-fires a line event there.
        self.last_line = line;
        self.emit(Inst::iabx(Op::ForLoop, base, back as u32));
        let skip = self.here() - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.l().code[prep] = Inst::iabx(Op::ForPrep, base, skip as u32);
        // ForLoop's back-edge lands at `body_top`; ForPrep's forward-skip
        // lands at the post-loop pc (= `here()` after the ForLoop emit).
        self.mark_target(body_top);
        let post_loop = self.here();
        self.mark_target(post_loop);
        self.leave_block()?;
        self.set_freereg(base);
        Ok(())
    }

    fn generic_for(
        &mut self,
        vars: &[ast::Name],
        exprs: &[ExprId],
        body: &Block,
        expr_line: u32,
    ) -> Result<(), SyntaxError> {
        let line = vars[0].line;
        self.last_line = line;
        // control slots: iterator, state, control, closing (<close>: slice 5)
        let base = self.explist_adjust(exprs, 4)?;
        self.set_freereg(base + 4);
        let control_start = self.here() as u32;
        self.enter_block(true);
        // the 4th control value is an implicit to-be-closed variable (5.4+);
        // a `return f()` in the body must not be a tail call, *and* a `goto`
        // leaving this block must close the iterator's closing value via a
        // trampoline (locals.lua:1219 nested-for goto regression).
        {
            let b = self.l().blocks.last_mut().expect("no block");
            b.tbc_scope = true;
            b.has_tbc = true;
        }
        let nvars = vars.len() as u32;
        let vbase = self.reserve(nvars)?;
        debug_assert_eq!(vbase, base + 4);
        for (i, v) in vars.iter().enumerate() {
            // 5.5: the control (first) variable is read-only
            self.declare_local(
                &v.text,
                vbase + i as u32,
                i == 0 && self.version >= LuaVersion::Lua55,
            )?;
        }
        let prep = self.emit(Inst::iabx(Op::TForPrep, base, 0));
        let body_top = self.here();
        self.stat_block(body)?;
        if self.block_captured() {
            self.emit(Inst::iabc(Op::Close, vbase, 0, 0, false));
        }
        let tforcall_pc = self.here();
        let skip = tforcall_pc - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.l().code[prep] = Inst::iabx(Op::TForPrep, base, skip as u32);
        // TForPrep's forward-skip lands at `tforcall_pc` (the upcoming TForCall
        // emit position). Mark before the TForCall emit advances `here()`.
        self.mark_target(tforcall_pc);
        // PUC `forbody` fixes TFORCALL/TFORLOOP to the line of the first token
        // after `in` (the EXPR's source line). A non-callable iterator
        // (`for k,v in 3 do ...`) then raises on the EXPR's line, not the
        // `for` line (errors.lua :428/:429).
        self.last_line = expr_line;
        self.emit(Inst::iabc(Op::TForCall, base, 0, nvars, false));
        let back = self.here() - body_top + 1;
        if back as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.emit(Inst::iabx(Op::TForLoop, base, back as u32));
        // TForLoop's back-edge lands at `body_top` (per-iteration restart).
        self.mark_target(body_top);
        // Override the body block's reg_floor to `base` so trampoline OP_Close
        // emitted for a `goto` leaving the loop closes the iterator's closing
        // value at `base + 3` (which sits BELOW the for-body's user-locals
        // floor `base + 4`). PUC's lparser does the same via `leavelevel` to
        // `f->level + 4` minus the to-be-closed control width.
        self.l().blocks.last_mut().expect("no block").reg_floor = base;
        self.leave_block()?;
        // close the iterator's closing value (4th control slot, 5.4+)
        self.emit(Inst::iabc(Op::Close, base, 0, 0, false));
        // PUC forlist registers three hidden control variables named
        // "(for state)" (generator, state, to-be-closed); debug.getlocal must
        // see them. They live across the loop body.
        let end_pc = self.here() as u32;
        // PUC 5.4 names ALL four control slots "(for state)" — generator,
        // state, control, and to-be-closed; 5.5 dropped the user-control
        // entry so only three are reported. 5.4 files.lua :443 expects the
        // to-be-closed at the 4th "(for state)" hit; 5.5 files.lua :433
        // expects it at the 3rd. Without the user-control entry on 5.4 the
        // file never gets closed on `break`.
        let regs: &[u32] = if self.version >= LuaVersion::Lua55 {
            &[base, base + 1, base + 3]
        } else {
            &[base, base + 1, base + 2, base + 3]
        };
        for &reg in regs {
            self.l().locvars.push(crate::runtime::LocVar {
                name: "(for state)".into(),
                reg,
                start_pc: control_start,
                end_pc,
            });
        }
        self.set_freereg(base);
        Ok(())
    }

    fn return_stat(&mut self, exprs: &[ExprId]) -> Result<(), SyntaxError> {
        match exprs.len() {
            0 => {
                self.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
            }
            1 => {
                // tail call: `return f(...)` (not parenthesized), but NOT in
                // the scope of a to-be-closed variable — the function must
                // return so its __close handlers run (PUC suppresses tail
                // calls inside tbc scope)
                let in_tbc = self.lr().blocks.iter().any(|b| b.tbc_scope);
                let is_call = matches!(
                    self.ast.expr(exprs[0]),
                    Expr::Call { .. } | Expr::MethodCall { .. }
                );
                if is_call {
                    let base = self.lr().freereg;
                    let e = self.call_expr(exprs[0])?;
                    let Exp::Open { pc, base: cb } = e else {
                        unreachable!()
                    };
                    debug_assert_eq!(cb, base);
                    if in_tbc {
                        // suppressed tail call: ordinary call returning all
                        // results, so __close handlers can run on RETURN
                        self.patch_wanted(pc, 0);
                        self.emit(Inst::iabc(Op::Return, base, 0, 0, false));
                    } else {
                        let call = self.l().code[pc];
                        self.l().code[pc] = Inst::iabc(Op::TailCall, call.a(), call.b(), 0, false);
                        // Fallback Return for the TailCall→native case: PUC
                        // never actually tail-calls a C function (the C frame
                        // has no Lua activation to fold into), so `OP_TAILCALL`
                        // there runs the native under the current Lua frame and
                        // returns the native's results to the caller. luna's
                        // `Op::TailCall` keeps the frame for native targets, so
                        // after the native completes (or yields-then-resumes)
                        // the run loop needs an explicit Return to forward the
                        // results — the Lua-target path pops the frame and so
                        // never reaches this op.
                        self.emit(Inst::iabc(Op::Return, base, 0, 0, false));
                    }
                    self.set_freereg(base);
                    return Ok(());
                }
                if matches!(self.ast.expr(exprs[0]), Expr::Vararg) {
                    let base = self.lr().freereg;
                    let e = self.expr(exprs[0])?;
                    let Exp::Open { pc, .. } = e else {
                        unreachable!()
                    };
                    self.patch_wanted(pc, 0);
                    self.emit(Inst::iabc(Op::Return, base, 0, 0, false));
                    self.set_freereg(base);
                    return Ok(());
                }
                let e = self.expr(exprs[0])?;
                let saved = self.lr().freereg;
                let r = self.exp_to_anyreg(e)?;
                self.set_freereg(saved);
                self.emit(Inst::iabc(Op::Return1, r, 0, 0, false));
            }
            n if n > 254 => {
                // PUC `OP_RETURN`'s B field is a byte (`b = nret + 1`), so the
                // statement supports at most 254 fixed return values
                // (calls.lua :573). Report the limit explicitly rather than
                // letting it surface as the generic register-pressure error.
                return Err(self.err(self.last_line, "too many returns"));
            }
            n => {
                let base = self.lr().freereg;
                let mut open = false;
                for (i, &eid) in exprs.iter().enumerate() {
                    let dst = base + i as u32;
                    self.set_freereg(dst);
                    let e = self.expr(eid)?;
                    if i == n - 1
                        && let Exp::Open { pc, base: ob } = e
                    {
                        debug_assert_eq!(ob, dst);
                        self.patch_wanted(pc, 0);
                        open = true;
                        break;
                    }
                    self.set_freereg(dst);
                    let got = self.exp_to_nextreg(e)?;
                    debug_assert_eq!(got, dst);
                }
                let b = if open { 0 } else { n as u32 + 1 };
                self.emit(Inst::iabc(Op::Return, base, b, 0, false));
                self.set_freereg(base);
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // v2.1 Phase 11 — A4' prerequisite: compiler-side snapshot gate.
    // -----------------------------------------------------------------

    /// Compiler-side metamethod-safety gate for the future A4' Index-LHS
    /// object snapshot elision attack (see
    /// `.dev/rfcs/v2.0-pi-phase11-a4-prime-rfc.md` §2.3 and
    /// `.dev/rfcs/v2.1-a4-prime-prereq-verdict.md`).
    ///
    /// Returns `true` when, for a single-target Index-LHS assignment
    /// `obj.key = rhs` (or `obj[key] = rhs`), the unconditional
    /// `exp_to_nextreg(oe)` snapshot at `assign_stat` line 2490 is
    /// provably redundant.
    ///
    /// The four conditions enforced (mirroring RFC §2.3):
    ///
    /// 1. `targets.len() == 1` and `exprs.len() == 1` — no inter-target
    ///    or multi-RHS conflict possible.
    /// 2. The single target is `Expr::Index { obj: Name(local), .. }`
    ///    where the name resolves to a real local in the current level
    ///    (not an upvalue / global / read-only / vararg-virtual).
    /// 3. `locals[reg].captured == false` — no closure has captured
    ///    this local's slot, so no metatable-stored Lua closure can
    ///    rebind it through the upvalue.
    /// 4. AST-side
    ///    [`ast::metamethod_safe_for_index_lhs`][crate::frontend::ast::metamethod_safe_for_index_lhs]
    ///    over `(obj, exprs[0])` returns true (no UserOrUnknown RHS
    ///    calls; obj is a bare Name).
    ///
    /// Wired by the A4' attack at `assign_stat` line 2487-2522
    /// Index-LHS branch (v2.1 PI Phase 11 ship).
    pub(crate) fn assign_stat_can_skip_obj_snapshot(
        &self,
        targets: &[ExprId],
        exprs: &[ExprId],
    ) -> bool {
        if targets.len() != 1 || exprs.len() != 1 {
            return false;
        }
        let (obj_eid, _key_eid) = match self.ast.expr(targets[0]) {
            Expr::Index { obj, key } => (*obj, *key),
            _ => return false,
        };
        let name_text = match self.ast.expr(obj_eid) {
            Expr::Name(n) => &*n.text,
            _ => return false,
        };
        // Resolve the name against the *current* level only — we
        // intentionally do not chase upvalues here because A4' only
        // elides snapshots for owner-level locals.
        let level = self.lr();
        let local = match level.locals.iter().find(|l| &*l.name == name_text) {
            Some(l) => l,
            None => return false,
        };
        if local.captured || local.vararg_virtual {
            return false;
        }
        // AST-side gate (call walker + obj-is-name check).
        ast::metamethod_safe_for_index_lhs(self.ast, obj_eid, exprs[0])
    }
}

/// Constant-fold arithmetic over two numeric literals where Lua semantics
/// are total (no division-by-zero style runtime errors).
fn fold_arith(op: BinOp, le: &Exp, ast: &Chunk, rhs: ExprId) -> Option<Exp> {
    let l = match le {
        Exp::Int(i) => Num::Int(*i),
        Exp::Float(f) => Num::Float(*f),
        _ => return None,
    };
    let r = match ast.expr(rhs) {
        Expr::Int(i) => Num::Int(*i),
        Expr::Float(f) => Num::Float(*f),
        _ => return None,
    };
    use Num::*;
    let v = match (op, l, r) {
        (BinOp::Add, Int(a), Int(b)) => Int(a.wrapping_add(b)),
        (BinOp::Sub, Int(a), Int(b)) => Int(a.wrapping_sub(b)),
        (BinOp::Mul, Int(a), Int(b)) => Int(a.wrapping_mul(b)),
        (BinOp::Add, a, b) => Float(a.as_f64() + b.as_f64()),
        (BinOp::Sub, a, b) => Float(a.as_f64() - b.as_f64()),
        (BinOp::Mul, a, b) => Float(a.as_f64() * b.as_f64()),
        (BinOp::Div, a, b) => Float(a.as_f64() / b.as_f64()),
        _ => return None,
    };
    Some(match v {
        Int(i) => Exp::Int(i),
        Float(f) => Exp::Float(f),
    })
}
