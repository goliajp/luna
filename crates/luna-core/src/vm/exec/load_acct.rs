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
use crate::compiler::puc_log::PEv;
use crate::compiler::puc_rec::{At, Rec};
use crate::frontend::parser::scan::{ScanKind, ScanText, Scanned};
use crate::runtime::Proto;
use crate::runtime::dialect::Dialect;
use crate::runtime::puc::PucObj;
use crate::runtime::puc::api::{PucCount, PucParse};
use crate::runtime::puc::records::parse::{
    FuncArrays, GrowVec, PKey, PTab, ParserArray, ParserSizes,
};

mod consts;
mod events;
mod failed;
mod failed_compile;
mod funcs;
#[cfg(test)]
mod long_tests;
mod replay;
mod scan;
mod steps;
mod syntax_msg;
mod undump;
mod undump_probe;
mod view;
use replay::{Cut, Fs, Replay, walk};
pub(super) use replay::{PVal, TabOp};
pub(super) use view::ReaderView;

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
        splices: Vec<(u32, crate::runtime::puc::Deferred)>,
    ) -> Result<(), SyntaxError> {
        let proto = cl.proto;
        let (marks, scanned) = rec;
        let d = self.heap.dialect();
        let mut r = Replay::new(
            (d, self.heap.parser_sizes()),
            proto,
            self.version,
            scanned,
            &chunk.names,
            src,
        );
        r.splice(splices);
        let log = self.compile_scratch.puc_log.take();
        let replayed = self
            .replay_begin(&mut r, cl)
            .and_then(|()| self.replay_log(&mut r, &log, marks, None));
        self.compile_scratch.puc_log = log;
        self.replay_left(&r, replayed.is_ok());
        replayed?;
        self.replay_long_names(&mut r, proto);
        // 5.1's `f_parser` makes the closure now; the others give it its
        // upvalue (`luaF_initupvals`)
        if d == Dialect::L51 {
            self.acct_main_closure(cl);
        } else {
            match cl.upvals().first() {
                Some(&uv) => {
                    let h = uv.hdr();
                    if h.is_uncounted() {
                        self.heap.promote_obj(h, PucObj::Upval);
                    } else {
                        self.heap.new_obj(PucObj::Upval);
                    }
                    self.heap.barrier_new_upval(cl, uv);
                }
                None => self.heap.new_obj(PucObj::Upval),
            }
        }
        self.replay_end(&r);
        Ok(())
    }

    /// What PUC makes before the main function opens: from 5.2 the main
    /// closure, from 5.3 the chunk's table (5.5 anchors the closure in it).
    fn replay_begin(&mut self, r: &mut Replay, cl: Gc<LuaClosure>) -> Result<(), SyntaxError> {
        // 5.5's `f_parser` makes the anchor table before it reads
        if r.d == Dialect::L55 {
            r.h = Some(self.new_ptab(r)?);
        }
        // `luaY_parser` (5.5 `f_parser`) before the main function opens;
        // the reader's first call, which reads the first character
        while r.splices.front().is_some_and(|&(t, _)| t < 0) {
            let (_, d) = r.splices.pop_front().expect("peeked");
            self.heap.replay_reader_call(d);
        }
        r.cl = Some(cl);
        if r.rec.is_none() && r.d != Dialect::L51 {
            crate::runtime::puc::records::parse::set_parsing(cl, true);
        }
        match r.d {
            Dialect::L51 => {}
            Dialect::L52 => self.acct_main_closure(cl),
            Dialect::L53 | Dialect::L54 => {
                self.acct_main_closure(cl);
                r.h = Some(self.new_ptab(r)?);
            }
            Dialect::L55 => {
                // the closure, anchored in the table
                self.acct_main_closure(cl);
                self.anchor_in_h(r, None)?;
            }
        }
        Ok(())
    }

    /// A new parser table (`luaH_new`): the chunk's (5.3 on) takes every
    /// string the scanner makes, a function's only some.
    fn new_ptab(&mut self, r: &mut Replay) -> Result<PTab, SyntaxError> {
        r.tabs += 1;
        let keys = if r.tabs == 1 && r.d >= Dialect::L53 {
            r.scanned.strings.len() + 8
        } else {
            16
        };
        // a view of the tables for the debug library counts nothing
        Ok(PTab::new(&mut self.heap, r.tabs, keys, r.rec.is_none())?)
    }

    /// `luaD_protectedparser` frees the scanner's buffer and the parser's
    /// descriptor arrays.
    fn replay_end(&mut self, r: &Replay) {
        self.heap.acct_block(r.buff, 0);
        if r.d != Dialect::L51 {
            self.heap.parser_free(r.actvar, ParserArray::ActVar);
            self.heap.parser_free(r.gt, ParserArray::Gotos);
            self.heap.parser_free(r.label, ParserArray::Labels);
        }
    }

    /// The strings PUC's prototypes under `p` keep for their local
    /// variable and upvalue names, made now (while nothing is counted) and
    /// held by the prototypes: the replay counts each where PUC's scanner
    /// makes it, and the collector frees it only with its prototype.
    /// From 5.3 one long name is one string for the whole chunk (the
    /// scanner's single table of names), in 5.2 one per function.
    pub(super) fn hold_puc_names(&mut self, p: Gc<Proto>) -> Result<(), SyntaxError> {
        self.hold_names_in(p, &mut Vec::new())
    }

    fn hold_names_in(
        &mut self,
        p: Gc<Proto>,
        longs: &mut Vec<Gc<crate::runtime::LuaStr>>,
    ) -> Result<(), SyntaxError> {
        let shared = !matches!(self.heap.dialect(), Dialect::L51 | Dialect::L52);
        let names = p.locvars.iter().map(|v| v.name.as_str());
        let names = names.chain(p.upvals.iter().map(|u| u.name.as_str()));
        let mut held = Vec::with_capacity(p.locvars.len() + p.upvals.len());
        for n in names {
            let n = n.as_bytes();
            let s = match longs.iter().find(|s| shared && s.as_bytes() == n) {
                Some(&s) => s,
                None => self.heap.try_intern(n)?,
            };
            if shared && n.len() > crate::runtime::string::MAX_SHORT_LEN {
                longs.push(s);
            }
            held.push(s);
        }
        *p.puc_names.borrow_mut() = held;
        for &c in p.protos.iter() {
            self.hold_names_in(c, longs)?;
        }
        Ok(())
    }

    /// The main closure, made without counting, is counted where PUC makes
    /// it.
    fn acct_main_closure(&mut self, cl: Gc<LuaClosure>) {
        self.heap.promote_main_closure(cl);
    }
}
