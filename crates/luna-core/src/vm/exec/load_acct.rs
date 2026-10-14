//! Counting a text load as PUC allocates it.
//!
//! luna parses to a tree and compiles the tree; PUC generates code while it
//! scans. The compile runs without counting and logs what PUC's parser and
//! code generator would allocate, each step with the token PUC's scanner is
//! on (`compiler::puc_log`). Here a second, recording parse finds the
//! tokens and the strings the scanner makes for them, and the two are
//! replayed in PUC's order: the main closure, the tables the parser keys
//! strings and constants in (real tables, garbage once the load is done,
//! as PUC's are), the prototypes and the growth and final trimming of their
//! arrays, the strings, the scanner's buffer and the parser's descriptor
//! arrays. Which tables there are differs by dialect: 5.1 and 5.2 give each
//! function its own, 5.3 and 5.4 have one for the chunk, 5.5 has one for
//! the chunk's strings and one per function for its constants.

use super::*;
use crate::compiler::puc_log::PEv;
use crate::frontend::parser::scan::{ScanKind, Scanned};
use crate::runtime::heap::PucKind;
use crate::runtime::table::{Dialect, TableError};
use crate::runtime::{Proto, Table};

mod events;

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
    /// the function's own table: 5.1 and 5.2 key its strings and constants
    /// in it, 5.5 its constants (`kcache`)
    h: Option<Gc<Table>>,
    prev_line: u32,
    iwthabs: u32,
    /// the last instruction's line entry: absolute, and the line before it
    /// (to take it back, PUC `removelastlineinfo`)
    last_abs: bool,
    last_prev: u32,
    /// which of the shapes is this function's, and the luna instructions
    /// emitted so far
    shape: usize,
    pc: usize,
}

/// The replay's state.
struct Replay<'s> {
    d: Dialect,
    fs: Vec<Fs>,
    /// the chunk's table (5.3+): the scanner's strings, 5.3 and 5.4's
    /// constants, 5.5's anchors
    h: Option<Gc<Table>>,
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
    /// the strings the scanner made, by token
    scanned: &'s Scanned,
    /// a distinct key for each object 5.5 anchors that luna has no object
    /// for
    anchors: usize,
}

/// A failed store into a parser table: out of memory (its keys are
/// strings, numbers, booleans and tables, all valid).
fn table_err(e: TableError) -> SyntaxError {
    match e {
        TableError::Memory(o) => o.into(),
        _ => unreachable!("a parser-table key is a valid key"),
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
        let (mut lines, mut closing) = (Vec::new(), Vec::new());
        walk(proto, &mut lines, &mut closing);
        let shapes = crate::vm::dump::shapes(&proto, self.version).unwrap_or_default();
        let mut r = Replay {
            d,
            fs: Vec::new(),
            h: None,
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
            scanned: &scanned,
            anchors: 0,
        };
        // `luaY_parser` (5.5 `f_parser`) before the main function opens
        match d {
            Dialect::L51 => {}
            Dialect::L52 => self.heap.acct_obj(PucKind::LuaClosure, s.lua_closure(1)),
            Dialect::L53 | Dialect::L54 => {
                self.heap.acct_obj(PucKind::LuaClosure, s.lua_closure(1));
                r.h = Some(self.heap.try_new_table()?);
            }
            Dialect::L55 => {
                // the anchor table, then the closure anchored in it
                r.h = Some(self.heap.try_new_table()?);
                self.heap.acct_obj(PucKind::LuaClosure, s.lua_closure(1));
                self.anchor_in_h(&mut r, None)?;
            }
        }
        self.replay_log(&mut r, &log)?;
        // 5.1's `f_parser` makes the closure now; the others give it its
        // upvalue (`luaF_initupvals`)
        if d == Dialect::L51 {
            self.heap.acct_obj(PucKind::LuaClosure, s.lua_closure(0));
        } else {
            self.heap.acct_obj(PucKind::Upval, s.upval);
        }
        // `luaD_protectedparser` frees the buffer and the descriptor arrays
        self.heap.acct_block(r.buff, 0);
        if d != Dialect::L51 {
            let (vd, ld) = crate::runtime::puc_size::dyd_sizes(d);
            for (a, e) in [(r.actvar, vd), (r.gt, ld), (r.label, ld)] {
                self.heap.acct_block(a.cap * e, 0);
            }
        }
        Ok(())
    }

    /// The log's events, with what the scanner does up to each one's token:
    /// it fills its buffer while it reads a token, then makes the token's
    /// string.
    fn replay_log(&mut self, r: &mut Replay, log: &[(i64, PEv)]) -> Result<(), SyntaxError> {
        let scanned = r.scanned;
        let mut strs = scanned.strings.iter().peekable();
        let mut peaks = scanned.peaks.iter().peekable();
        for &(tok, ev) in log {
            #[cfg(feature = "gc-trace")]
            self.heap.trace.line(format_args!("E {tok} {ev:?}"));
            loop {
                let p = peaks.peek().map(|&&(t, _)| i64::from(t));
                let st = strs.peek().map(|(t, ..)| i64::from(*t));
                match (p, st) {
                    (Some(p), _) if p <= tok && st.is_none_or(|st| p <= st) => {
                        let (_, peak) = peaks.next().expect("peeked");
                        // each save that overflows grows the buffer once
                        while r.buff < *peak as usize {
                            let size = crate::frontend::parser::scan::grown(
                                self.version,
                                r.buff,
                                r.buff + 1,
                            );
                            self.heap.acct_block(r.buff, size);
                            r.buff = size;
                        }
                    }
                    (_, Some(st)) if st <= tok => {
                        let (_, bytes, kind) = strs.next().expect("peeked");
                        self.replay_scan(r, bytes, *kind)?;
                    }
                    _ => break,
                }
            }
            self.replay_event(r, tok, ev)?;
        }
        Ok(())
    }

    /// 5.5 `luaD_anchorobj`: an object keyed in the chunk's table (`t`, or
    /// one luna has no object for).
    fn anchor_in_h(&mut self, r: &mut Replay, t: Option<Gc<Table>>) -> Result<(), SyntaxError> {
        let key = match t {
            Some(t) => Value::Table(t),
            None => {
                r.anchors += 1;
                Value::LightUserdata(r.anchors as *const ())
            }
        };
        match r.h {
            Some(h) => self.set_in(h, key, key),
            None => Ok(()),
        }
    }

    /// `t[k] = v` in a parser table, when `k` is not there yet (PUC
    /// `luaH_set` on a new key; a key already there allocates nothing).
    fn set_in(&mut self, t: Gc<Table>, k: Value, v: Value) -> Result<(), SyntaxError> {
        if !t.get(k).is_nil() {
            return Ok(());
        }
        #[cfg(feature = "gc-trace")]
        if let Value::Str(s) = k {
            self.heap.trace.line(format_args!(
                "K {:p} {} {:08x}",
                t.as_ptr(),
                String::from_utf8_lossy(s.as_bytes()),
                s.hash()
            ));
        }
        self.store_in(t, k, v)
    }

    /// `t[k] = v` in a parser table.
    fn store_in(&mut self, t: Gc<Table>, k: Value, v: Value) -> Result<(), SyntaxError> {
        // SAFETY: parser tables are made for this load and held only by
        // the replay
        let t = unsafe { t.as_mut() };
        t.set(&mut self.heap, k, v).map_err(table_err)
    }
}
