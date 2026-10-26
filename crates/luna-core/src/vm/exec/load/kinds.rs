//! The kinds of chunk a whole-source load takes: a binary chunk, a
//! MacroLua chunk, and a text chunk.

use super::*;

impl Vm {
    /// A binary chunk, undumped.
    pub(super) fn load_binary(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        let allow_puc = self.puc_bytecode_loading;
        if !self.heap.counting() {
            let p = crate::vm::dump::undump_named(
                src,
                &mut self.heap,
                self.version,
                allow_puc,
                chunkname,
            )
            .map_err(SyntaxError::unpositioned)?;
            return self.close_chunk(p);
        }
        // made without counting, then counted as PUC's undumper
        // makes it
        let off = self.heap.counting_off();
        let v = self.version;
        let made = crate::vm::dump::undump_named(src, &mut self.heap, v, allow_puc, chunkname)
            .map_err(SyntaxError::unpositioned)
            .and_then(|p| self.hold_puc_names(p).map(|()| p))
            .and_then(|p| self.close_chunk(p));
        self.heap.counting_on(off);
        let cl = match made {
            Ok(cl) => cl,
            Err(e) if e.is_memory() => return Err(e),
            Err(e) => {
                self.acct_failed_undump(src, chunkname, &e)?;
                return Err(e);
            }
        };
        self.acct_undump(src, cl)?;
        Ok(cl)
    }

    /// A MacroLua chunk: the macros expanded, then parsed and compiled.
    pub(super) fn load_macro(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
        name: Option<Gc<crate::runtime::LuaStr>>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        let source = match name {
            Some(n) => n,
            None => self.heap.try_intern(chunkname)?,
        };
        // MacroLua dialect: drain the lexer into a
        // token vec, run the macro expander pre-pass against the
        // per-Vm registry, then hand the rewritten stream to
        // `parse_tokens`. The AST + compiler are dialect-agnostic
        // because by this point all `@`/quote tokens are gone.
        let mut lexer = crate::frontend::lexer::Lexer::new(src, self.version);
        let mut raw: Vec<crate::frontend::token::TokenInfo> = Vec::new();
        loop {
            let t = lexer.next_token()?;
            let eof = matches!(t.tok, crate::frontend::token::Token::Eof);
            raw.push(t);
            if eof {
                break;
            }
        }
        // Drop the trailing Eof — expander operates on the body and
        // `parse_tokens` reinserts Eof when it runs out of tokens.
        raw.pop();
        let expanded = self.macro_registry.expand(raw)?;
        let depth = self.g.nccalls;
        let parsed = crate::frontend::parser::parse_tokens_at_depth(
            expanded,
            src,
            self.version,
            depth,
            self.heap.mem_owner(),
        )?;
        let p = self.compile_parsed(&parsed.chunk, &parsed.end_lines, source)?;
        self.close_chunk(p)
    }

    /// A text chunk, parsed and compiled, then counted as PUC's parser
    /// makes it.
    pub(super) fn load_text(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
        name: Option<Gc<crate::runtime::LuaStr>>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        // what the parser and the compiler make is counted afterwards,
        // in the order PUC's parser makes it; that includes its own
        // string of the chunk name (`Heap::chunk_name`)
        let counted = self.heap.counting();
        self.parse_scratch.record = counted;
        let span = self.heap.load_begins();
        let building = self.heap.build_load(counted);
        let source = self
            .heap
            .chunk_name(chunkname, name)
            .map_err(SyntaxError::from);
        let made = source
            .clone()
            .and_then(|source| self.compile_text_in_place(src, source))
            .and_then(|p| self.hold_puc_names(p).map(|()| p))
            .and_then(|p| Ok((p, self.close_chunk(p)?)));
        self.heap.load_built(building);
        // what PUC made before the error is counted as it made it
        let failed = match (&made, source) {
            (Err(e), Ok(source)) if counted && !e.is_memory() => {
                self.acct_failed_load(src, e, source, Vec::new())
            }
            _ => Ok(()),
        };
        self.parse_scratch.record = false;
        let out = 'acct: {
            if let Err(e) = failed {
                break 'acct Err(e);
            }
            let cl = match made {
                Ok((_, cl)) => cl,
                Err(e) => break 'acct Err(e),
            };
            let scratch = std::mem::replace(
                &mut self.parse_scratch,
                crate::frontend::parser::ParseScratch::new(self.heap.mem_owner()),
            );
            let counted = match scratch.recorded() {
                Some(rec) => self.acct_text_load(src, scratch.parsed().0, rec, cl, Vec::new()),
                None => Ok(()),
            };
            self.parse_scratch = scratch;
            counted.map(|()| cl)
        };
        self.heap.load_ends(span);
        out
    }
}
