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
    /// Count the load of `src`, named `source`, that failed with `e`.
    pub(in crate::vm::exec) fn acct_failed_load(
        &mut self,
        src: &[u8],
        e: &SyntaxError,
        source: Gc<crate::runtime::LuaStr>,
    ) -> Result<(), SyntaxError> {
        let (v, depth) = (self.version, self.g.nccalls);
        let Some(failed) = self.parse_scratch.failed else {
            return Ok(());
        };
        if self.parse_scratch.failed_scan().is_none() {
            return Ok(());
        }
        let Some(fixed) = completed(src, v, depth, failed) else {
            return Ok(());
        };
        // the completed source, parsed and compiled without counting
        let mut tree = ParseScratch::new(self.heap.mem_owner());
        tree.record = true;
        self.heap.puc.silent += 1;
        self.heap.set_aside_loads(true);
        let built = match parse_in_place(&fixed, v, depth, &mut tree) {
            Ok(()) => {
                let (ast, end_lines) = tree.parsed();
                self.compile_parsed(ast, end_lines, source)
                    .and_then(|p| self.close_chunk(p))
                    .ok()
            }
            Err(_) => None,
        };
        self.heap.set_aside_loads(false);
        self.heap.puc.silent -= 1;
        let Some(cl) = built else {
            return Ok(());
        };
        let Some((marks, _)) = tree.recorded() else {
            return Ok(());
        };
        let chunk = tree.parsed().0;
        let parsed = std::mem::replace(
            &mut self.parse_scratch,
            ParseScratch::new(self.heap.mem_owner()),
        );
        let (scanned, names) = parsed.failed_scan().expect("checked above");
        let d = self.heap.table_dialect;
        let mut r = Replay::new(d, cl.proto, v, scanned, names, src);
        let log = self.compile_scratch.puc_log.take();
        // a token that failed to scan comes after the one the parser was on
        let at = i64::from(failed.at);
        // a first token that failed to scan: nothing was read
        let cut = if failed.first {
            Cut {
                events: 0,
                ended: false,
                scans: -1,
            }
        } else {
            Cut {
                events: at + i64::from(failed.scanning),
                ended: !failed.scanning,
                scans: at,
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
                unclosed(&mut r, self.heap.sizes);
                self.replay_syntax_error(&mut r, e, failed.near, source.as_bytes())
            });
        self.compile_scratch.puc_log = log;
        self.replay_left(&r, false);
        if replayed.is_ok() {
            // `luaD_pcall` shrinks the stack, then the parser's buffers go
            self.shrink_load_error();
            self.replay_end(&r);
        }
        self.parse_scratch = parsed;
        replayed
    }
}

/// The prototypes of the functions still open keep their arrays as grown:
/// their sizes are what freeing them frees.
fn unclosed(r: &mut Replay, s: crate::runtime::puc_size::PucSizes) {
    for f in &r.fs {
        if let Some(p) = r.opening.get(f.shape) {
            p.puc_arrays.set(f.puc_caps(r.d, &s));
        }
    }
}
