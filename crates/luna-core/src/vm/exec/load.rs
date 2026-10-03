//! `load`: a chunk parsed and compiled (or undumped) into a closure over
//! the globals table.

use super::*;

impl Vm {
    /// Parse + compile a chunk and close it over the globals table.
    pub fn load(&mut self, src: &[u8], chunkname: &[u8]) -> Result<Gc<LuaClosure>, SyntaxError> {
        self.load_named(src, chunkname, None)
    }

    /// [`Vm::load`] with the chunk name already a heap string (`name`,
    /// whose bytes are `chunkname`), which the functions then share.
    pub(crate) fn load_named(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
        name: Option<Gc<crate::runtime::LuaStr>>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        // Reject oversize input *before* handing the parser/lexer a
        // potentially multi-GB slice. The PUC-shaped `not enough memory`
        // message keeps `heavy.lua::loadrep` compatibility: that test
        // accepts either `string length overflow` or `not enough memory`
        // as the failure mode for a feeder loop that outruns the host
        // allocator. See `set_loader_input_budget`.
        if src.len() > self.loader_input_budget {
            return Err(SyntaxError {
                line: 0,
                msg: b"not enough memory".to_vec(),
            });
        }
        // a precompiled (binary) chunk is undumped; source is parsed + compiled
        let is_bytecode = crate::vm::dump::is_binary_chunk(src);
        if is_bytecode && !self.bytecode_loading {
            return Err(SyntaxError {
                line: 0,
                msg: b"attempt to load a binary chunk (bytecode loading disabled)".to_vec(),
            });
        }
        let proto = if is_bytecode {
            let allow_puc = self.puc_bytecode_loading;
            crate::vm::dump::undump_named(src, &mut self.heap, self.version, allow_puc, chunkname)
                .map_err(SyntaxError::unpositioned)?
        } else if self.version.is_macro_lua() {
            let source = name.unwrap_or_else(|| self.heap.intern(chunkname));
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
            let depth = self.c_depth + self.pcall_depth;
            let parsed =
                crate::frontend::parser::parse_tokens_at_depth(expanded, src, self.version, depth)?;
            crate::compiler::compile_parsed(
                &parsed.chunk,
                &parsed.end_lines,
                self.version,
                source,
                &mut self.heap,
                &mut self.compile_scratch,
            )?
        } else {
            // PUC's `nCcalls` counts protected calls as well
            let depth = self.c_depth + self.pcall_depth;
            let source = name.unwrap_or_else(|| self.heap.intern(chunkname));
            let scratch = std::mem::take(&mut self.parse_scratch);
            let parsed = crate::frontend::parser::parse_reusing(src, self.version, depth, scratch)?;
            let proto = crate::compiler::compile_parsed(
                &parsed.chunk,
                &parsed.end_lines,
                self.version,
                source,
                &mut self.heap,
                &mut self.compile_scratch,
            )?;
            self.parse_scratch = crate::frontend::parser::ParseScratch::recycle(parsed);
            proto
        };
        if self.heap.track_chunk_roots {
            self.heap.chunk_roots.push(proto);
        }
        // PUC `lua_load` (lapi.c) only seeds the loaded closure's first
        // upvalue with the globals table when the closure has *exactly* one
        // upvalue — that's the main-chunk `_ENV` case. A dumped non-main
        // function with two-or-more upvalues keeps every cell at nil; the
        // host must use `debug.setupvalue` to wire them up. 5.2 calls.lua
        // :293's `assert(x() == nil)` pins this contract.
        let n = proto.upvals.len();
        let mut ups: Vec<Gc<Upvalue>> = Vec::with_capacity(n.max(1));
        if n == 0 {
            // synthetic main chunk has no declared upvalues, but the engine
            // still expects at least one cell so the host can probe via
            // `debug.upvalueid` etc. Match the historical luna shape.
            ups.push(
                self.heap
                    .new_upvalue(UpvalState::Closed(Value::Table(self.globals))),
            );
        } else if n == 1 {
            ups.push(
                self.heap
                    .new_upvalue(UpvalState::Closed(Value::Table(self.globals))),
            );
        } else {
            for _ in 0..n {
                ups.push(self.heap.new_upvalue(UpvalState::Closed(Value::Nil)));
            }
        }
        Ok(self.heap.new_closure(proto, ups.into_boxed_slice()))
    }
}
