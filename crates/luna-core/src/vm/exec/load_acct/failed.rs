//! Counting a text load that failed to parse as PUC allocates it. PUC
//! generates code while it parses, so everything up to the token the error
//! is on has happened: the main closure, the prototypes opened so far with
//! their arrays as grown, the strings the scanner made, the parser's
//! tables. luna's parse stopped there with no tree; the source up to that
//! token is completed so that it parses, compiled without counting, and
//! the log of that compile replayed up to the token. The prototypes and
//! tables it leaves are garbage, as PUC's are. Then the strings of the
//! message, and what PUC's protected call and `luaD_protectedparser` do
//! on the way out.

use super::*;
use crate::frontend::parser::{ParseScratch, completed, parse_in_place};

impl Vm {
    /// Count the load of `src`, named `source`, that failed with `e`; its
    /// reader's calls counted `splices` (see [`Replay::splice`]).
    pub(in crate::vm::exec) fn acct_failed_load(
        &mut self,
        src: &[u8],
        e: &SyntaxError,
        source: Gc<crate::runtime::LuaStr>,
        splices: Vec<(u32, crate::runtime::puc::Deferred)>,
    ) -> Result<(), SyntaxError> {
        let (v, depth) = (self.version, self.g.nccalls);
        let Some(failed) = self.parse_scratch.failed else {
            let parsed = std::mem::replace(
                &mut self.parse_scratch,
                ParseScratch::new(self.heap.mem_owner()),
            );
            let r = match parsed.recorded() {
                Some(rec) => self.acct_failed_compile(src, e, parsed.parsed().0, rec, splices),
                None => self.splices_stand(splices),
            };
            self.parse_scratch = parsed;
            return r;
        };
        // the construct the load failed on is completed too, and compiles
        let fixed = completed(src, v, depth, failed, true);
        let Some(fixed) = fixed.filter(|_| self.parse_scratch.failed_scan().is_some()) else {
            return self.splices_stand(splices);
        };
        // the completed source, parsed and compiled without counting
        let mut tree = ParseScratch::new(self.heap.mem_owner());
        tree.record = true;
        tree.lenient(true);
        let building = self.heap.build_load(true);
        self.compile_scratch.replay.lenient = true;
        self.compile_scratch.replay.seal = true;
        let made = match parse_in_place(&fixed, v, depth, &mut tree) {
            Ok(()) => {
                let (ast, end_lines) = tree.parsed();
                Some(self.compile_parsed(ast, end_lines, source).map_err(|ce| {
                    let s = &mut self.compile_scratch;
                    crate::compiler::place_error(s, ast, &fixed, v, depth, ce)
                }))
            }
            Err(_) => None,
        };
        let opts = &mut self.compile_scratch.replay;
        (opts.lenient, opts.seal) = (false, false);
        // a compile error PUC meets before the syntax error is the load's
        // (see `Vm::error_before`)
        let early = opts.at.is_some_and(|(ix, _)| ix <= failed.at);
        let proto = match made {
            Some(Ok(p)) => Some(p),
            Some(Err(_)) if !early => opts.sealed,
            _ => None,
        };
        if !early {
            (opts.sealed, opts.at) = (None, None);
        }
        let built = proto.and_then(|p| self.close_chunk(p).ok());
        self.heap.load_built(building);
        if let (Some(Err(_)), true, Some(rec)) = (&made, early, tree.recorded()) {
            // the token is shown as the source has it, not the completion
            let opts = &mut self.compile_scratch.replay;
            if let Some((ix, kind)) = &mut opts.at
                && let Some(t) = crate::frontend::parser::token_at(src, v, depth, *ix, false)
            {
                *kind = t.kind;
            }
            return self.acct_failed_compile(&fixed, e, tree.parsed().0, rec, splices);
        }
        let (Some(cl), Some((marks, _))) = (built, tree.recorded()) else {
            return self.splices_stand(splices);
        };
        let chunk = tree.parsed().0;
        let parsed = std::mem::replace(
            &mut self.parse_scratch,
            ParseScratch::new(self.heap.mem_owner()),
        );
        let (scanned, names) = parsed.failed_scan().expect("checked above");
        let d = self.heap.dialect();
        let mut r = Replay::new((d, self.heap.puc_sizes()), cl.proto, v, scanned, names, src);
        r.splice(splices);
        let log = self.compile_scratch.puc_log.take();
        self.heap.trace_line(format_args!(
            "F at {} scanning {} log {}",
            failed.at,
            failed.scanning,
            log.len()
        ));
        // a token that failed to scan comes after the one the parser was on
        let at = i64::from(failed.at);
        // a goto with no label is found as its function closes, after the
        // final return and before the arrays shrink
        let closes = !unresolved_goto(&e.msg);
        let ended = !failed.scanning;
        let closed = parsed.failed_closed();
        let block_ends =
            e.msg.starts_with(b"'end' expected") || e.msg.starts_with(b"'until' expected");
        // a first token that failed to scan: nothing was read
        let cut = if failed.first {
            Cut {
                events: 0,
                ended: false,
                scans: -1,
                closes,
                block_ends,
                closed,
            }
        } else {
            Cut {
                events: at + i64::from(failed.scanning),
                ended,
                scans: at,
                closes,
                block_ends,
                closed,
            }
        };
        let replayed = self
            .replay_begin(&mut r, cl)
            .and_then(|()| self.replay_log(&mut r, &log, chunk, marks, Some(cut)))
            .and_then(|()| {
                if failed.scanning {
                    let (start, pos) = (failed.scan.0 as usize, failed.scan.1 as usize);
                    let text = &src[start..pos.min(src.len())];
                    use crate::frontend::parser::scan::peak;
                    let n = match text.first() {
                        // a comment is read as the gap before a token
                        Some(b'-') => peak(v, text, b""),
                        // the string as far as it went, its closing quote
                        // not there
                        Some(&q @ (b'"' | b'\'')) => peak(v, b"", &[text, &[q]].concat()) - 1,
                        _ => peak(v, b"", text),
                    };
                    self.grow_buff(&mut r, n);
                }
                unclosed(&mut r, self.heap.puc_sizes());
                self.replay_syntax_error(&mut r, e, failed.near, source.as_bytes())
            });
        self.compile_scratch.puc_log = log;
        self.replay_left(&r, false);
        if replayed.is_ok() {
            // a runtime error runs the message handler where it is raised
            if let Some(m) = self.heap.load_msg().filter(|_| e.line == 0)
                && let Some(v) = self.load_runerror_handled(e, Value::Str(m))
            {
                self.heap.take_load_msg();
                self.load_raised = Some(LuaError(v));
            }
            // `luaD_pcall` shrinks the stack, then the parser's buffers go
            self.shrink_caught_error();
            self.replay_end(&r);
        }
        drop(r);
        self.parse_scratch = parsed;
        replayed
    }
}

impl Vm {
    /// What a reader's calls counted, where it stands: nothing of the load
    /// is replayed.
    pub(in crate::vm::exec) fn splices_stand(
        &mut self,
        splices: Vec<(u32, crate::runtime::puc::Deferred)>,
    ) -> Result<(), SyntaxError> {
        for (_, d) in splices {
            self.heap.replay_reader_call(d);
        }
        Ok(())
    }
}

/// Whether `msg` reports a goto or `break` that found no label.
fn unresolved_goto(msg: &[u8]) -> bool {
    let has = |p: &[u8]| msg.windows(p.len()).any(|w| w == p);
    has(b"no visible label") || has(b"break outside loop") || has(b"not inside a loop")
}

/// The prototypes of the functions still open keep their arrays as grown:
/// their sizes are what freeing them frees.
pub(super) fn unclosed(r: &mut Replay, s: crate::runtime::puc::size::PucSizes) {
    for f in &r.fs {
        if let Some(p) = r.opening.get(f.shape) {
            p.puc_arrays.set(f.puc_caps(r.d, &s));
        }
    }
}
