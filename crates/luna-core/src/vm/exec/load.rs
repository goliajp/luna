//! `load`: a chunk parsed and compiled (or undumped) into a closure over
//! the globals table.

use super::*;
use crate::runtime::mem::{LVec, Oom, catch_load_oom, outside_load};

mod compile;
mod streamed;
mod text;
pub use text::{ParsedText, TextLoad};

/// The memory error of a load whose frontend ran out of memory.
#[cold]
fn load_oom(heap: &crate::runtime::Heap) -> SyntaxError {
    heap.mem_ctx().raise_oom();
    SyntaxError::memory()
}

impl Vm {
    /// Parse + compile a chunk and close it over the globals table. It
    /// does not collect: the closures loaded before and held only in Rust
    /// values stay valid until a call or an `eval` runs.
    pub fn load(&mut self, src: &[u8], chunkname: &[u8]) -> Result<Gc<LuaClosure>, SyntaxError> {
        self.heap.puc.load_checked = true;
        // the parser's checks for a collection do not collect either
        let held = std::mem::replace(&mut self.heap.puc.load_holds, true);
        let r = self.load_named(src, chunkname, None);
        self.heap.puc.load_holds = held;
        r
    }

    /// [`Vm::load`] with the chunk name already a heap string (`name`,
    /// whose bytes are `chunkname`), which the functions then share.
    pub(crate) fn load_named(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
        name: Option<Gc<crate::runtime::LuaStr>>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        // the chunk's name is on the C function's stack meanwhile
        let base = self.lib_roots.mark();
        self.lib_roots.extend(name.map(Value::Str));
        let r = self.load_named_rooted(src, chunkname, name);
        self.lib_roots.release(base);
        r
    }

    /// The load fails with `e`, an error a collection check inside it
    /// raised: kept for the load's caller to report as it is.
    pub(super) fn load_failed_by(&mut self, e: LuaError) -> SyntaxError {
        let text = match e.0 {
            Value::Str(s) => s.as_bytes().to_vec(),
            _ => Vec::new(),
        };
        self.load_raised = Some(e);
        SyntaxError::unpositioned(&text[..])
    }

    fn load_named_rooted(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
        name: Option<Gc<crate::runtime::LuaStr>>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        // 5.4 on, `lua_load` checks for a collection before it parses, as
        // 5.1's `f_parser` does
        // (5.1's runs inside the load's protected call: an error there is
        // the load's)
        if !std::mem::take(&mut self.heap.puc.load_checked)
            && let Err(e) = self.lua_load_checkpoint()
        {
            return Err(self.load_failed_by(e));
        }
        // the parser and the compiler cannot collect (what they made so far
        // is held only by them), so a load that ran out of memory is run
        // once more after an emergency collection, as PUC's allocations
        // inside the parser retry; the source is all in `src`
        match self.guarded_load(|vm| vm.load_named_inner(src, chunkname, name)) {
            Err(e) if e.is_memory() && src.len() <= self.loader_input_budget => {
                if !self.emergency_collect() {
                    return Err(e);
                }
                self.guarded_load(|vm| vm.load_named_inner(src, chunkname, name))
            }
            r => r,
        }
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
            if !self.heap.counting() {
                crate::vm::dump::undump_named(
                    src,
                    &mut self.heap,
                    self.version,
                    allow_puc,
                    chunkname,
                )
                .map_err(SyntaxError::unpositioned)?
            } else {
                // made without counting, then counted as PUC's undumper
                // makes it
                let off = self.heap.counting_off();
                let v = self.version;
                let made =
                    crate::vm::dump::undump_named(src, &mut self.heap, v, allow_puc, chunkname)
                        .map_err(SyntaxError::unpositioned)
                        .and_then(|p| self.hold_puc_names(p).map(|()| p))
                        .and_then(|p| self.close_chunk(p));
                self.heap.counting_on(off);
                let cl = made?;
                self.acct_undump(src, cl)?;
                return Ok(cl);
            }
        } else if self.version.is_macro_lua() {
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
            self.compile_parsed(&parsed.chunk, &parsed.end_lines, source)?
        } else {
            // what the parser and the compiler make is counted afterwards,
            // in the order PUC's parser makes it; that includes its own
            // string of the chunk name (`Heap::chunk_name`)
            let counted = self.heap.counting();
            self.parse_scratch.record = counted;
            let off = self.heap.counting_off();
            let base = self.heap.unborn_len();
            let aside = self.heap.set_aside_loads(counted);
            let source = self
                .heap
                .chunk_name(chunkname, name)
                .map_err(SyntaxError::from);
            let made = source
                .clone()
                .and_then(|source| self.compile_text_in_place(src, source))
                .and_then(|p| self.hold_puc_names(p).map(|()| p))
                .and_then(|p| Ok((p, self.close_chunk(p)?)));
            self.heap.set_aside_loads(aside);
            self.heap.counting_on(off);
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
            // what PUC did not make (luna's own, or the garbage of a load
            // that failed) is on the list now
            self.heap.flush_unborn(base);
            return out;
        };
        self.close_chunk(proto)
    }

    /// Run `f`, a load, so that running out of memory anywhere in it (the
    /// parser, the compiler, the objects they make) is the memory error.
    /// What the load built is dropped on the way out, and the compiler's
    /// vectors, left half filled, are replaced.
    fn guarded_load<R>(
        &mut self,
        f: impl FnOnce(&mut Vm) -> Result<R, SyntaxError>,
    ) -> Result<R, SyntaxError> {
        let (counting, frames) = (self.heap.counting_mark(), self.heap.puc.parse.len());
        let (aside, unborn) = (self.heap.puc.unborn_on, self.heap.unborn_len());
        match catch_load_oom(|| f(self)) {
            Ok(r) => r,
            Err(()) => {
                // the failure unwound past what stops counting for the load,
                // and past a step's frame of the parser's objects
                self.heap.restore_counting(counting);
                self.heap.puc.parse.truncate(frames);
                self.heap.set_aside_loads(aside);
                self.heap.flush_unborn(unborn);
                self.compile_scratch = crate::compiler::CompileScratch::new(self.heap.mem());
                Err(load_oom(&self.heap))
            }
        }
    }

    /// The closure of a loaded main function over the globals.
    pub(in crate::vm::exec) fn close_chunk(
        &mut self,
        proto: Gc<crate::runtime::Proto>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        // the new prototype is held only here until the closure holds it, so
        // these allocations fail without an emergency collection
        if self.heap.track_chunk_roots {
            self.heap.chunk_roots.push(proto);
        }
        // PUC `lua_load` (lapi.c) only seeds the loaded closure's first
        // upvalue with the globals table when the closure has *exactly* one
        // upvalue — that's the main-chunk `_ENV` case. A dumped non-main
        // function with two-or-more upvalues keeps every cell at nil; the
        // host must use `debug.setupvalue` to wire them up. 5.2 calls.lua
        // :293's `assert(x() == nil)` pins this contract. A synthetic main
        // chunk with no declared upvalues still gets one cell, so the host can
        // probe it via `debug.upvalueid`.
        let n = proto.upvals.len();
        let first = if n <= 1 {
            Value::Table(self.globals)
        } else {
            Value::Nil
        };
        let mut ups = crate::runtime::mem::LVec::with_capacity(self.heap.mem(), n.max(1))?;
        for _ in 0..n.max(1) {
            ups.push(self.heap.try_new_upvalue(UpvalState::Closed(first))?)?;
        }
        Ok(self.heap.try_new_closure(proto, &ups)?)
    }
}
