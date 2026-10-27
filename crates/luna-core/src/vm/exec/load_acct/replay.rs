//! The replay's state: the functions PUC's parser has open, its tables,
//! and what has been made so far.

use super::*;
use crate::compiler::TokRef;

/// A function being compiled, in PUC's terms.
#[derive(Default)]
pub(super) struct Fs {
    /// its arrays as PUC grows them
    pub(super) a: FuncArrays,
    /// the function's own table: 5.1 and 5.2 key its strings and constants
    /// in it, 5.5 its constants (`kcache`)
    pub(super) h: Option<PTab>,
    /// which of [`Replay::opening`] is this function's, PUC's layout of
    /// its code, and the luna instructions emitted so far
    pub(super) shape: usize,
    pub(super) lay: crate::vm::dump::Layout,
    pub(super) pc: usize,
    /// where its strings start among `Replay::kstr`
    pub(super) kbase: usize,
}

/// Where the replay of a load that failed stops: before the events at
/// token `events` (with `ended`, after those at it that finish what ended
/// before it: PUC finishes them seeing the token, then fails on it), after
/// what the scanner does up to token `scans`.
#[derive(Clone, Copy)]
pub(super) struct Cut {
    pub(super) events: i64,
    pub(super) ended: bool,
    pub(super) scans: i64,
    /// a function closing at token `events` (with `ended`) closes: the
    /// error was not raised while it closed, after its final return
    pub(super) closes: bool,
    /// the parser failed where it expected the `end` (or `until`) of a
    /// block: the block was over, and what leaving it does was done
    pub(super) block_ends: bool,
    /// the statements and expressions the failed parse read to their end
    /// (see `ParseScratch::failed_closed`): only theirs are done when the
    /// parser fails on the token
    pub(super) closed: (u32, u32),
}

impl Cut {
    /// Whether event `ev`, logged at token `t`, the token of the cut, is
    /// replayed: one of a node that ended before the token (done while
    /// the parser looked at it, before it failed on it), or, when the
    /// error was raised as a function closed, any up to the closing.
    pub(super) fn takes(&self, t: TokRef, ev: PEv, m: &crate::frontend::parser::Marks) -> bool {
        if !self.closes {
            return !matches!(ev, PEv::Close);
        }
        let ended = |id: u32, closed: u32, follow: &[u32]| {
            id < closed
                && follow
                    .get(id as usize)
                    .is_some_and(|&f| i64::from(f) <= self.events)
        };
        let (stats, exprs) = self.closed;
        match t {
            // a statement's last token: what its body leaves is done
            // before the parser checks the token is the `end` it expects
            TokRef::StatAfter(_, -1) if self.block_ends => true,
            // done as the scanner reached the expression's first token,
            // before the parser read it (`luaK_infix` after an operator)
            TokRef::First(..) => true,
            TokRef::After(e, _) | TokRef::Open(e, _) => ended(e.0, exprs, &m.expr_follow),
            TokRef::StatFirst(s, _)
            | TokRef::StatAfter(s, _)
            | TokRef::StatMark(s, _)
            | TokRef::MarkPlus(s, ..) => ended(s.0, stats, &m.stat_follow),
            _ => false,
        }
    }
}

/// A value in a parser table, as the debug library is shown it.
#[derive(Clone, Copy)]
pub(in crate::vm::exec) enum PVal {
    V(Value),
    /// another parser table, by number
    Tab(u32),
    /// the main closure (5.5 anchors it)
    Closure,
}

/// A write into parser table `tab` (`val` `None`: the key is removed),
/// kept for [`Vm::reader_view`].
#[derive(Clone, Copy)]
pub(in crate::vm::exec) struct TabOp {
    pub(in crate::vm::exec) tab: u32,
    pub(in crate::vm::exec) key: PVal,
    pub(in crate::vm::exec) val: Option<PVal>,
}

/// The replay's state.
pub(super) struct Replay<'s> {
    pub(super) d: Dialect,
    pub(super) sizes: ParserSizes,
    /// the chunk's main closure
    pub(super) cl: Option<Gc<LuaClosure>>,
    /// 5.2: the table of the function closing, while it anchors the token
    /// read last in the enclosing function's (still on the stack)
    pub(super) closing_tab: Option<*mut crate::runtime::heap::GcHeader>,
    /// the strings among the constants of the functions open, each one's
    /// after those of the function it is nested in
    pub(super) kstr: Vec<Gc<crate::runtime::LuaStr>>,
    pub(super) fs: Vec<Fs>,
    /// the chunk's table (5.3+): the scanner's strings, 5.3 and 5.4's
    /// constants, 5.5's anchors
    pub(super) h: Option<PTab>,
    /// the parser tables made so far
    pub(super) tabs: u32,
    /// the first long string of each content keyed in a parser table: a
    /// table finds a long string by its bytes
    pub(super) long_keys: std::collections::HashMap<Box<[u8]>, Gc<crate::runtime::LuaStr>>,
    /// the scanner's buffer
    pub(super) buff: usize,
    pub(super) actvar: GrowVec,
    pub(super) gt: GrowVec,
    pub(super) label: GrowVec,
    /// the prototypes in the order their functions open
    pub(super) opening: Vec<Gc<Proto>>,
    pub(super) opened: usize,
    pub(super) version: crate::version::LuaVersion,
    /// the prototypes in the order their functions close (postorder), and
    /// how many have closed
    pub(super) closing: Vec<Gc<Proto>>,
    pub(super) closed: usize,
    /// the chunk name, as the prototypes hold it
    pub(super) source: Gc<crate::runtime::LuaStr>,
    /// the strings the scanner made, by token, and the chunk's names and
    /// source their text is in
    pub(super) scanned: &'s Scanned,
    pub(super) names: &'s crate::frontend::ast::Names,
    pub(super) src: &'s [u8],
    /// by name number: its string once made, and the table it was last
    /// found in or put in (see [`Replay::table_id`]; 0 for none)
    pub(super) syms: Vec<(Option<Gc<crate::runtime::LuaStr>>, u32)>,
    /// the same by reserved word
    pub(super) reserved:
        [(Option<Gc<crate::runtime::LuaStr>>, u32); crate::frontend::parser::scan::RESERVED],
    /// a distinct key for each object 5.5 anchors that luna has no object
    /// for
    pub(super) anchors: usize,
    /// the long strings the scanner made that no constant has taken yet
    pub(super) longs: Vec<Gc<crate::runtime::LuaStr>>,
    /// what the reader of a load read piece by piece counted, and the token
    /// whose scan called it (-1: before the parse starts)
    pub(super) splices: std::collections::VecDeque<(i64, crate::runtime::puc::Deferred)>,
    /// the writes into the parser tables, when they are to be shown
    pub(super) rec: Option<Vec<TabOp>>,
}

/// The prototypes under `p` in the order their functions open (preorder)
/// and in the order they close.
pub(super) fn walk(p: Gc<Proto>, opening: &mut Vec<Gc<Proto>>, closing: &mut Vec<Gc<Proto>>) {
    opening.push(p);
    for &c in p.protos.iter() {
        walk(c, opening, closing);
    }
    closing.push(p);
}

impl<'s> Replay<'s> {
    /// A replay of the load of `proto`, whose scanner made `scanned`.
    pub(super) fn new(
        (d, sizes): (Dialect, ParserSizes),
        proto: Gc<Proto>,
        version: crate::version::LuaVersion,
        scanned: &'s Scanned,
        names: &'s crate::frontend::ast::Names,
        src: &'s [u8],
    ) -> Replay<'s> {
        let (mut opening, mut closing) = (Vec::new(), Vec::new());
        walk(proto, &mut opening, &mut closing);
        #[cfg(any(debug_assertions, feature = "gc-trace"))]
        crate::vm::dump::check_compiled(&proto, version);
        Replay {
            d,
            sizes,
            cl: None,
            closing_tab: None,
            kstr: Vec::new(),
            fs: Vec::new(),
            h: None,
            tabs: 0,
            long_keys: Default::default(),
            buff: 0,
            actvar: GrowVec::default(),
            gt: GrowVec::default(),
            label: GrowVec::default(),
            opening,
            opened: 0,
            version,
            closing,
            closed: 0,
            source: proto.source,
            scanned,
            names,
            src,
            syms: vec![(None, 0); names.len()],
            reserved: [(None, 0); crate::frontend::parser::scan::RESERVED],
            anchors: 0,
            longs: Vec::new(),
            splices: Default::default(),
            rec: None,
        }
    }

    /// Keep a write into parser table `tab`, when they are being kept.
    pub(super) fn note(&mut self, tab: u32, key: PVal, val: Option<PVal>) {
        if let Some(rec) = &mut self.rec {
            rec.push(TabOp { tab, key, val });
        }
    }

    /// What the scanner's entry for a string holds: `true` up to 5.3, the
    /// string itself from 5.4.
    pub(super) fn scan_val(&self, s: Gc<crate::runtime::LuaStr>) -> PVal {
        if self.d >= Dialect::L54 {
            PVal::V(Value::Str(s))
        } else {
            PVal::V(Value::Bool(true))
        }
    }

    /// The reader calls `splices` made at their byte offsets, each placed
    /// at the token whose scan read that byte.
    pub(super) fn splice(&mut self, splices: Vec<(u32, crate::runtime::puc::Deferred)>) {
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

impl Drop for Replay<'_> {
    /// However the replay ends, the prototypes of the functions still open
    /// keep the arrays it counted for them: what freeing them takes off
    /// (see `Proto::puc_arrays`).
    fn drop(&mut self) {
        if self.rec.is_some() {
            return;
        }
        for f in &self.fs {
            if let Some(p) = self.opening.get(f.shape) {
                p.puc_arrays.set(f.a.caps(&self.sizes));
            }
        }
    }
}
