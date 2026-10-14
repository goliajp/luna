//! Counting a text load as PUC allocates it.
//!
//! luna parses to a tree and compiles the tree; PUC generates code while it
//! scans. The compile runs without counting and logs what PUC's parser and
//! code generator would allocate, each step with the token PUC's scanner is
//! on (`compiler::puc_log`). Here a second, recording parse finds the
//! tokens and the strings the scanner makes for them, and the two are
//! replayed in PUC's order: the main closure, the scanner's string table
//! (a real table, garbage once the load is done, as PUC's is), the
//! prototypes and the growth and final trimming of their arrays, the
//! strings, the scanner's buffer and the parser's descriptor arrays.

use super::*;
use crate::compiler::puc_log::PEv;
use crate::runtime::Proto;
use crate::runtime::heap::PucKind;
use crate::runtime::table::{Dialect, TableError};

/// One array PUC grows by doubling (`luaM_growvector`) and trims when the
/// function ends.
#[derive(Clone, Copy, Default)]
struct Arr {
    n: usize,
    cap: usize,
}

impl Arr {
    /// One more element of `elem` bytes.
    fn push(&mut self, heap: &mut Heap, elem: usize) {
        if self.n + 1 > self.cap {
            let new = if self.cap < 2 { 4 } else { self.cap * 2 };
            heap.acct_block(self.cap * elem, new * elem);
            self.cap = new;
        }
        self.n += 1;
    }

    /// Grown to hold `n` (the parser's descriptor arrays, which shrink and
    /// grow again).
    fn reach(&mut self, heap: &mut Heap, n: usize, elem: usize) {
        while self.cap < n {
            let new = if self.cap < 2 { 4 } else { self.cap * 2 };
            heap.acct_block(self.cap * elem, new * elem);
            self.cap = new;
        }
        self.n = n;
    }

    /// Trimmed to its elements (`luaM_shrinkvector`).
    fn trim(&mut self, heap: &mut Heap, elem: usize) {
        heap.acct_block(self.cap * elem, self.n * elem);
        self.cap = self.n;
    }
}

/// A function being compiled, in PUC's terms.
#[derive(Default)]
struct Fs {
    code: Arr,
    line: Arr,
    abs: Arr,
    k: Arr,
    p: Arr,
    loc: Arr,
    up: Arr,
    prev_line: u32,
    iwthabs: u32,
    /// which of the shapes is this function's, and the luna instructions
    /// emitted so far
    shape: usize,
    pc: usize,
}

/// The replay's state.
struct Replay {
    d: Dialect,
    fs: Vec<Fs>,
    /// the scanner's string table
    h: Gc<crate::runtime::Table>,
    /// the scanner's buffer
    buff: usize,
    actvar: Arr,
    gt: Arr,
    label: Arr,
    /// the prototypes in the order their functions open, with their
    /// definition lines
    lines_defined: Vec<u32>,
    opened: usize,
    /// PUC's layout of each function, in the order they open
    shapes: Vec<crate::vm::dump::Shape>,
    /// the prototypes in the order their functions close (postorder), and
    /// how many have closed
    closing: Vec<Gc<Proto>>,
    closed: usize,
    /// the chunk name, as the prototypes hold it
    source: Gc<crate::runtime::LuaStr>,
}

/// A failed store into the scanner table: out of memory (its keys are
/// strings, numbers, booleans and itself, all valid).
fn table_err(e: TableError) -> SyntaxError {
    match e {
        TableError::Memory(o) => o.into(),
        _ => unreachable!("a scanner-table key is a valid key"),
    }
}

/// The definition line of each prototype under `p`, in the order the
/// functions open (preorder), and the prototypes in the order they close.
fn walk(p: Gc<Proto>, lines: &mut Vec<u32>, closing: &mut Vec<Gc<Proto>>) {
    lines.push(p.line_defined);
    for &c in p.protos.iter() {
        walk(c, lines, closing);
    }
    closing.push(p);
}

impl Vm {
    /// Count a text chunk that compiled to `proto` (without counting) as
    /// PUC's parser allocates it. Nothing is counted while luna builds its
    /// own objects.
    pub(super) fn acct_text_load(
        &mut self,
        src: &[u8],
        chunk: &crate::frontend::ast::Chunk,
        proto: Gc<Proto>,
    ) -> Result<(), SyntaxError> {
        if self.heap.puc.silent > 0 {
            return Ok(());
        }
        let (marks, scanned) =
            crate::frontend::parser::record_strings(src, self.version, self.g.nccalls)?;
        let log = crate::compiler::puc_log::resolve(&self.compile_scratch.puc_log, chunk, &marks);
        let d = self.heap.table_dialect;
        let s = self.heap.sizes;
        // `luaY_parser`: the main closure, the scanner's table, the main
        // prototype, the scanner's buffer
        self.heap.acct_obj(PucKind::LuaClosure, s.lua_closure(1));
        let h = self.heap.try_new_table()?;
        let (mut lines, mut closing) = (Vec::new(), Vec::new());
        walk(proto, &mut lines, &mut closing);
        let shapes = crate::vm::dump::shapes(&proto, self.version).unwrap_or_default();
        let mut r = Replay {
            d,
            fs: Vec::new(),
            h,
            buff: 0,
            actvar: Arr::default(),
            gt: Arr::default(),
            label: Arr::default(),
            lines_defined: lines,
            opened: 0,
            shapes,
            closing,
            closed: 0,
            source: proto.source,
        };
        let mut strs = scanned.strings.iter().peekable();
        let mut peaks = scanned.peaks.iter().peekable();
        for &(tok, ev) in &log {
            #[cfg(feature = "gc-trace")]
            self.heap.trace.line(format_args!("E {tok} {ev:?}"));
            // what the scanner does up to this token: it fills its buffer
            // while it reads a token, then makes the token's string
            loop {
                let p = peaks.peek().map(|&&(t, _)| i64::from(t));
                let st = strs.peek().map(|(t, _)| i64::from(*t));
                match (p, st) {
                    (Some(p), _) if p <= tok && st.is_none_or(|st| p <= st) => {
                        let (_, peak) = peaks.next().expect("peeked");
                        let size = crate::frontend::parser::scan_buf_grown(
                            self.version,
                            r.buff,
                            *peak as usize,
                        );
                        self.heap.acct_block(r.buff, size);
                        r.buff = size;
                    }
                    (_, Some(st)) if st <= tok => {
                        let (_, bytes) = strs.next().expect("peeked");
                        self.replay_scan(&mut r, bytes)?;
                    }
                    _ => break,
                }
            }
            self.replay_event(&mut r, ev)?;
        }
        // `luaF_initupvals`, then `luaD_protectedparser` frees the buffer and
        // the descriptor arrays
        self.heap.acct_obj(PucKind::Upval, s.upval);
        self.heap.acct_block(r.buff, 0);
        let (vd, ld) = crate::runtime::puc_size::dyd_sizes(d);
        for (a, e) in [(r.actvar, vd), (r.gt, ld), (r.label, ld)] {
            self.heap.acct_block(a.cap * e, 0);
        }
        Ok(())
    }

    /// The scanner makes the string of a token (`luaX_newstring`): the
    /// string, then the table entry that anchors it.
    fn replay_scan(&mut self, r: &mut Replay, bytes: &[u8]) -> Result<(), SyntaxError> {
        let s = self.heap.try_intern(bytes)?;
        let key = Value::Str(s);
        // SAFETY: `r.h` is the scanner table made for this load, held only
        // by the replay
        let t = unsafe { r.h.as_mut() };
        if t.get(key).is_nil() {
            t.set(&mut self.heap, key, key).map_err(table_err)?;
        }
        Ok(())
    }

    fn replay_event(&mut self, r: &mut Replay, ev: PEv) -> Result<(), SyntaxError> {
        let s = self.heap.sizes;
        let lineinfo_elem = if matches!(r.d, Dialect::L54 | Dialect::L55) {
            1
        } else {
            4
        };
        match ev {
            PEv::Open => {
                let ld = r.lines_defined.get(r.opened).copied().unwrap_or(0);
                if let Some(parent) = r.fs.last_mut() {
                    // `addprototype`: the parent's array, then the new prototype
                    parent.p.push(&mut self.heap, s.ptr);
                }
                // the parser's own string of the chunk name (`luaS_new`):
                // 5.1 makes it first, the others after the main prototype
                if r.opened == 0 && r.d == Dialect::L51 {
                    self.heap.acct_source(r.source);
                }
                self.heap.acct_obj(PucKind::Proto, s.proto);
                if r.opened == 0 && r.d != Dialect::L51 {
                    self.heap.acct_source(r.source);
                }
                if r.opened == 0 {
                    // `luaX_setinput` sizes the scanner's buffer
                    r.buff = 32;
                    self.heap.acct_block(0, 32);
                }
                let shape = r.opened;
                r.opened += 1;
                r.fs.push(Fs {
                    prev_line: ld,
                    shape,
                    ..Fs::default()
                });
            }
            PEv::Prologue => {
                // a vararg function's first instructions (`setvararg`)
                let Some(f) = r.fs.last() else {
                    return Ok(());
                };
                let (shape, ld) = (f.shape, f.prev_line);
                let pro = r
                    .shapes
                    .get(shape)
                    .map_or(0, |sh| sh.pc_map.first().copied().unwrap_or(0));
                for j in 0..pro as usize {
                    let line = r.shapes[shape].lines.get(j).copied().unwrap_or(ld);
                    self.replay_code(r, line);
                }
            }
            PEv::Lit(text) => self.replay_scan(r, text.as_bytes())?,
            PEv::Close => {
                let mut f = r.fs.pop().unwrap_or_default();
                let h = &mut self.heap;
                f.code.trim(h, 4);
                f.line.trim(h, lineinfo_elem);
                f.abs.trim(h, s.abslineinfo);
                f.k.trim(h, s.tvalue);
                f.p.trim(h, s.ptr);
                f.loc.trim(h, s.locvar);
                f.up.trim(h, s.upvaldesc);
                if let Some(p) = r.closing.get(r.closed) {
                    let arrays = [
                        (f.code, 4),
                        (f.p, s.ptr),
                        (f.k, s.tvalue),
                        (f.line, lineinfo_elem),
                        (f.abs, s.abslineinfo),
                        (f.loc, s.locvar),
                        (f.up, s.upvaldesc),
                    ];
                    p.puc_arrays.set(arrays.map(|(a, e)| (a.n * e) as u32));
                }
                r.closed += 1;
            }
            PEv::Code(line) => {
                let Some(f) = r.fs.last() else {
                    return Ok(());
                };
                let (shape, pc) = (f.shape, f.pc);
                // the PUC instructions that stand for this one, with their
                // lines, and the constants PUC adds for it
                let (from, to) = r.shapes.get(shape).map_or((0, 1), |sh| {
                    let a = sh.pc_map.get(pc).copied().unwrap_or(0) as usize;
                    let b = sh.pc_map.get(pc + 1).copied().unwrap_or(a as u32 + 1) as usize;
                    (a, b.max(a + 1))
                });
                for j in from..to {
                    let l = r
                        .shapes
                        .get(shape)
                        .and_then(|sh| sh.lines.get(j).copied())
                        .unwrap_or(line);
                    self.replay_code(r, l);
                }
                let added = r
                    .shapes
                    .get(shape)
                    .map_or(0, |sh| sh.k_added.iter().filter(|&&p| p == pc).count());
                for _ in 0..added {
                    if let Some(f) = r.fs.last_mut() {
                        f.k.push(&mut self.heap, s.tvalue);
                    }
                }
                if let Some(f) = r.fs.last_mut() {
                    f.pc += 1;
                }
            }
            PEv::CodeUp(line) => {
                let child = r.fs.pop();
                let coded = self.replay_event(r, PEv::Code(line));
                r.fs.extend(child);
                coded?;
            }
            PEv::Use => {}
            PEv::Uncode => {
                if let Some(f) = r.fs.last_mut() {
                    f.pc -= 1;
                    f.code.n -= 1;
                    f.line.n -= 1;
                }
            }
            PEv::Const(key) => {
                if let Some(f) = r.fs.last_mut() {
                    f.k.push(&mut self.heap, s.tvalue);
                }
                if let Some(k) = key.and_then(|k| self.scanner_key(r, k)) {
                    // SAFETY: as in `replay_scan`
                    let t = unsafe { r.h.as_mut() };
                    if t.get(k).is_nil() {
                        t.set(&mut self.heap, k, Value::Int(0)).map_err(table_err)?;
                    }
                }
            }
            PEv::Upval(depth) => {
                if let Some(f) = r.fs.get_mut(depth as usize) {
                    f.up.push(&mut self.heap, s.upvaldesc);
                }
            }
            PEv::LocVar => {
                if let Some(f) = r.fs.last_mut() {
                    f.loc.push(&mut self.heap, s.locvar);
                }
            }
            PEv::ActVar(n) => {
                // `new_localvar` asks for room past the new entry
                let (vd, _) = crate::runtime::puc_size::dyd_sizes(r.d);
                r.actvar.reach(&mut self.heap, n as usize + 1, vd);
                r.actvar.n = n as usize;
            }
            PEv::Goto(n) => {
                let (_, ld) = crate::runtime::puc_size::dyd_sizes(r.d);
                r.gt.reach(&mut self.heap, n as usize, ld);
            }
            PEv::Label(n) => {
                let (_, ld) = crate::runtime::puc_size::dyd_sizes(r.d);
                r.label.reach(&mut self.heap, n as usize, ld);
            }
        }
        Ok(())
    }

    /// One PUC instruction on `line` (`luaK_code`): the code array, then
    /// the line info (5.4+ `savelineinfo`, with its absolute entries).
    fn replay_code(&mut self, r: &mut Replay, line: u32) {
        let d = r.d;
        let s = self.heap.sizes;
        let Some(f) = r.fs.last_mut() else {
            return;
        };
        let h = &mut self.heap;
        f.code.push(h, 4);
        if matches!(d, Dialect::L54 | Dialect::L55) {
            let dif = i64::from(line) - i64::from(f.prev_line);
            let wthabs = f.iwthabs;
            f.iwthabs += 1;
            if dif.abs() >= 0x80 || wthabs >= 128 {
                f.abs.push(h, s.abslineinfo);
                f.iwthabs = 1;
            }
            f.prev_line = line;
            f.line.push(h, 1);
        } else {
            f.line.push(h, 4);
        }
    }

    /// The scanner-table key of a constant that is not a string (a string
    /// constant's key is the scanner's own string, already there).
    fn scanner_key(&self, r: &Replay, k: crate::compiler::const_map::ConstKey) -> Option<Value> {
        use crate::compiler::const_map::ConstKey as K;
        match k {
            K::Int(i) => Some(Value::Int(i)),
            K::Float(b) => Some(Value::Float(f64::from_bits(b))),
            K::Bool(b) => Some(Value::Bool(b)),
            K::Nil => Some(Value::Table(r.h)),
            K::Zero => Some(Value::Int(0)),
            K::LightInt(_) | K::Str(_) | K::Bytes(..) => None,
        }
    }
}
