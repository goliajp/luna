//! `load`: a chunk parsed and compiled (or undumped) into a closure over
//! the globals table.

use super::*;
use crate::runtime::mem::{LVec, Oom, catch_load_oom, outside_load};

/// The memory error of a load whose frontend ran out of memory.
#[cold]
fn load_oom(heap: &crate::runtime::Heap) -> SyntaxError {
    heap.mem_ctx().raise_oom();
    SyntaxError::memory()
}

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
        self.guarded_load(|vm| vm.load_named_inner(src, chunkname, name))
    }

    fn load_named_inner(
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
            let parsed = crate::frontend::parser::parse_tokens_at_depth(
                expanded,
                src,
                self.version,
                depth,
                self.heap.mem_owner(),
            )?;
            self.compile_parsed(&parsed.chunk, &parsed.end_lines, source)?
        } else {
            // PUC's `nCcalls` counts protected calls as well
            let depth = self.c_depth + self.pcall_depth;
            let source = name.unwrap_or_else(|| self.heap.intern(chunkname));
            let scratch = self.parse_scratch.take();
            let parsed = crate::frontend::parser::parse_reusing(src, self.version, depth, scratch)?;
            self.compile_text(parsed, source)?
        };
        Ok(self.close_chunk(proto))
    }

    /// Compile a parsed chunk named `source`, keeping the parse's vectors
    /// for the next load.
    fn compile_text(
        &mut self,
        parsed: crate::frontend::parser::Parsed,
        source: Gc<crate::runtime::LuaStr>,
    ) -> Result<Gc<crate::runtime::Proto>, SyntaxError> {
        let proto = self.compile_parsed(&parsed.chunk, &parsed.end_lines, source)?;
        self.parse_scratch = crate::frontend::parser::ParseScratch::recycle(parsed);
        Ok(proto)
    }

    fn compile_parsed(
        &mut self,
        chunk: &crate::frontend::ast::Chunk,
        end_lines: &[u32],
        source: Gc<crate::runtime::LuaStr>,
    ) -> Result<Gc<crate::runtime::Proto>, SyntaxError> {
        crate::compiler::compile_parsed(
            chunk,
            end_lines,
            self.version,
            source,
            &mut self.heap,
            &mut self.compile_scratch,
        )
    }

    /// Run `f`, a load, so that running out of memory anywhere in it (the
    /// parser, the compiler, the objects they make) is the memory error.
    /// What the load built is dropped on the way out, and the compiler's
    /// vectors, left half filled, are replaced.
    fn guarded_load<R>(
        &mut self,
        f: impl FnOnce(&mut Vm) -> Result<R, SyntaxError>,
    ) -> Result<R, SyntaxError> {
        match catch_load_oom(|| f(self)) {
            Ok(r) => r,
            Err(()) => {
                self.compile_scratch = crate::compiler::CompileScratch::new(self.heap.mem());
                Err(load_oom(&self.heap))
            }
        }
    }

    /// Start loading a text chunk that a reader hands over piece by piece
    /// (`lua_load`, `load` with a function): what the parse needs from the
    /// vm, so that the reader can use the vm while the parse runs. `None`
    /// for MacroLua, whose macro pass needs the whole source first.
    #[doc(hidden)]
    pub fn text_load(&mut self) -> Option<TextLoad> {
        if self.version.is_macro_lua() {
            return None;
        }
        Some(TextLoad {
            version: self.version,
            depth: self.c_depth + self.pcall_depth,
            budget: self.loader_input_budget,
            scratch: self.parse_scratch.take(),
        })
    }

    /// Compile what [`TextLoad::parse`] read as the chunk `chunkname` and
    /// close it over the globals, as [`Vm::load`] does.
    #[doc(hidden)]
    pub fn load_parsed(
        &mut self,
        parsed: ParsedText,
        chunkname: &[u8],
        name: Option<Gc<crate::runtime::LuaStr>>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        let parsed = parsed.0?;
        self.guarded_load(|vm| {
            let source = name.unwrap_or_else(|| vm.heap.intern(chunkname));
            let proto = vm.compile_text(parsed, source)?;
            Ok(vm.close_chunk(proto))
        })
    }

    /// The closure of a loaded main function over the globals.
    fn close_chunk(&mut self, proto: Gc<crate::runtime::Proto>) -> Gc<LuaClosure> {
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
        self.heap.new_closure(proto, ups.into_boxed_slice())
    }
}

/// A text chunk being loaded from a reader (see [`Vm::text_load`]).
#[doc(hidden)]
pub struct TextLoad {
    version: crate::version::LuaVersion,
    depth: u32,
    budget: usize,
    scratch: crate::frontend::parser::ParseScratch,
}

/// The outcome of [`TextLoad::parse`], for [`Vm::load_parsed`].
#[doc(hidden)]
pub struct ParsedText(Result<crate::frontend::parser::Parsed, SyntaxError>);

impl TextLoad {
    /// Parse the chunk whose first piece is `first`, calling `feed` for
    /// each further piece only when the parser moves past the end of what
    /// it has; `feed` appends a piece and returns true, or returns false at
    /// the end. Input past the loader's byte budget fails the load with
    /// "not enough memory", as [`Vm::load`] does.
    pub fn parse(self, first: Vec<u8>, feed: &mut dyn FnMut(&mut Vec<u8>) -> bool) -> ParsedText {
        let oom = || SyntaxError {
            line: 0,
            msg: b"not enough memory".to_vec(),
        };
        let budget = self.budget;
        if first.len() > budget {
            return ParsedText(Err(oom()));
        }
        // the source read so far lives in the vm's memory; each piece comes
        // through a buffer of its own first, as the reader hands it over
        let mut whole = LVec::new(self.scratch.mem());
        if let Err(e) = whole.extend_from_slice(&first) {
            return ParsedText(Err(e.into()));
        }
        drop(first);
        let mut over = false;
        let mut piece = Vec::new();
        let mut capped = |buf: &mut LVec<u8>| -> Result<bool, Oom> {
            piece.clear();
            // the reader runs Lua code, outside the load's unwinding
            let more = outside_load(|| feed(&mut piece));
            if buf.len() + piece.len() > budget {
                over = true;
                return Ok(false);
            }
            buf.extend_from_slice(&piece)?;
            Ok(more)
        };
        let mem = self.scratch.mem();
        let (version, depth, scratch) = (self.version, self.depth, self.scratch);
        let r = catch_load_oom(|| {
            crate::frontend::parser::parse_stream(whole, &mut capped, version, depth, scratch)
        });
        let r = match r {
            Ok(r) => r,
            Err(()) => {
                mem.ctx().raise_oom();
                Err(SyntaxError::memory())
            }
        };
        ParsedText(if over { Err(oom()) } else { r })
    }
}
