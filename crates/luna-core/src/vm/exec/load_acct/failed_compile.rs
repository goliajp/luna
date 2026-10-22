//! Counting a text load that parsed and failed to compile as PUC
//! allocates it: PUC's code generator met the error while it parsed, at
//! the token luna places the error at. The compile's log is replayed up to
//! that token, from the prototypes the compile left (each function it had
//! open closed as it stood); then the strings of the message, and what
//! PUC's protected call does on the way out.

use super::*;
use crate::frontend::ast::Chunk;
use crate::frontend::parser::Marks;

impl Vm {
    /// Count the load of `src` that failed with `e`, `chunk` its tree with
    /// `rec` recorded, its reader's calls having counted `splices`.
    pub(in crate::vm::exec) fn acct_failed_compile(
        &mut self,
        src: &[u8],
        e: &SyntaxError,
        chunk: &Chunk,
        rec: (&Marks, &Scanned),
        splices: Vec<(u32, crate::runtime::heap::Deferred)>,
    ) -> Result<(), SyntaxError> {
        let opts = std::mem::take(&mut self.compile_scratch.replay);
        let (Some(proto), Some((at, near))) = (opts.sealed, opts.at) else {
            return self.splices_stand(splices);
        };
        self.heap.puc.silent += 1;
        let aside = self.heap.set_aside_loads(true);
        let cl = self.close_chunk(proto);
        self.heap.set_aside_loads(aside);
        self.heap.puc.silent -= 1;
        let Ok(cl) = cl else {
            return self.splices_stand(splices);
        };
        let (marks, scanned) = rec;
        let d = self.heap.table_dialect;
        let mut r = Replay::new(
            (d, self.heap.sizes),
            proto,
            self.version,
            scanned,
            &chunk.names,
            src,
        );
        r.splice(splices);
        let log = self.compile_scratch.puc_log.take();
        let at = i64::from(at);
        let cut = Cut {
            events: at,
            ended: true,
            scans: at,
            closes: true,
            block_ends: false,
            closed: (u32::MAX, u32::MAX),
        };
        let replayed = self
            .replay_begin(&mut r, cl)
            .and_then(|()| self.replay_log(&mut r, &log, chunk, marks, Some(cut)))
            .and_then(|()| {
                failed::unclosed(&mut r, self.heap.sizes);
                self.replay_syntax_error(&mut r, e, near, proto.source.as_bytes())
            });
        self.compile_scratch.puc_log = log;
        self.replay_left(&r, false);
        if replayed.is_ok() {
            // `luaD_pcall` shrinks the stack, then the parser's buffers go
            self.shrink_caught_error();
            self.replay_end(&r);
        }
        replayed
    }
}
