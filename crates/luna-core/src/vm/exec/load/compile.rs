//! Parsing and compiling a text chunk, and placing a compile error the way
//! PUC, which compiles while it parses, reports it.

use super::*;

impl Vm {
    /// Parse and compile `src` named `source`, in the Vm's scratch.
    pub(super) fn compile_text_in_place(
        &mut self,
        src: &[u8],
        source: Gc<crate::runtime::LuaStr>,
    ) -> Result<Gc<crate::runtime::Proto>, SyntaxError> {
        // PUC's `nCcalls` counts protected calls as well
        let depth = self.g.nccalls;
        let parsed = crate::frontend::parser::parse_in_place(
            src,
            self.version,
            depth,
            &mut self.parse_scratch,
        );
        if let Err(e) = parsed {
            let failed = self.parse_scratch.failed;
            // a compile error, not where the parse failed (counted as
            // `Vm::acct_failed_load` finds it)
            return Err(self.error_before(src, failed, source).unwrap_or(e));
        }
        let (chunk, end_lines) = self.parse_scratch.parsed();
        let marks = self.parse_scratch.recorded().map(|(m, _)| m);
        // a counted load that fails is replayed from what it compiled
        let s = &mut self.compile_scratch;
        s.replay.seal = self.parse_scratch.record;
        let made = crate::compiler::compile_parsed(
            chunk,
            end_lines,
            marks,
            self.version,
            source,
            &mut self.heap,
            s,
        );
        s.replay.seal = false;
        made.map_err(|e| crate::compiler::place_error(s, chunk, src, self.version, depth, e))
    }

    /// Compile a parsed chunk named `source`, keeping the parse's vectors
    /// for the next load.
    pub(super) fn compile_text(
        &mut self,
        mut parsed: crate::frontend::parser::Parsed,
        source: Gc<crate::runtime::LuaStr>,
    ) -> Result<Gc<crate::runtime::Proto>, SyntaxError> {
        let src = parsed.src.take();
        let proto = self
            .compile_parsed(&parsed.chunk, &parsed.end_lines, marks_of(&parsed), source)
            .map_err(|e| match &src {
                Some(src) => {
                    let (v, depth) = (self.version, self.g.nccalls);
                    let s = &mut self.compile_scratch;
                    crate::compiler::place_error(s, &parsed.chunk, src, v, depth, e)
                }
                None => e,
            })?;
        self.parse_scratch = crate::frontend::parser::ParseScratch::recycle(parsed);
        Ok(proto)
    }

    /// The compile error a load of `src`, which failed to parse at
    /// `failed`, meets before its syntax error, if any.
    pub(super) fn error_before(
        &mut self,
        src: &[u8],
        failed: Option<crate::frontend::parser::Failed>,
        source: Gc<crate::runtime::LuaStr>,
    ) -> Option<SyntaxError> {
        let (v, depth) = (self.version, self.g.nccalls);
        let (heap, scratch) = (&mut self.heap, &mut self.compile_scratch);
        crate::compiler::error_before(src, v, depth, failed?, source, heap, scratch)
    }

    pub(in crate::vm::exec) fn compile_parsed(
        &mut self,
        chunk: &crate::frontend::ast::Chunk,
        end_lines: &[u32],
        marks: Option<&crate::frontend::parser::Marks>,
        source: Gc<crate::runtime::LuaStr>,
    ) -> Result<Gc<crate::runtime::Proto>, SyntaxError> {
        crate::compiler::compile_parsed(
            chunk,
            end_lines,
            marks,
            self.version,
            source,
            &mut self.heap,
            &mut self.compile_scratch,
        )
    }
}

/// The marks a parse recorded, if it did.
pub(super) fn marks_of(
    p: &crate::frontend::parser::Parsed,
) -> Option<&crate::frontend::parser::Marks> {
    p.recorded().map(|(m, _)| m)
}
