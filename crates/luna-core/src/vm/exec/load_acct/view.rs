//! The parser's tables as PUC's parser holds them when it asks a `load`
//! reader for more, made for the debug library to look at.
//!
//! luna parses the whole chunk first and keeps no tables while it does.
//! What PUC holds at a reader call follows from the source read so far:
//! the tokens its scanner has finished are parsed (the source cut where
//! the token it is reading starts, and completed so that it parses, as
//! for a load that failed), compiled without counting, and the load's
//! replay run up to that token, writing down each entry PUC's parser puts
//! in its tables.

use super::*;
use crate::frontend::parser::{ParseScratch, completed, parse_in_place, scanned_upto};

/// What [`Vm::reader_view`] found.
pub(in crate::vm::exec) struct ReaderView {
    /// the writes into the parser tables, in order
    pub(crate) ops: Vec<TabOp>,
    /// the table of each function open, outermost first (5.1, 5.2, 5.5)
    pub(crate) fs_tabs: Vec<Option<u32>>,
    /// the chunk's table (5.3 on)
    pub(crate) chunk_tab: Option<u32>,
    /// the main function compiled from what was read, standing for PUC's
    /// closure: its name, where it starts and its parameters are PUC's
    pub(crate) main: Gc<LuaClosure>,
    /// PUC's array size and node count of each table PUC's parser holds
    pub(crate) sizes: Vec<(u32, u64, u64)>,
}

impl Vm {
    /// The parser's tables at a reader call of a load of `src` (read so
    /// far) named `source`; `None` when the source read cannot be parsed
    /// that far. Nothing is counted.
    pub(in crate::vm::exec) fn reader_view(
        &mut self,
        src: &[u8],
        source: Gc<crate::runtime::LuaStr>,
    ) -> Option<ReaderView> {
        self.uncounted(|vm| vm.reader_view_silent(src, source))
    }

    fn reader_view_silent(
        &mut self,
        src: &[u8],
        source: Gc<crate::runtime::LuaStr>,
    ) -> Option<ReaderView> {
        let (v, depth) = (self.version, self.g.nccalls);
        let (done, cut_at) = scanned_upto(src, v);
        let mut text = src[..cut_at].to_vec();
        let mut tree = ParseScratch::new(self.heap.mem_owner());
        tree.record = true;
        if parse_in_place(&text, v, depth, &mut tree).is_err() {
            text = completed(&text, v, depth, tree.failed?, false)?;
            tree = ParseScratch::new(self.heap.mem_owner());
            tree.record = true;
            parse_in_place(&text, v, depth, &mut tree).ok()?;
        }
        let (ast, end_lines) = tree.parsed();
        let proto = self.compile_parsed(ast, end_lines, source).ok()?;
        let cl = self.close_chunk(proto).ok()?;
        let (marks, scanned) = tree.recorded()?;
        let chunk = tree.parsed().0;
        let d = self.heap.table_dialect;
        let mut r = Replay::new(
            (d, self.heap.puc.sizes),
            cl.proto,
            v,
            scanned,
            &chunk.names,
            &text,
        );
        r.rec = Some(Vec::new());
        let cut = Cut {
            events: i64::from(done),
            ended: false,
            scans: i64::from(done) - 1,
            closes: true,
            block_ends: false,
            closed: (u32::MAX, u32::MAX),
        };
        let log = self.compile_scratch.puc_log.take();
        let replayed = self
            .replay_begin(&mut r, cl)
            .and_then(|()| self.replay_log(&mut r, &log, chunk, marks, Some(cut)));
        self.compile_scratch.puc_log = log;
        replayed.ok()?;
        let sizes =
            r.fs.iter()
                .filter_map(|f| f.h.as_ref())
                .chain(&r.h)
                .map(|t| {
                    let (a, n) = t.sizes();
                    (t.id, a, n)
                })
                .collect();
        Some(ReaderView {
            sizes,
            fs_tabs: r.fs.iter().map(|f| f.h.as_ref().map(|t| t.id)).collect(),
            chunk_tab: r.h.as_ref().map(|t| t.id),
            ops: r.rec.take().unwrap_or_default(),
            main: cl,
        })
    }
}
