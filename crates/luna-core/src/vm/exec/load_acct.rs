//! Counting a text load as PUC allocates it.
//!
//! luna parses to a tree and compiles the tree; PUC generates code while it
//! scans. The compile runs without counting and logs what PUC's parser and
//! code generator would allocate, each step with the token PUC's scanner is
//! on (`compiler::puc_log`). The parse recorded the tokens and the strings
//! the scanner makes for them (`parser::Marks`), and the two are
//! replayed in PUC's order: the main closure, the tables the parser keys
//! strings and constants in (real tables, garbage once the load is done,
//! as PUC's are), the prototypes and the growth and final trimming of their
//! arrays, the strings, the scanner's buffer and the parser's descriptor
//! arrays. Which tables there are differs by dialect: 5.1 and 5.2 give each
//! function its own, 5.3 and 5.4 have one for the chunk, 5.5 has one for
//! the chunk's strings and one per function for its constants.

use super::*;
use crate::compiler::TokRef;
use crate::compiler::puc_log::PEv;
use crate::frontend::parser::scan::{ScanKind, ScanText, Scanned};
use crate::runtime::heap::PucKind;
use crate::runtime::table::{Dialect, TableError};
use crate::runtime::{Proto, Table};

mod consts;
mod events;
mod scan;

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
    /// the prototypes in the order their functions open
    opening: Vec<Gc<Proto>>,
    opened: usize,
    /// PUC's layout of each function, in the order they open
    shapes: Vec<crate::vm::dump::Shape>,
    /// the prototypes in the order their functions close (postorder), and
    /// how many have closed
    closing: Vec<Gc<Proto>>,
    closed: usize,
    /// the chunk name, as the prototypes hold it
    source: Gc<crate::runtime::LuaStr>,
    /// the strings the scanner made, by token, and the chunk's names and
    /// source their text is in
    scanned: &'s Scanned,
    names: &'s crate::frontend::ast::Names,
    src: &'s [u8],
    /// by name number: its string once made, and the table it was last
    /// found in or put in (see [`Replay::table_id`]; 0 for none)
    syms: Vec<(Option<Gc<crate::runtime::LuaStr>>, u32)>,
    /// the same by reserved word
    reserved: [(Option<Gc<crate::runtime::LuaStr>>, u32); crate::frontend::parser::scan::RESERVED],
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

/// The prototypes under `p` in the order their functions open (preorder)
/// and in the order they close.
fn walk(p: Gc<Proto>, opening: &mut Vec<Gc<Proto>>, closing: &mut Vec<Gc<Proto>>) {
    opening.push(p);
    for &c in p.protos.iter() {
        walk(c, opening, closing);
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
        parsed: &crate::frontend::parser::ParseScratch,
        cl: Gc<LuaClosure>,
    ) -> Result<(), SyntaxError> {
        let proto = cl.proto;
        let Some((marks, scanned)) = parsed.recorded() else {
            return Ok(());
        };
        let chunk = parsed.parsed().0;
        let d = self.heap.table_dialect;
        let s = self.heap.sizes;
        let (mut opening, mut closing) = (Vec::new(), Vec::new());
        walk(proto, &mut opening, &mut closing);
        let shapes = crate::vm::dump::shapes(&proto, self.version).unwrap_or_default();
        let mut r = Replay {
            d,
            fs: Vec::new(),
            h: None,
            buff: 0,
            actvar: Arr::default(),
            gt: Arr::default(),
            label: Arr::default(),
            opening,
            opened: 0,
            shapes,
            closing,
            closed: 0,
            source: proto.source,
            scanned,
            names: &chunk.names,
            src,
            syms: vec![(None, 0); chunk.names.len()],
            reserved: [(None, 0); crate::frontend::parser::scan::RESERVED],
            anchors: 0,
        };
        // `luaY_parser` (5.5 `f_parser`) before the main function opens
        match d {
            Dialect::L51 => {}
            Dialect::L52 => self.acct_main_closure(cl),
            Dialect::L53 | Dialect::L54 => {
                self.acct_main_closure(cl);
                r.h = Some(self.heap.try_new_table()?);
            }
            Dialect::L55 => {
                // the anchor table, then the closure anchored in it
                r.h = Some(self.heap.try_new_table()?);
                self.acct_main_closure(cl);
                self.anchor_in_h(&mut r, None)?;
            }
        }
        let log = self.compile_scratch.puc_log.take();
        let replayed = self.replay_log(&mut r, &log, chunk, marks);
        self.compile_scratch.puc_log = log;
        replayed?;
        // 5.1's `f_parser` makes the closure now; the others give it its
        // upvalue (`luaF_initupvals`)
        if d == Dialect::L51 {
            self.acct_main_closure(cl);
        } else {
            match cl.upvals().first() {
                Some(&uv) => {
                    let h = uv.as_ptr() as *mut crate::runtime::heap::GcHeader;
                    if crate::runtime::heap::is_uncounted(h) {
                        self.heap.acct_promote_obj(h, PucKind::Upval, s.upval);
                    } else {
                        self.heap.acct_obj(PucKind::Upval, s.upval);
                    }
                }
                None => self.heap.acct_obj(PucKind::Upval, s.upval),
            }
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

    /// The main closure, made without counting, is counted where PUC makes
    /// it.
    fn acct_main_closure(&mut self, cl: Gc<LuaClosure>) {
        let size = self.heap.closure_size(&cl.proto, cl.upvals().len());
        let h = cl.as_ptr() as *mut crate::runtime::heap::GcHeader;
        self.heap.acct_promote_obj(h, PucKind::LuaClosure, size);
    }
}
