//! The replay's state: the functions PUC's parser has open, its tables,
//! and what has been made so far.

use super::*;

/// A function being compiled, in PUC's terms.
#[derive(Default)]
pub(super) struct Fs {
    pub(super) code: Arr,
    pub(super) line: Arr,
    pub(super) abs: Arr,
    pub(super) k: Arr,
    pub(super) p: Arr,
    pub(super) loc: Arr,
    pub(super) up: Arr,
    /// the function's own table: 5.1 and 5.2 key its strings and constants
    /// in it, 5.5 its constants (`kcache`)
    pub(super) h: Option<PTab>,
    pub(super) prev_line: u32,
    pub(super) iwthabs: u32,
    /// the last instruction's line entry: absolute, and the line before it
    /// (to take it back, PUC `removelastlineinfo`)
    pub(super) last_abs: bool,
    pub(super) last_prev: u32,
    /// which of the shapes is this function's, and the luna instructions
    /// emitted so far
    pub(super) shape: usize,
    pub(super) pc: usize,
    /// where its strings start among `Replay::kstr`
    pub(super) kbase: usize,
}

impl Fs {
    /// The bytes of each of its prototype's arrays as grown (see
    /// `Proto::puc_arrays`).
    pub(super) fn puc_caps(&self, d: Dialect, s: &crate::runtime::puc_size::PucSizes) -> [u32; 7] {
        let line = if matches!(d, Dialect::L54 | Dialect::L55) {
            1
        } else {
            4
        };
        [
            (self.code, 4),
            (self.p, s.ptr),
            (self.k, s.tvalue),
            (self.line, line),
            (self.abs, s.abslineinfo),
            (self.loc, s.locvar),
            (self.up, s.upvaldesc),
        ]
        .map(|(a, e)| (a.cap * e) as u32)
    }
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
    pub(super) long_keys: std::collections::HashMap<Box<[u8]>, usize>,
    /// the scanner's buffer
    pub(super) buff: usize,
    pub(super) actvar: Arr,
    pub(super) gt: Arr,
    pub(super) label: Arr,
    /// the prototypes in the order their functions open
    pub(super) opening: Vec<Gc<Proto>>,
    pub(super) opened: usize,
    /// PUC's layout of each function, in the order they open
    pub(super) shapes: Vec<crate::vm::dump::Shape>,
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
    pub(super) splices: std::collections::VecDeque<(i64, crate::runtime::heap::Deferred)>,
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
            cl: None,
            closing_tab: None,
            kstr: Vec::new(),
            fs: Vec::new(),
            h: None,
            tabs: 0,
            long_keys: Default::default(),
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
    pub(super) fn splice(&mut self, splices: Vec<(u32, crate::runtime::heap::Deferred)>) {
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
