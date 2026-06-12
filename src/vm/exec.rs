//! The interpreter. Dispatch is a plain match over opcodes (the P10 ceiling
//! pass owns dispatch optimization). Lua→Lua calls share one loop and never
//! recurse the Rust stack; only native↔Lua boundaries do (e.g. pcall).
//!
//! Varargs follow 5.5 semantics: a vararg call materializes a vararg table
//! (fields 1..n plus "n") kept in the function's own stack slot; `...`
//! expands from it and `...name` binds it. Stack-spread varargs for the
//! 5.1/5.4 compat modes arrive in P08.

use crate::compiler::compile_chunk;
use crate::frontend::{SyntaxError, parse};
use crate::numeric::{self, Num};
use crate::runtime::heap::GcHeader;
use crate::runtime::{Gc, Heap, LuaClosure, Table, TableError, UpvalState, Upvalue, Value};
use crate::version::LuaVersion;
use crate::vm::error::LuaError;
use crate::vm::isa::{Inst, Op};

pub struct Vm {
    pub heap: Heap,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    /// open upvalues, sorted ascending by stack slot
    open_upvals: Vec<(u32, Gc<Upvalue>)>,
    /// to-be-closed slots, ascending
    tbc: Vec<u32>,
    /// logical stack top for multi-result sequences
    top: u32,
    globals: Gc<Table>,
    /// shared metatable for all strings (populated by the string lib, P04)
    string_mt: Option<Gc<Table>>,
    /// pre-interned metamethod event names, indexed by `Mm`
    mm_names: Vec<Gc<crate::runtime::LuaStr>>,
    /// native↔Lua nesting depth (PUC C-stack guard analogue)
    c_depth: u32,
    /// xoshiro256** state (math.random)
    rng: [u64; 4],
    /// VM creation time (os.clock)
    started: std::time::Instant,
    version: LuaVersion,
}

/// Metamethod events; discriminants index `Vm::mm_names`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum Mm {
    Index,
    NewIndex,
    Call,
    ToString,
    Metatable,
    Name,
    Eq,
    Lt,
    Le,
    Concat,
    Len,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    IDiv,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
    Unm,
    BNot,
    Close,
}

const MM_NAMES: [&str; 26] = [
    "__index",
    "__newindex",
    "__call",
    "__tostring",
    "__metatable",
    "__name",
    "__eq",
    "__lt",
    "__le",
    "__concat",
    "__len",
    "__add",
    "__sub",
    "__mul",
    "__div",
    "__mod",
    "__pow",
    "__idiv",
    "__band",
    "__bor",
    "__bxor",
    "__shl",
    "__shr",
    "__unm",
    "__bnot",
    "__close",
];

/// PUC MAXTAGLOOP: bound on `__index`/`__newindex` chains.
const MAX_TAG_LOOP: u32 = 2000;
/// PUC LUAI_MAXCCALLS analogue: native↔Lua nesting bound.
const MAX_C_DEPTH: u32 = 200;
/// PUC LUAI_MAXSTACK analogue: total VM stack slots.
const MAX_LUA_STACK: u32 = 1 << 20;

#[derive(Clone, Copy)]
struct Frame {
    closure: Gc<LuaClosure>,
    /// stack index of register 0
    base: u32,
    pc: u32,
    /// stack slot of the function (results land here; vararg table lives here)
    func_slot: u32,
    /// results expected by the caller (-1 = all)
    nresults: i32,
}

#[derive(Debug)]
pub enum Error {
    Syntax(SyntaxError),
    Runtime(LuaError),
}

impl From<SyntaxError> for Error {
    fn from(e: SyntaxError) -> Error {
        Error::Syntax(e)
    }
}

impl From<LuaError> for Error {
    fn from(e: LuaError) -> Error {
        Error::Runtime(e)
    }
}

impl Vm {
    pub fn new(version: LuaVersion) -> Vm {
        let mut heap = Heap::new();
        let globals = heap.new_table();
        let mm_names = MM_NAMES.iter().map(|n| heap.intern(n.as_bytes())).collect();
        let mut vm = Vm {
            heap,
            stack: Vec::new(),
            frames: Vec::new(),
            open_upvals: Vec::new(),
            tbc: Vec::new(),
            top: 0,
            globals,
            string_mt: None,
            mm_names,
            c_depth: 0,
            rng: [0; 4],
            started: std::time::Instant::now(),
            version,
        };
        let (a, b) = vm.rng_auto_seed();
        vm.rng_seed(a as u64, b as u64);
        crate::vm::builtins::open_base(&mut vm);
        crate::vm::lib_math::open_math(&mut vm);
        crate::vm::lib_table::open_table(&mut vm);
        crate::vm::lib_string::open_string(&mut vm);
        crate::vm::lib_utf8::open_utf8(&mut vm);
        crate::vm::lib_os_io::open_os_io(&mut vm);
        crate::vm::lib_debug::open_debug(&mut vm);
        crate::vm::lib_os_io::open_package(&mut vm);
        vm
    }

    /// xoshiro256** next.
    pub(crate) fn rng_next(&mut self) -> u64 {
        let s = &mut self.rng;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Seed the RNG via splitmix64 expansion (PUC randseed shape).
    pub(crate) fn rng_seed(&mut self, a: u64, b: u64) {
        let mut sm = a ^ b.rotate_left(32);
        let mut next = move || {
            sm = sm.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = sm;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        self.rng = [next(), next(), next(), next()];
        for _ in 0..16 {
            self.rng_next();
        }
    }

    /// Wall-clock since VM creation (os.clock approximation).
    pub(crate) fn uptime(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// Entropy for math.randomseed() with no arguments.
    pub(crate) fn rng_auto_seed(&mut self) -> (i64, i64) {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let addr = &self.rng as *const _ as u64;
        (t as i64, addr as i64)
    }

    /// Allocate a native function object (no upvalues): builtin registration.
    pub fn native(&mut self, f: crate::runtime::value::NativeFn) -> Value {
        Value::Native(self.heap.new_native(f, Box::new([])))
    }

    /// Allocate a native function object with captured upvalues.
    pub fn native_with(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: Box<[Value]>,
    ) -> Value {
        Value::Native(self.heap.new_native(f, upvals))
    }

    /// Install the shared string metatable (string library, P04).
    pub fn set_string_metatable(&mut self, mt: Option<Gc<Table>>) {
        self.string_mt = mt;
    }

    pub fn globals(&self) -> Gc<Table> {
        self.globals
    }

    pub fn version(&self) -> LuaVersion {
        self.version
    }

    pub fn set_global(&mut self, name: &str, v: Value) {
        let k = Value::Str(self.heap.intern(name.as_bytes()));
        unsafe { self.globals.as_mut() }
            .set(k, v)
            .expect("global name is a valid key");
    }

    /// Parse + compile a chunk and close it over the globals table.
    pub fn load(&mut self, src: &[u8], chunkname: &[u8]) -> Result<Gc<LuaClosure>, SyntaxError> {
        let ast = parse(src, self.version)?;
        let proto = compile_chunk(&ast, self.version, chunkname, &mut self.heap)?;
        let env = self
            .heap
            .new_upvalue(UpvalState::Closed(Value::Table(self.globals)));
        Ok(self.heap.new_closure(proto, Box::new([env])))
    }

    /// Convenience: load + run, returning the chunk's results.
    pub fn eval(&mut self, src: &str) -> Result<Vec<Value>, Error> {
        let cl = self.load(src.as_bytes(), b"=eval")?;
        Ok(self.call_value(Value::Closure(cl), &[])?)
    }

    /// Render an error value for messages/tests.
    pub fn error_text(&self, e: &LuaError) -> String {
        match e.0 {
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            v => format!("(error object is a {} value)", v.type_name()),
        }
    }

    /// Call any callable value from the host (or from natives like pcall).
    pub fn call_value(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        if self.c_depth >= MAX_C_DEPTH {
            return Err(self.rt_err("stack overflow"));
        }
        self.c_depth += 1;
        let func_slot = self.stack.len() as u32;
        self.stack.push(f);
        self.stack.extend_from_slice(args);
        self.top = self.stack.len() as u32;
        let r = self.call_at(func_slot, args.len() as u32);
        self.c_depth -= 1;
        if r.is_err() {
            self.stack.truncate(func_slot as usize);
            self.top = func_slot;
        }
        r
    }

    /// Call a metamethod with a single expected result.
    fn call_mm1(&mut self, f: Value, args: &[Value]) -> Result<Value, LuaError> {
        let mut r = self.call_value(f, args)?;
        Ok(if r.is_empty() {
            Value::Nil
        } else {
            r.swap_remove(0)
        })
    }

    // ---- metatables ----

    pub(crate) fn metatable_of(&self, v: Value) -> Option<Gc<Table>> {
        match v {
            Value::Table(t) => t.metatable(),
            Value::Str(_) => self.string_mt,
            _ => None,
        }
    }

    /// The metamethod of `v` for `mm`, or nil.
    pub(crate) fn get_mm(&self, v: Value, mm: Mm) -> Value {
        match self.metatable_of(v) {
            Some(mt) => mt.get(Value::Str(self.mm_names[mm as usize])),
            None => Value::Nil,
        }
    }

    fn call_at(&mut self, func_slot: u32, nargs: u32) -> Result<Vec<Value>, LuaError> {
        if self.begin_call(func_slot, Some(nargs), -1)? {
            self.exec()
        } else {
            // native completed inline; results at func_slot..top
            Ok(self.take_results(func_slot))
        }
    }

    /// Run a full collection with the VM's roots.
    pub fn collect_garbage(&mut self) -> usize {
        let mut roots: Vec<Value> = Vec::with_capacity(self.stack.len() + 32);
        roots.push(Value::Table(self.globals));
        if let Some(mt) = self.string_mt {
            roots.push(Value::Table(mt));
        }
        for &n in &self.mm_names {
            roots.push(Value::Str(n));
        }
        roots.extend_from_slice(&self.stack);
        for f in &self.frames {
            roots.push(Value::Closure(f.closure));
        }
        let extra: Vec<*mut GcHeader> = self
            .open_upvals
            .iter()
            .map(|&(_, uv)| uv.as_ptr() as *mut GcHeader)
            .collect();
        self.heap.collect_ex(&roots, &extra)
    }

    // ---- frames & calls ----

    /// Begin calling stack[func_slot] with `nargs` (None: up to self.top).
    /// Returns true if a Lua frame was pushed (the dispatch loop continues
    /// there), false if a native completed inline.
    fn begin_call(
        &mut self,
        func_slot: u32,
        nargs: Option<u32>,
        nresults: i32,
    ) -> Result<bool, LuaError> {
        let nargs = match nargs {
            Some(n) => n,
            None => self.top - (func_slot + 1),
        };
        match self.stack[func_slot as usize] {
            Value::Closure(cl) => {
                self.push_frame(cl, func_slot, nargs, nresults)?;
                Ok(true)
            }
            Value::Native(nc) => {
                let nret = (nc.f)(self, func_slot, nargs)?;
                self.finish_results(func_slot, nret, nresults);
                Ok(false)
            }
            v => {
                // __call: insert the handler before the original value so it
                // becomes the first argument (single level, PUC tryfuncTM).
                // Slots above shift by one; at a call site those are dead
                // temps of the current frame.
                let mm = self.get_mm(v, Mm::Call);
                if mm.is_nil() || !matches!(mm, Value::Closure(_) | Value::Native(_)) {
                    return Err(self.type_err("call", v));
                }
                self.stack.insert(func_slot as usize, mm);
                if self.top > func_slot {
                    self.top += 1;
                }
                self.begin_call_inner(mm, func_slot, nargs + 1, nresults)
            }
        }
    }

    fn begin_call_inner(
        &mut self,
        f: Value,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> Result<bool, LuaError> {
        match f {
            Value::Closure(cl) => {
                self.push_frame(cl, func_slot, nargs, nresults)?;
                Ok(true)
            }
            Value::Native(nc) => {
                let nret = (nc.f)(self, func_slot, nargs)?;
                self.finish_results(func_slot, nret, nresults);
                Ok(false)
            }
            _ => unreachable!(),
        }
    }

    fn push_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> Result<(), LuaError> {
        if func_slot + 256 > MAX_LUA_STACK {
            return Err(self.rt_err("stack overflow"));
        }
        let proto = cl.proto;
        let nparams = proto.num_params as u32;
        if proto.is_vararg {
            // 5.5: collect extras into the vararg table, stored in the
            // function's own slot (the Frame keeps the closure)
            let nextra = nargs.saturating_sub(nparams);
            let t = self.heap.new_table();
            {
                let tm = unsafe { t.as_mut() };
                for i in 0..nextra {
                    tm.set_int(
                        i as i64 + 1,
                        self.stack[(func_slot + 1 + nparams + i) as usize],
                    );
                }
            }
            let n_key = Value::Str(self.heap.intern(b"n"));
            unsafe { t.as_mut() }
                .set(n_key, Value::Int(nextra as i64))
                .expect("'n' is a valid key");
            self.stack[func_slot as usize] = Value::Table(t);
        }
        let base = func_slot + 1;
        let need = (base + proto.max_stack as u32) as usize;
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
        // wipe the register window beyond the kept parameters (drops extra
        // args and stale values — required for GC-safety and codegen)
        let kept = nargs.min(nparams);
        for i in (base + kept) as usize..need {
            self.stack[i] = Value::Nil;
        }
        self.frames.push(Frame {
            closure: cl,
            base,
            pc: 0,
            func_slot,
            nresults,
        });
        Ok(())
    }

    /// Pad/announce results sitting at func_slot.
    fn finish_results(&mut self, func_slot: u32, nret: u32, wanted: i32) {
        if wanted < 0 {
            self.top = func_slot + nret;
        } else {
            let wanted = wanted as u32;
            let need = (func_slot + wanted) as usize;
            if self.stack.len() < need {
                self.stack.resize(need, Value::Nil);
            }
            for i in nret..wanted {
                self.stack[(func_slot + i) as usize] = Value::Nil;
            }
            self.top = func_slot + wanted;
        }
    }

    fn take_results(&mut self, func_slot: u32) -> Vec<Value> {
        let nret = self.top - func_slot;
        let out = self.stack[func_slot as usize..(func_slot + nret) as usize].to_vec();
        self.stack.truncate(func_slot as usize);
        self.top = func_slot;
        out
    }

    // ---- open upvalues ----

    fn find_or_create_upval(&mut self, slot: u32) -> Gc<Upvalue> {
        match self.open_upvals.binary_search_by_key(&slot, |&(s, _)| s) {
            Ok(i) => self.open_upvals[i].1,
            Err(i) => {
                let uv = self.heap.new_upvalue(UpvalState::Open(slot));
                self.open_upvals.insert(i, (slot, uv));
                uv
            }
        }
    }

    fn close_from(&mut self, slot: u32) {
        while let Some(&(s, uv)) = self.open_upvals.last() {
            if s < slot {
                break;
            }
            let v = self.stack[s as usize];
            unsafe { uv.as_mut() }.set_closed(v);
            self.open_upvals.pop();
        }
    }

    /// Register a to-be-closed slot (TBC op / generic-for closing value).
    fn register_tbc(&mut self, slot: u32) -> Result<(), LuaError> {
        let v = self.stack[slot as usize];
        if matches!(v, Value::Nil | Value::Bool(false)) {
            return Ok(()); // nil and false are silently ignored
        }
        if self.get_mm(v, Mm::Close).is_nil() {
            return Err(self.rt_err(&format!(
                "variable of a to-be-closed slot has a non-closable value (a {} value)",
                v.type_name()
            )));
        }
        debug_assert!(self.tbc.last().is_none_or(|&s| s < slot));
        self.tbc.push(slot);
        Ok(())
    }

    /// Close upvalues and run `__close` handlers for slots ≥ `from`
    /// (handlers in reverse registration order; PUC luaF_close).
    fn close_slots(&mut self, from: u32, err: Option<Value>) -> Result<(), LuaError> {
        self.close_from(from);
        while let Some(&s) = self.tbc.last() {
            if s < from {
                break;
            }
            self.tbc.pop();
            let v = self.stack[s as usize];
            if matches!(v, Value::Nil | Value::Bool(false)) {
                continue;
            }
            let mm = self.get_mm(v, Mm::Close);
            if !mm.is_nil() {
                self.call_value(mm, &[v, err.unwrap_or(Value::Nil)])?;
            }
        }
        Ok(())
    }

    fn upval_get(&self, cl: Gc<LuaClosure>, idx: u32) -> Value {
        match cl.upvals[idx as usize].state() {
            UpvalState::Open(slot) => self.stack[slot as usize],
            UpvalState::Closed(v) => v,
        }
    }

    fn upval_set(&mut self, cl: Gc<LuaClosure>, idx: u32, v: Value) {
        let uv = cl.upvals[idx as usize];
        match uv.state() {
            UpvalState::Open(slot) => self.stack[slot as usize] = v,
            UpvalState::Closed(_) => unsafe { uv.as_mut() }.set_closed(v),
        }
    }

    // ---- register / error helpers ----

    #[inline(always)]
    fn r(&self, base: u32, i: u32) -> Value {
        self.stack[(base + i) as usize]
    }

    #[inline(always)]
    fn set_r(&mut self, base: u32, i: u32, v: Value) {
        self.stack[(base + i) as usize] = v;
    }

    pub(crate) fn rt_err(&mut self, msg: &str) -> LuaError {
        let text = match self.position_prefix() {
            Some(p) => format!("{p}{msg}"),
            None => msg.to_string(),
        };
        LuaError(Value::Str(self.heap.intern(text.as_bytes())))
    }

    pub(crate) fn type_err(&mut self, what: &str, v: Value) -> LuaError {
        self.rt_err(&format!("attempt to {what} a {} value", v.type_name()))
    }

    /// Position prefix of the currently executing Lua frame.
    pub(crate) fn position_prefix(&self) -> Option<String> {
        let f = self.frames.last()?;
        let proto = f.closure.proto;
        let line = proto.lines[(f.pc as usize).saturating_sub(1).min(proto.lines.len() - 1)];
        let src = String::from_utf8_lossy(chunk_display_name(proto.source.as_ptr())).into_owned();
        Some(format!("{src}:{line}: "))
    }

    // ---- the interpreter ----

    fn exec(&mut self) -> Result<Vec<Value>, LuaError> {
        let entry_depth = self.frames.len();
        let mut r = self.run(entry_depth);
        if let Err(ref e) = r {
            // unwind the frames this activation created; __close handlers
            // see the error object, and an error in a handler replaces it
            let mut err = *e;
            while self.frames.len() >= entry_depth {
                let f = self.frames.pop().expect("frame");
                if let Err(e2) = self.close_slots(f.base, Some(err.0)) {
                    err = e2;
                }
                self.stack.truncate(f.func_slot as usize);
                self.top = f.func_slot;
                self.tbc.retain(|&s| s < f.func_slot);
            }
            r = Err(err);
        }
        r
    }

    fn run(&mut self, entry_depth: usize) -> Result<Vec<Value>, LuaError> {
        loop {
            let f = self.frames.last().expect("no frame");
            let cl = f.closure;
            let base = f.base;
            let func_slot = f.func_slot;
            let pc = f.pc;
            let inst = cl.proto.code[pc as usize];
            self.frames.last_mut().expect("no frame").pc = pc + 1;

            match inst.op() {
                Op::Move => {
                    let v = self.r(base, inst.b());
                    self.set_r(base, inst.a(), v);
                }
                Op::LoadI => self.set_r(base, inst.a(), Value::Int(inst.sbx() as i64)),
                Op::LoadF => self.set_r(base, inst.a(), Value::Float(inst.sbx() as f64)),
                Op::LoadK => {
                    let v = cl.proto.consts[inst.bx() as usize];
                    self.set_r(base, inst.a(), v);
                }
                Op::LoadKx => {
                    let extra = cl.proto.code[self.pc_of_top() as usize];
                    self.bump_pc();
                    let v = cl.proto.consts[extra.ax() as usize];
                    self.set_r(base, inst.a(), v);
                }
                Op::LoadFalse => self.set_r(base, inst.a(), Value::Bool(false)),
                Op::LFalseSkip => {
                    self.set_r(base, inst.a(), Value::Bool(false));
                    self.bump_pc();
                }
                Op::LoadTrue => self.set_r(base, inst.a(), Value::Bool(true)),
                Op::LoadNil => {
                    let a = inst.a();
                    for i in 0..=inst.b() {
                        self.set_r(base, a + i, Value::Nil);
                    }
                }
                Op::GetUpval => {
                    let v = self.upval_get(cl, inst.b());
                    self.set_r(base, inst.a(), v);
                }
                Op::SetUpval => {
                    let v = self.r(base, inst.a());
                    self.upval_set(cl, inst.b(), v);
                }
                Op::GetTabUp => {
                    let t = self.upval_get(cl, inst.b());
                    let key = cl.proto.consts[inst.c() as usize];
                    let v = self.index_value(t, key)?;
                    self.set_r(base, inst.a(), v);
                }
                Op::GetTable => {
                    let t = self.r(base, inst.b());
                    let key = self.r(base, inst.c());
                    let v = self.index_value(t, key)?;
                    self.set_r(base, inst.a(), v);
                }
                Op::GetI => {
                    let t = self.r(base, inst.b());
                    let v = self.index_value(t, Value::Int(inst.c() as i64))?;
                    self.set_r(base, inst.a(), v);
                }
                Op::GetField => {
                    let t = self.r(base, inst.b());
                    let key = cl.proto.consts[inst.c() as usize];
                    let v = self.index_value(t, key)?;
                    self.set_r(base, inst.a(), v);
                }
                Op::SetTabUp => {
                    let t = self.upval_get(cl, inst.a());
                    let key = cl.proto.consts[inst.b() as usize];
                    let v = self.r(base, inst.c());
                    self.newindex_value(t, key, v)?;
                }
                Op::SetTable => {
                    let t = self.r(base, inst.a());
                    let key = self.r(base, inst.b());
                    let v = self.r(base, inst.c());
                    self.newindex_value(t, key, v)?;
                }
                Op::SetI => {
                    let t = self.r(base, inst.a());
                    let v = self.r(base, inst.c());
                    self.newindex_value(t, Value::Int(inst.b() as i64), v)?;
                }
                Op::SetField => {
                    let t = self.r(base, inst.a());
                    let key = cl.proto.consts[inst.b() as usize];
                    let v = self.r(base, inst.c());
                    self.newindex_value(t, key, v)?;
                }
                Op::NewTable => {
                    let t = self.heap.new_table();
                    self.set_r(base, inst.a(), Value::Table(t));
                }
                Op::SetList => {
                    let a = inst.a();
                    let abs_a = base + a;
                    let n = if inst.b() == 0 {
                        self.top - (abs_a + 1)
                    } else {
                        inst.b()
                    };
                    let offset = if inst.k() {
                        let extra = cl.proto.code[self.pc_of_top() as usize];
                        self.bump_pc();
                        extra.ax() as i64
                    } else {
                        inst.c() as i64
                    };
                    let Value::Table(t) = self.r(base, a) else {
                        unreachable!("SETLIST on non-table");
                    };
                    for i in 1..=n {
                        let v = self.r(base, a + i);
                        unsafe { t.as_mut() }.set_int(offset + i as i64, v);
                    }
                }
                Op::SelfOp => {
                    let o = self.r(base, inst.b());
                    self.set_r(base, inst.a() + 1, o);
                    let key = cl.proto.consts[inst.c() as usize];
                    let m = self.index_value(o, key)?;
                    self.set_r(base, inst.a(), m);
                }
                Op::Add => self.arith_rr(inst, base, ArithOp::Add)?,
                Op::Sub => self.arith_rr(inst, base, ArithOp::Sub)?,
                Op::Mul => self.arith_rr(inst, base, ArithOp::Mul)?,
                Op::Mod => self.arith_rr(inst, base, ArithOp::Mod)?,
                Op::Pow => self.arith_rr(inst, base, ArithOp::Pow)?,
                Op::Div => self.arith_rr(inst, base, ArithOp::Div)?,
                Op::IDiv => self.arith_rr(inst, base, ArithOp::IDiv)?,
                Op::BAnd => self.arith_rr(inst, base, ArithOp::BAnd)?,
                Op::BOr => self.arith_rr(inst, base, ArithOp::BOr)?,
                Op::BXor => self.arith_rr(inst, base, ArithOp::BXor)?,
                Op::Shl => self.arith_rr(inst, base, ArithOp::Shl)?,
                Op::Shr => self.arith_rr(inst, base, ArithOp::Shr)?,
                Op::Unm => {
                    let v = self.r(base, inst.b());
                    let r = match coerce_num(v) {
                        Some(Num::Int(i)) => Value::Int(i.wrapping_neg()),
                        Some(Num::Float(f)) => Value::Float(-f),
                        None => {
                            let mm = self.get_mm(v, Mm::Unm);
                            if mm.is_nil() {
                                return Err(self.type_err("perform arithmetic on", v));
                            }
                            self.call_mm1(mm, &[v, v])?
                        }
                    };
                    self.set_r(base, inst.a(), r);
                }
                Op::BNot => {
                    let v = self.r(base, inst.b());
                    let r = match coerce_num(v) {
                        Some(n) => {
                            let i = self.int_from_num(n)?;
                            Value::Int(!i)
                        }
                        None => {
                            let mm = self.get_mm(v, Mm::BNot);
                            if mm.is_nil() {
                                return Err(self.type_err("perform bitwise operation on", v));
                            }
                            self.call_mm1(mm, &[v, v])?
                        }
                    };
                    self.set_r(base, inst.a(), r);
                }
                Op::Not => {
                    let v = self.r(base, inst.b());
                    self.set_r(base, inst.a(), Value::Bool(!v.truthy()));
                }
                Op::Len => {
                    let v = self.r(base, inst.b());
                    let r = match v {
                        Value::Str(s) => Value::Int(s.len() as i64),
                        Value::Table(_) => {
                            let mm = self.get_mm(v, Mm::Len);
                            if mm.is_nil() {
                                let Value::Table(t) = v else { unreachable!() };
                                Value::Int(t.len())
                            } else {
                                self.call_mm1(mm, &[v])?
                            }
                        }
                        v => {
                            let mm = self.get_mm(v, Mm::Len);
                            if mm.is_nil() {
                                return Err(self.type_err("get length of", v));
                            }
                            self.call_mm1(mm, &[v])?
                        }
                    };
                    self.set_r(base, inst.a(), r);
                }
                Op::Concat => {
                    // right fold (Lua concat is right-associative)
                    let a = inst.a();
                    let n = inst.b();
                    let mut acc = self.r(base, a + n - 1);
                    for i in (0..n - 1).rev() {
                        let l = self.r(base, a + i);
                        acc = self.concat_values(l, acc)?;
                    }
                    self.set_r(base, a, acc);
                }
                Op::Close => {
                    self.close_slots(base + inst.a(), None)?;
                }
                Op::Tbc => {
                    self.register_tbc(base + inst.a())?;
                }
                Op::Jmp => {
                    self.add_pc(inst.sj());
                }
                Op::Eq => {
                    let l = self.r(base, inst.a());
                    let r = self.r(base, inst.b());
                    let cond = self.eq_value(l, r)?;
                    self.cond_skip(cond, inst.k());
                }
                Op::EqK => {
                    let l = self.r(base, inst.a());
                    let r = cl.proto.consts[inst.b() as usize];
                    let cond = self.eq_value(l, r)?;
                    self.cond_skip(cond, inst.k());
                }
                Op::Lt => {
                    let l = self.r(base, inst.a());
                    let r = self.r(base, inst.b());
                    let cond = self.less_than(l, r, false)?;
                    self.cond_skip(cond, inst.k());
                }
                Op::Le => {
                    let l = self.r(base, inst.a());
                    let r = self.r(base, inst.b());
                    let cond = self.less_than(l, r, true)?;
                    self.cond_skip(cond, inst.k());
                }
                Op::Test => {
                    let cond = self.r(base, inst.a()).truthy();
                    self.cond_skip(cond, inst.k());
                }
                Op::TestSet => {
                    let v = self.r(base, inst.b());
                    if v.truthy() == inst.k() {
                        self.set_r(base, inst.a(), v);
                    } else {
                        self.bump_pc();
                    }
                }
                Op::Call => {
                    let abs = base + inst.a();
                    let nargs = if inst.b() == 0 {
                        None
                    } else {
                        Some(inst.b() - 1)
                    };
                    let wanted = inst.c() as i32 - 1;
                    self.begin_call(abs, nargs, wanted)?;
                }
                Op::TailCall => {
                    let fr = *self.frames.last().expect("no frame");
                    let abs = base + inst.a();
                    let nargs = if inst.b() == 0 {
                        self.top - (abs + 1)
                    } else {
                        inst.b() - 1
                    };
                    self.close_slots(fr.base, None)?;
                    for i in 0..=nargs {
                        self.stack[(fr.func_slot + i) as usize] = self.stack[(abs + i) as usize];
                    }
                    self.frames.pop();
                    if !self.begin_call(fr.func_slot, Some(nargs), fr.nresults)?
                        && self.frames.len() < entry_depth
                    {
                        // a native completed what was this function's result
                        return Ok(self.take_results(fr.func_slot));
                    }
                }
                Op::Return | Op::Return0 | Op::Return1 => {
                    let (abs_a, nret) = match inst.op() {
                        Op::Return0 => (base, 0),
                        Op::Return1 => (base + inst.a(), 1),
                        _ => {
                            let abs_a = base + inst.a();
                            let nret = if inst.b() == 0 {
                                self.top - abs_a
                            } else {
                                inst.b() - 1
                            };
                            (abs_a, nret)
                        }
                    };
                    // close before moving results: __close handlers run above
                    // the stack top, so the result region stays intact
                    self.close_slots(base, None)?;
                    let fr = self.frames.pop().expect("no frame");
                    for i in 0..nret {
                        self.stack[(fr.func_slot + i) as usize] = self.stack[(abs_a + i) as usize];
                    }
                    if self.frames.len() < entry_depth {
                        self.top = fr.func_slot + nret;
                        return Ok(self.take_results(fr.func_slot));
                    }
                    self.finish_results(fr.func_slot, nret, fr.nresults);
                }
                Op::ForPrep => self.for_prep(inst, base)?,
                Op::ForLoop => self.for_loop(inst, base),
                Op::TForPrep => {
                    // the 4th control slot is the iterator's closing value
                    self.register_tbc(base + inst.a() + 3)?;
                    self.add_pc(inst.bx() as i32);
                }
                Op::TForCall => {
                    let abs = base + inst.a();
                    let need = (abs + 7) as usize;
                    if self.stack.len() < need {
                        self.stack.resize(need, Value::Nil);
                    }
                    self.stack[(abs + 4) as usize] = self.stack[abs as usize];
                    self.stack[(abs + 5) as usize] = self.stack[(abs + 1) as usize];
                    self.stack[(abs + 6) as usize] = self.stack[(abs + 2) as usize];
                    let nvars = inst.c() as i32;
                    self.begin_call(abs + 4, Some(2), nvars)?;
                }
                Op::TForLoop => {
                    let a = inst.a();
                    let ctrl = self.r(base, a + 4);
                    if !ctrl.is_nil() {
                        self.set_r(base, a + 2, ctrl);
                        self.add_pc(-(inst.bx() as i32));
                    }
                }
                Op::Closure => {
                    let proto = cl.proto.protos[inst.bx() as usize];
                    let mut ups = Vec::with_capacity(proto.upvals.len());
                    for d in proto.upvals.iter() {
                        if d.in_stack {
                            ups.push(self.find_or_create_upval(base + d.index as u32));
                        } else {
                            ups.push(cl.upvals[d.index as usize]);
                        }
                    }
                    let nc = self.heap.new_closure(proto, ups.into_boxed_slice());
                    self.set_r(base, inst.a(), Value::Closure(nc));
                }
                Op::Vararg => {
                    let abs_a = base + inst.a();
                    let wanted = inst.c() as i32 - 1;
                    let Value::Table(vt) = self.stack[func_slot as usize] else {
                        unreachable!("vararg function without vararg table");
                    };
                    let n_key = Value::Str(self.heap.intern(b"n"));
                    let n = match vt.get(n_key) {
                        Value::Int(n) => n.max(0) as u32,
                        _ => 0,
                    };
                    let count = if wanted < 0 { n } else { wanted as u32 };
                    let need = (abs_a + count) as usize;
                    if self.stack.len() < need {
                        self.stack.resize(need, Value::Nil);
                    }
                    for i in 0..count {
                        let v = if i < n {
                            vt.get_int(i as i64 + 1)
                        } else {
                            Value::Nil
                        };
                        self.stack[(abs_a + i) as usize] = v;
                    }
                    if wanted < 0 {
                        self.top = abs_a + count;
                    }
                }
                Op::GetVarg => {
                    let v = self.stack[func_slot as usize];
                    self.set_r(base, inst.a(), v);
                }
                Op::ExtraArg => unreachable!("EXTRAARG executed directly"),
            }
        }
    }

    #[inline(always)]
    fn pc_of_top(&self) -> u32 {
        self.frames.last().expect("no frame").pc
    }

    #[inline(always)]
    fn bump_pc(&mut self) {
        self.frames.last_mut().expect("no frame").pc += 1;
    }

    #[inline(always)]
    fn add_pc(&mut self, d: i32) {
        let f = self.frames.last_mut().expect("no frame");
        f.pc = (f.pc as i64 + d as i64) as u32;
    }

    /// PUC conditional-skip convention: the JMP that follows is executed when
    /// `cond == k`; otherwise it is skipped.
    #[inline(always)]
    fn cond_skip(&mut self, cond: bool, k: bool) {
        if cond != k {
            self.bump_pc();
        }
    }

    // ---- indexing (with __index/__newindex chains) ----

    fn index_value(&mut self, t: Value, key: Value) -> Result<Value, LuaError> {
        let mut cur = t;
        for _ in 0..MAX_TAG_LOOP {
            let mm = match cur {
                Value::Table(tb) => {
                    let v = tb.get(key);
                    if !v.is_nil() {
                        return Ok(v);
                    }
                    let mm = self.get_mm(cur, Mm::Index);
                    if mm.is_nil() {
                        return Ok(Value::Nil);
                    }
                    mm
                }
                v => {
                    let mm = self.get_mm(v, Mm::Index);
                    if mm.is_nil() {
                        return Err(self.type_err("index", v));
                    }
                    mm
                }
            };
            match mm {
                Value::Closure(_) | Value::Native(_) => {
                    return self.call_mm1(mm, &[cur, key]);
                }
                next => cur = next,
            }
        }
        Err(self.rt_err("'__index' chain too long; possible loop"))
    }

    fn newindex_value(&mut self, t: Value, key: Value, v: Value) -> Result<(), LuaError> {
        let mut cur = t;
        for _ in 0..MAX_TAG_LOOP {
            let mm = match cur {
                Value::Table(tb) => {
                    if !tb.get(key).is_nil() {
                        return self.raw_set(tb, key, v);
                    }
                    let mm = self.get_mm(cur, Mm::NewIndex);
                    if mm.is_nil() {
                        return self.raw_set(tb, key, v);
                    }
                    mm
                }
                bad => {
                    let mm = self.get_mm(bad, Mm::NewIndex);
                    if mm.is_nil() {
                        return Err(self.type_err("index", bad));
                    }
                    mm
                }
            };
            match mm {
                Value::Closure(_) | Value::Native(_) => {
                    self.call_value(mm, &[cur, key, v])?;
                    return Ok(());
                }
                next => cur = next,
            }
        }
        Err(self.rt_err("'__newindex' chain too long; possible loop"))
    }

    fn raw_set(&mut self, t: Gc<Table>, key: Value, v: Value) -> Result<(), LuaError> {
        match unsafe { t.as_mut() }.set(key, v) {
            Ok(()) => Ok(()),
            Err(TableError::NilIndex) => Err(self.rt_err("table index is nil")),
            Err(TableError::NanIndex) => Err(self.rt_err("table index is NaN")),
            Err(TableError::InvalidNext) => unreachable!(),
        }
    }

    /// Equality with __eq (tried when both operands are tables and raw
    /// equality fails — PUC 5.4 rule).
    fn eq_value(&mut self, l: Value, r: Value) -> Result<bool, LuaError> {
        if l.raw_eq(r) {
            return Ok(true);
        }
        if let (Value::Table(_), Value::Table(_)) = (l, r) {
            let mut mm = self.get_mm(l, Mm::Eq);
            if mm.is_nil() {
                mm = self.get_mm(r, Mm::Eq);
            }
            if !mm.is_nil() {
                return Ok(self.call_mm1(mm, &[l, r])?.truthy());
            }
        }
        Ok(false)
    }

    // ---- arithmetic ----

    fn arith_rr(&mut self, inst: Inst, base: u32, op: ArithOp) -> Result<(), LuaError> {
        let l = self.r(base, inst.b());
        let r = self.r(base, inst.c());
        let v = self.arith(op, l, r)?;
        self.set_r(base, inst.a(), v);
        Ok(())
    }

    fn arith(&mut self, op: ArithOp, l: Value, r: Value) -> Result<Value, LuaError> {
        use ArithOp::*;
        match op {
            BAnd | BOr | BXor | Shl | Shr => {
                // strings coerce for bitwise too (PUC tointegerns via cvt2num)
                match (coerce_num(l), coerce_num(r)) {
                    (Some(a), Some(b)) => {
                        let a = self.int_from_num(a)?;
                        let b = self.int_from_num(b)?;
                        let v = match op {
                            BAnd => a & b,
                            BOr => a | b,
                            BXor => a ^ b,
                            Shl => shift_left(a, b),
                            Shr => shift_left(a, b.wrapping_neg()),
                            _ => unreachable!(),
                        };
                        return Ok(Value::Int(v));
                    }
                    _ => return self.arith_mm(op, l, r, "perform bitwise operation on"),
                }
            }
            _ => {}
        }
        let (ln, rn) = match (coerce_num(l), coerce_num(r)) {
            (Some(a), Some(b)) => (a, b),
            _ => return self.arith_mm(op, l, r, "perform arithmetic on"),
        };
        match (op, ln, rn) {
            (Add, Num::Int(a), Num::Int(b)) => Ok(Value::Int(a.wrapping_add(b))),
            (Sub, Num::Int(a), Num::Int(b)) => Ok(Value::Int(a.wrapping_sub(b))),
            (Mul, Num::Int(a), Num::Int(b)) => Ok(Value::Int(a.wrapping_mul(b))),
            (IDiv, Num::Int(a), Num::Int(b)) => {
                if b == 0 {
                    return Err(self.rt_err("attempt to perform 'n//0'"));
                }
                let mut q = a.wrapping_div(b);
                if (a ^ b) < 0 && q.wrapping_mul(b) != a {
                    q -= 1;
                }
                Ok(Value::Int(q))
            }
            (Mod, Num::Int(a), Num::Int(b)) => {
                if b == 0 {
                    return Err(self.rt_err("attempt to perform 'n%0'"));
                }
                let mut m = a.wrapping_rem(b);
                if m != 0 && (m ^ b) < 0 {
                    m += b;
                }
                Ok(Value::Int(m))
            }
            (Add, a, b) => Ok(Value::Float(a.as_f64() + b.as_f64())),
            (Sub, a, b) => Ok(Value::Float(a.as_f64() - b.as_f64())),
            (Mul, a, b) => Ok(Value::Float(a.as_f64() * b.as_f64())),
            (Div, a, b) => Ok(Value::Float(a.as_f64() / b.as_f64())),
            (Pow, a, b) => Ok(Value::Float(a.as_f64().powf(b.as_f64()))),
            (IDiv, a, b) => Ok(Value::Float((a.as_f64() / b.as_f64()).floor())),
            (Mod, a, b) => {
                let (x, y) = (a.as_f64(), b.as_f64());
                let mut m = x % y;
                if m * y < 0.0 {
                    m += y;
                }
                Ok(Value::Float(m))
            }
            _ => unreachable!(),
        }
    }

    pub(crate) fn int_from(&mut self, v: Value, what: &str) -> Result<i64, LuaError> {
        match v {
            Value::Int(i) => Ok(i),
            Value::Float(f) => match crate::runtime::value::f2i_exact(f) {
                Some(i) => Ok(i),
                None => Err(self.rt_err("number has no integer representation")),
            },
            v => Err(self.type_err(what, v)),
        }
    }

    fn int_from_num(&mut self, n: Num) -> Result<i64, LuaError> {
        match n {
            Num::Int(i) => Ok(i),
            Num::Float(f) => match crate::runtime::value::f2i_exact(f) {
                Some(i) => Ok(i),
                None => Err(self.rt_err("number has no integer representation")),
            },
        }
    }

    /// Metamethod fallback for arithmetic/bitwise (left operand first).
    fn arith_mm(&mut self, op: ArithOp, l: Value, r: Value, what: &str) -> Result<Value, LuaError> {
        let event = match op {
            ArithOp::Add => Mm::Add,
            ArithOp::Sub => Mm::Sub,
            ArithOp::Mul => Mm::Mul,
            ArithOp::Div => Mm::Div,
            ArithOp::Mod => Mm::Mod,
            ArithOp::Pow => Mm::Pow,
            ArithOp::IDiv => Mm::IDiv,
            ArithOp::BAnd => Mm::BAnd,
            ArithOp::BOr => Mm::BOr,
            ArithOp::BXor => Mm::BXor,
            ArithOp::Shl => Mm::Shl,
            ArithOp::Shr => Mm::Shr,
        };
        let mut mm = self.get_mm(l, event);
        if mm.is_nil() {
            mm = self.get_mm(r, event);
        }
        if mm.is_nil() {
            let bad = if coerce_num(l).is_none() { l } else { r };
            return Err(self.type_err(what, bad));
        }
        self.call_mm1(mm, &[l, r])
    }

    // ---- comparison ----

    pub(crate) fn less_than(&mut self, l: Value, r: Value, or_eq: bool) -> Result<bool, LuaError> {
        match (l, r) {
            (Value::Int(a), Value::Int(b)) => Ok(if or_eq { a <= b } else { a < b }),
            (Value::Float(a), Value::Float(b)) => Ok(if or_eq { a <= b } else { a < b }),
            (Value::Int(a), Value::Float(b)) => Ok(if or_eq {
                int_le_float(a, b)
            } else {
                int_lt_float(a, b)
            }),
            (Value::Float(a), Value::Int(b)) => Ok(if a.is_nan() {
                false
            } else if or_eq {
                !int_lt_float(b, a)
            } else {
                !int_le_float(b, a)
            }),
            (Value::Str(a), Value::Str(b)) => {
                let (a, b) = (a.as_bytes(), b.as_bytes());
                Ok(if or_eq { a <= b } else { a < b })
            }
            (l, r) => {
                let event = if or_eq { Mm::Le } else { Mm::Lt };
                let mut mm = self.get_mm(l, event);
                if mm.is_nil() {
                    mm = self.get_mm(r, event);
                }
                if mm.is_nil() {
                    return Err(self.rt_err(&format!(
                        "attempt to compare {} with {}",
                        l.type_name(),
                        r.type_name()
                    )));
                }
                Ok(self.call_mm1(mm, &[l, r])?.truthy())
            }
        }
    }

    // ---- numeric for ----

    fn for_prep(&mut self, inst: Inst, base: u32) -> Result<(), LuaError> {
        let a = inst.a();
        let init = self.r(base, a);
        let limit = self.r(base, a + 1);
        let step = self.r(base, a + 2);
        let (Some(init_n), Some(limit_n), Some(step_n)) =
            (as_num(init), as_num(limit), as_num(step))
        else {
            let which = if as_num(init).is_none() {
                "initial"
            } else if as_num(limit).is_none() {
                "limit"
            } else {
                "step"
            };
            return Err(self.rt_err(&format!("'for' {which} value must be a number")));
        };
        match (init_n, step_n) {
            (Num::Int(i0), Num::Int(st)) => {
                if st == 0 {
                    return Err(self.rt_err("'for' step is zero"));
                }
                let (lim, empty) = int_for_limit(limit_n, i0, st);
                if empty {
                    self.add_pc(inst.bx() as i32);
                    return Ok(());
                }
                let count = if st > 0 {
                    (lim as u64).wrapping_sub(i0 as u64) / (st as u64)
                } else {
                    (i0 as u64).wrapping_sub(lim as u64) / (st as i128).unsigned_abs() as u64
                };
                self.set_r(base, a, Value::Int(i0));
                self.set_r(base, a + 1, Value::Int(count as i64));
                self.set_r(base, a + 2, Value::Int(st));
                self.set_r(base, a + 3, Value::Int(i0));
            }
            _ => {
                let (x0, lim, st) = (init_n.as_f64(), limit_n.as_f64(), step_n.as_f64());
                if st == 0.0 {
                    return Err(self.rt_err("'for' step is zero"));
                }
                let runs = if st > 0.0 { x0 <= lim } else { x0 >= lim };
                if !runs {
                    self.add_pc(inst.bx() as i32);
                    return Ok(());
                }
                self.set_r(base, a, Value::Float(x0));
                self.set_r(base, a + 1, Value::Float(lim));
                self.set_r(base, a + 2, Value::Float(st));
                self.set_r(base, a + 3, Value::Float(x0));
            }
        }
        Ok(())
    }

    fn for_loop(&mut self, inst: Inst, base: u32) {
        let a = inst.a();
        match self.r(base, a) {
            Value::Int(cur) => {
                let Value::Int(count) = self.r(base, a + 1) else {
                    unreachable!()
                };
                if count > 0 {
                    let Value::Int(st) = self.r(base, a + 2) else {
                        unreachable!()
                    };
                    let next = cur.wrapping_add(st);
                    self.set_r(base, a, Value::Int(next));
                    self.set_r(base, a + 1, Value::Int(count - 1));
                    self.set_r(base, a + 3, Value::Int(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            Value::Float(cur) => {
                let Value::Float(lim) = self.r(base, a + 1) else {
                    unreachable!()
                };
                let Value::Float(st) = self.r(base, a + 2) else {
                    unreachable!()
                };
                let next = cur + st;
                let cont = if st > 0.0 { next <= lim } else { next >= lim };
                if cont {
                    self.set_r(base, a, Value::Float(next));
                    self.set_r(base, a + 3, Value::Float(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            _ => unreachable!("corrupt for-loop state"),
        }
    }

    // ---- native helpers (used by builtins) ----

    /// A native function's own captured upvalue (self lives at func_slot).
    pub(crate) fn nat_upval(&self, func_slot: u32, i: usize) -> Value {
        let Value::Native(nc) = self.stack[func_slot as usize] else {
            unreachable!("native frame without native closure");
        };
        nc.upvals[i]
    }

    /// Write a native function's own upvalue (stateful iterators).
    pub(crate) fn nat_set_upval(&mut self, func_slot: u32, i: usize, v: Value) {
        let Value::Native(nc) = self.stack[func_slot as usize] else {
            unreachable!("native frame without native closure");
        };
        unsafe { nc.as_mut() }.upvals[i] = v;
    }

    pub(crate) fn nat_arg(&self, func_slot: u32, nargs: u32, i: u32) -> Value {
        if i < nargs {
            self.stack[(func_slot + 1 + i) as usize]
        } else {
            Value::Nil
        }
    }

    pub(crate) fn nat_return(&mut self, func_slot: u32, vals: &[Value]) -> u32 {
        let need = func_slot as usize + vals.len();
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
        for (i, &v) in vals.iter().enumerate() {
            self.stack[func_slot as usize + i] = v;
        }
        vals.len() as u32
    }

    fn concat_values(&mut self, l: Value, r: Value) -> Result<Value, LuaError> {
        fn piece(v: Value) -> Option<Vec<u8>> {
            match v {
                Value::Str(s) => Some(s.as_bytes().to_vec()),
                Value::Int(x) => Some(numeric::num_to_string(Num::Int(x)).into_bytes()),
                Value::Float(x) => Some(numeric::num_to_string(Num::Float(x)).into_bytes()),
                _ => None,
            }
        }
        match (piece(l), piece(r)) {
            (Some(mut a), Some(b)) => {
                a.extend_from_slice(&b);
                Ok(Value::Str(self.heap.intern(&a)))
            }
            (la, _) => {
                let mut mm = self.get_mm(l, Mm::Concat);
                if mm.is_nil() {
                    mm = self.get_mm(r, Mm::Concat);
                }
                if mm.is_nil() {
                    let bad = if la.is_none() { l } else { r };
                    return Err(self.type_err("concatenate", bad));
                }
                self.call_mm1(mm, &[l, r])
            }
        }
    }

    /// tostring with __tostring / __name support.
    pub(crate) fn tostring_value(&mut self, v: Value) -> Result<Vec<u8>, LuaError> {
        let mm = self.get_mm(v, Mm::ToString);
        if !mm.is_nil() {
            return match self.call_mm1(mm, &[v])? {
                Value::Str(s) => Ok(s.as_bytes().to_vec()),
                _ => Err(self.rt_err("'__tostring' must return a string")),
            };
        }
        if let Value::Table(t) = v
            && let Value::Str(name) = self.get_mm(v, Mm::Name)
        {
            let mut out = name.as_bytes().to_vec();
            out.extend_from_slice(format!(": {:p}", t.as_ptr()).as_bytes());
            return Ok(out);
        }
        Ok(self.tostring_basic(v))
    }

    /// Basic tostring (no metamethods).
    pub(crate) fn tostring_basic(&mut self, v: Value) -> Vec<u8> {
        match v {
            Value::Nil => b"nil".to_vec(),
            Value::Bool(true) => b"true".to_vec(),
            Value::Bool(false) => b"false".to_vec(),
            Value::Int(i) => numeric::num_to_string(Num::Int(i)).into_bytes(),
            Value::Float(f) => numeric::num_to_string(Num::Float(f)).into_bytes(),
            Value::Str(s) => s.as_bytes().to_vec(),
            Value::Table(t) => format!("table: {:p}", t.as_ptr()).into_bytes(),
            Value::Closure(c) => format!("function: {:p}", c.as_ptr()).into_bytes(),
            Value::Native(n) => format!("function: builtin: {:p}", n.as_ptr()).into_bytes(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArithOp {
    Add,
    Sub,
    Mul,
    Mod,
    Pow,
    Div,
    IDiv,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
}

fn as_num(v: Value) -> Option<Num> {
    match v {
        Value::Int(i) => Some(Num::Int(i)),
        Value::Float(f) => Some(Num::Float(f)),
        _ => None,
    }
}

/// Number, or string coerced to number (5.5 default string-arith coercion).
fn coerce_num(v: Value) -> Option<Num> {
    match v {
        Value::Int(i) => Some(Num::Int(i)),
        Value::Float(f) => Some(Num::Float(f)),
        Value::Str(s) => numeric::str2num(s.as_bytes(), true, true),
        _ => None,
    }
}

/// Lua shifts: logical on 64 bits; |shift| ≥ 64 yields 0; negative shifts
/// reverse direction.
fn shift_left(a: i64, b: i64) -> i64 {
    if b < 0 {
        if b <= -64 {
            0
        } else {
            ((a as u64) >> (-b as u32)) as i64
        }
    } else if b >= 64 {
        0
    } else {
        ((a as u64) << (b as u32)) as i64
    }
}

/// i < f, exactly (PUC LTintfloat shape).
fn int_lt_float(i: i64, f: f64) -> bool {
    if f.is_nan() {
        return false;
    }
    if f >= 9_223_372_036_854_775_808.0 {
        return true;
    }
    if f < -9_223_372_036_854_775_808.0 {
        return false;
    }
    let ff = f.floor();
    let fi = ff as i64;
    if f == ff { i < fi } else { i <= fi }
}

/// i <= f, exactly.
fn int_le_float(i: i64, f: f64) -> bool {
    if f.is_nan() {
        return false;
    }
    if f >= 9_223_372_036_854_775_808.0 {
        return true;
    }
    if f < -9_223_372_036_854_775_808.0 {
        return false;
    }
    i <= f.floor() as i64
}

/// Clip a numeric `for` limit to the integer range (PUC forlimit). Returns
/// (clipped limit, loop-is-empty).
fn int_for_limit(limit: Num, init: i64, step: i64) -> (i64, bool) {
    match limit {
        Num::Int(l) => {
            let empty = if step > 0 { init > l } else { init < l };
            (l, empty)
        }
        Num::Float(f) => {
            if f.is_nan() {
                return (0, true);
            }
            if step > 0 {
                if f >= 9_223_372_036_854_775_808.0 {
                    (i64::MAX, false)
                } else {
                    let l = f.floor();
                    if l < -9_223_372_036_854_775_808.0 {
                        (i64::MIN, true)
                    } else {
                        let li = l as i64;
                        (li, init > li)
                    }
                }
            } else if f <= -9_223_372_036_854_775_808.0 {
                (i64::MIN, false)
            } else {
                let l = f.ceil();
                if l >= 9_223_372_036_854_775_808.0 {
                    (i64::MAX, init < i64::MAX)
                } else {
                    let li = l as i64;
                    (li, init < li)
                }
            }
        }
    }
}

/// Strip the load-prefix sigil from a chunk name for messages (PUC keeps
/// `@file` / `=name` markers in `source`).
fn chunk_display_name(p: *const crate::runtime::LuaStr) -> &'static [u8] {
    let b = unsafe { crate::runtime::string::bytes_of(p) };
    match b.first() {
        Some(b'@') | Some(b'=') => &b[1..],
        _ => b,
    }
}

impl Vm {
    /// Frame introspection for debug.getinfo: `level` 1 = the Lua function
    /// that called the current native. Returns (closure, current line,
    /// extra vararg count).
    pub(crate) fn frame_info(&mut self, level: i64) -> Option<(Gc<LuaClosure>, u32, i64)> {
        if level < 1 || level as usize > self.frames.len() {
            return None;
        }
        let n_key = Value::Str(self.heap.intern(b"n"));
        let f = &self.frames[self.frames.len() - level as usize];
        let proto = f.closure.proto;
        let pc = (f.pc as usize)
            .saturating_sub(1)
            .min(proto.lines.len().saturating_sub(1));
        let line = proto.lines.get(pc).copied().unwrap_or(0);
        let extra = if proto.is_vararg {
            match self.stack[f.func_slot as usize] {
                Value::Table(t) => match t.get(n_key) {
                    Value::Int(n) => n,
                    _ => 0,
                },
                _ => 0,
            }
        } else {
            0
        };
        Some((f.closure, line, extra))
    }

    /// Read an upvalue cell of a closure (debug.getupvalue).
    pub(crate) fn upvalue_value(&self, cl: Gc<LuaClosure>, idx: usize) -> Value {
        match cl.upvals[idx].state() {
            UpvalState::Open(slot) => self.stack[slot as usize],
            UpvalState::Closed(v) => v,
        }
    }

    /// Write an upvalue cell of a closure (debug.setupvalue).
    pub(crate) fn upvalue_set_value(&mut self, cl: Gc<LuaClosure>, idx: usize, v: Value) {
        let uv = cl.upvals[idx];
        match uv.state() {
            UpvalState::Open(slot) => self.stack[slot as usize] = v,
            UpvalState::Closed(_) => unsafe { uv.as_mut() }.set_closed(v),
        }
    }

    /// Lines for debug.traceback.
    pub(crate) fn traceback_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for f in self.frames.iter().rev() {
            let proto = f.closure.proto;
            let src = chunk_display_name(proto.source.as_ptr());
            let pc = (f.pc as usize)
                .saturating_sub(1)
                .min(proto.lines.len().saturating_sub(1));
            let line = proto.lines.get(pc).copied().unwrap_or(0);
            out.extend_from_slice(b"\n\t");
            out.extend_from_slice(src);
            out.extend_from_slice(format!(":{line}: in ").as_bytes());
            if proto.line_defined == 0 {
                out.extend_from_slice(b"main chunk");
            } else {
                out.extend_from_slice(
                    format!(
                        "function <{}:{}>",
                        String::from_utf8_lossy(src),
                        proto.line_defined
                    )
                    .as_bytes(),
                );
            }
        }
        out
    }
}
