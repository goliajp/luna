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

mod arr;
mod consts;
mod events;
mod failed;
mod scan;
mod syntax_msg;

use arr::Arr;

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

/// Where the replay of a load that failed stops: before the events at
/// token `events` (with `ended`, after those at it that finish what ended
/// before it: PUC finishes them seeing the token, then fails on it), after
/// what the scanner does up to token `scans`.
#[derive(Clone, Copy)]
struct Cut {
    events: i64,
    ended: bool,
    scans: i64,
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
    /// what the reader of a load read piece by piece counted, and the token
    /// whose scan called it (-1: before the parse starts)
    splices: std::collections::VecDeque<(i64, crate::runtime::heap::Deferred)>,
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

impl<'s> Replay<'s> {
    /// A replay of the load of `proto`, whose scanner made `scanned`.
    fn new(
        d: Dialect,
        proto: Gc<Proto>,
        version: crate::version::LuaVersion,
        scanned: &'s Scanned,
        names: &'s crate::frontend::ast::Names,
        src: &'s [u8],
    ) -> Replay<'s> {
        let (mut opening, mut closing) = (Vec::new(), Vec::new());
        walk(proto, &mut opening, &mut closing);
        Replay {
            d,
            fs: Vec::new(),
            h: None,
            buff: 0,
            actvar: Arr::default(),
            gt: Arr::default(),
            label: Arr::default(),
            opening,
            opened: 0,
            shapes: crate::vm::dump::shapes(&proto, version).unwrap_or_default(),
            closing,
            closed: 0,
            source: proto.source,
            scanned,
            names,
            src,
            syms: vec![(None, 0); names.len()],
            reserved: [(None, 0); crate::frontend::parser::scan::RESERVED],
            anchors: 0,
            splices: Default::default(),
        }
    }

    /// The reader calls `splices` made at their byte offsets, each placed
    /// at the token whose scan read that byte.
    fn splice(&mut self, splices: Vec<(u32, crate::runtime::heap::Deferred)>) {
        let ends = &self.scanned.ends;
        for (off, d) in splices {
            let tok = if off == 0 {
                -1
            } else {
                ends.partition_point(|&e| e < off) as i64
            };
            self.splices.push_back((tok, d));
        }
    }
}

impl Vm {
    /// Count a text chunk that compiled to `proto` (without counting) as
    /// PUC's parser allocates it. Nothing is counted while luna builds its
    /// own objects.
    pub(super) fn acct_text_load(
        &mut self,
        src: &[u8],
        chunk: &crate::frontend::ast::Chunk,
        rec: (&crate::frontend::parser::Marks, &Scanned),
        cl: Gc<LuaClosure>,
        splices: Vec<(u32, crate::runtime::heap::Deferred)>,
    ) -> Result<(), SyntaxError> {
        let proto = cl.proto;
        let (marks, scanned) = rec;
        let d = self.heap.table_dialect;
        let s = self.heap.sizes;
        let mut r = Replay::new(d, proto, self.version, scanned, &chunk.names, src);
        r.splice(splices);
        self.replay_begin(&mut r, cl)?;
        let log = self.compile_scratch.puc_log.take();
        let replayed = self.replay_log(&mut r, &log, chunk, marks, None);
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
        self.replay_end(&r);
        Ok(())
    }

    /// What PUC makes before the main function opens: from 5.2 the main
    /// closure, from 5.3 the chunk's table (5.5 anchors the closure in it).
    fn replay_begin(&mut self, r: &mut Replay, cl: Gc<LuaClosure>) -> Result<(), SyntaxError> {
        // `luaY_parser` (5.5 `f_parser`) before the main function opens;
        // 5.1's `f_parser` checks for a collection first
        #[cfg(feature = "gc-trace")]
        if r.d == Dialect::L51 {
            self.trace_checkpoint();
        }
        // the reader's first call, which reads the first character
        while r.splices.front().is_some_and(|&(t, _)| t < 0) {
            let (_, d) = r.splices.pop_front().expect("peeked");
            self.heap.defer_apply(d);
        }
        match r.d {
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
                self.anchor_in_h(r, None)?;
            }
        }
        Ok(())
    }

    /// `luaD_protectedparser` frees the scanner's buffer and the parser's
    /// descriptor arrays.
    fn replay_end(&mut self, r: &Replay) {
        self.heap.acct_block(r.buff, 0);
        if r.d != Dialect::L51 {
            let (vd, ld) = crate::runtime::puc_size::dyd_sizes(r.d);
            for (a, e) in [(r.actvar, vd), (r.gt, ld), (r.label, ld)] {
                self.heap.acct_block(a.cap * e, 0);
            }
        }
    }

    /// The strings PUC's prototypes under `p` keep for their local
    /// variable and upvalue names, made now (while nothing is counted) and
    /// held by the prototypes: the replay counts each where PUC's scanner
    /// makes it, and the collector frees it only with its prototype.
    pub(super) fn hold_puc_names(&mut self, p: Gc<Proto>) -> Result<(), SyntaxError> {
        let names = p.locvars.iter().map(|v| v.name.as_str());
        let names = names.chain(p.upvals.iter().map(|u| u.name.as_str()));
        let mut held = Vec::with_capacity(p.locvars.len() + p.upvals.len());
        for n in names {
            held.push(self.heap.try_intern(n.as_bytes())?);
        }
        *p.puc_names.borrow_mut() = held;
        for &c in p.protos.iter() {
            self.hold_puc_names(c)?;
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
