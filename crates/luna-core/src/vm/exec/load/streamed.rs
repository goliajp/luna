//! Loads whose text a reader hands over piece by piece.

use super::*;

impl Vm {
    /// Start loading a text chunk that a reader hands over piece by piece
    /// (`lua_load`, `load` with a function): what the parse needs from the
    /// vm, so that the reader can use the vm while the parse runs. `None`
    /// for MacroLua, whose macro pass needs the whole source first.
    pub(crate) fn text_load(&mut self) -> Option<TextLoad> {
        if self.version.is_macro_lua() {
            return None;
        }
        let mut scratch = self.parse_scratch.take();
        // a counted load records what its scanner makes
        scratch.record = self.heap.counting();
        Some(TextLoad {
            version: self.version,
            depth: self.g.nccalls,
            budget: self.loader_input_budget,
            scratch,
        })
    }

    /// 5.4 on, `lua_load` checks for a collection before it reads (5.1:
    /// `f_parser`, once the first character is read).
    pub(crate) fn lua_load_checkpoint(&mut self) -> Result<(), LuaError> {
        // 5.1's `f_parser` checks before it parses or undumps
        if self.version >= LuaVersion::Lua54 || self.version == LuaVersion::Lua51 {
            return self.native_checkpoint();
        }
        Ok(())
    }

    /// `lua_load` with a reader starts: the check made before it reads,
    /// once for the load whichever way it goes on; 5.2 on, the thread is
    /// not yieldable while the chunk is parsed (`incnny` in
    /// `luaD_protectedparser`).
    pub(crate) fn host_load_begin(&mut self) -> Result<(), LuaError> {
        let checked = self.lua_load_checkpoint();
        self.heap.set_load_checked(true);
        if self.version >= LuaVersion::Lua52 {
            self.nny += 1;
        }
        checked
    }

    /// The parse of the `lua_load` `Vm::host_load_begin` began has ended.
    pub(crate) fn host_load_end(&mut self) {
        self.heap.set_load_checked(false);
        if self.version >= LuaVersion::Lua52 {
            self.nny -= 1;
        }
    }

    /// Compile what [`TextLoad::parse`] read as the chunk `chunkname` and
    /// close it over the globals, as [`Vm::load`] does.
    pub(crate) fn load_parsed(
        &mut self,
        parsed: ParsedText,
        chunkname: &[u8],
        name: Option<Gc<crate::runtime::LuaStr>>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        self.load_parsed_spliced(parsed, chunkname, name, Vec::new())
    }

    /// [`Vm::load_parsed`] of a parse whose reader calls counted `splices`
    /// (each at the byte offset of the piece it read), set aside: they are
    /// counted again among what PUC's parser makes, in its order.
    pub(crate) fn load_parsed_spliced(
        &mut self,
        parsed: ParsedText,
        chunkname: &[u8],
        name: Option<Gc<crate::runtime::LuaStr>>,
        splices: Vec<(u32, crate::runtime::puc::Deferred)>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        self.heap.set_load_checked(false);
        let counted = match &parsed.0 {
            Ok(p) => p.recorded().is_some(),
            Err((e, Some(_))) => self.heap.counting() && !e.is_memory(),
            Err(_) => false,
        };
        let mut splices = splices;
        if !counted {
            // nothing of the parse is replayed: the reader's counts stand
            // where they are
            for (_, d) in splices.drain(..) {
                self.heap.replay_reader_call(d);
            }
        }
        self.guarded_load(|vm| {
            let mut parsed = match parsed.0 {
                Ok(p) => p,
                Err((e, Some((failed, src)))) if counted => {
                    return Err(vm.acct_streamed_failure(e, failed, &src, chunkname, name, splices));
                }
                Err((e, failed)) => {
                    let source = match name {
                        Some(n) => n,
                        None => vm.heap.try_intern(chunkname)?,
                    };
                    return Err(match failed {
                        Some((failed, src)) => {
                            vm.error_before(&src, Some(failed), source).unwrap_or(e)
                        }
                        None => e,
                    });
                }
            };
            if !counted {
                let source = match name {
                    Some(n) => n,
                    None => vm.heap.try_intern(chunkname)?,
                };
                let proto = vm.compile_text(parsed, source)?;
                return vm.close_chunk(proto);
            }
            // made without counting, then counted as PUC's parser makes it
            let span = vm.heap.load_begins();
            let building = vm.heap.build_load(true);
            let made = vm.compile_streamed(&mut parsed, chunkname, name);
            vm.heap.load_built(building);
            let cl = match made {
                Ok(cl) => cl,
                Err(e) => {
                    let r = match (&parsed.src, parsed.recorded()) {
                        (Some(src), Some(rec)) if !e.is_memory() => {
                            vm.acct_failed_compile(src, &e, &parsed.chunk, rec, splices)
                        }
                        _ => vm.splices_stand(splices),
                    };
                    vm.heap.load_ends(span);
                    return Err(r.err().unwrap_or(e));
                }
            };
            let src = parsed
                .src
                .take()
                .unwrap_or_else(|| LVec::new(vm.heap.mem()));
            let rec = parsed.recorded().expect("counted");
            let r = vm.acct_text_load(&src, &parsed.chunk, rec, cl, splices);
            vm.heap.load_ends(span);
            vm.parse_scratch = crate::frontend::parser::ParseScratch::recycle(parsed);
            r.map(|()| cl)
        })
    }

    /// The closure of a streamed parse, its chunk name a string of its own
    /// as `luaY_parser` makes it (5.2 on, a long one is not interned).
    fn compile_streamed(
        &mut self,
        parsed: &mut crate::frontend::parser::Parsed,
        chunkname: &[u8],
        name: Option<Gc<crate::runtime::LuaStr>>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        let source = self.heap.chunk_name(chunkname, name)?;
        self.compile_scratch.replay.seal = true;
        let made = self.compile_parsed(&parsed.chunk, &parsed.end_lines, source);
        self.compile_scratch.replay.seal = false;
        let proto = made.map_err(|e| match &parsed.src {
            // placed as PUC, which compiles while it reads, meets it
            Some(src) => {
                let (v, depth) = (self.version, self.g.nccalls);
                let s = &mut self.compile_scratch;
                crate::compiler::place_error(s, &parsed.chunk, src, v, depth, e)
            }
            None => e,
        })?;
        self.hold_puc_names(proto)?;
        self.close_chunk(proto)
    }

    /// The error of a counted load whose reader's text, `src`, failed to
    /// parse at `failed` with `e`, counted as PUC allocates it (see
    /// [`Vm::acct_failed_load`]); the reader's calls counted `splices`.
    fn acct_streamed_failure(
        &mut self,
        e: SyntaxError,
        failed: crate::frontend::parser::Failed,
        src: &[u8],
        chunkname: &[u8],
        name: Option<Gc<crate::runtime::LuaStr>>,
        splices: Vec<(u32, crate::runtime::puc::Deferred)>,
    ) -> SyntaxError {
        let (v, depth) = (self.version, self.g.nccalls);
        let span = self.heap.load_begins();
        let building = self.heap.build_load(true);
        let source = self
            .heap
            .chunk_name(chunkname, name)
            .map_err(SyntaxError::from);
        // the text read, parsed again for what its scanner makes
        let e = match &source {
            &Ok(source) => {
                // the reader's calls are placed by where the tokens end
                (self.parse_scratch.record, self.parse_scratch.record_ends) = (true, true);
                let _ =
                    crate::frontend::parser::parse_in_place(src, v, depth, &mut self.parse_scratch);
                self.parse_scratch.record_ends = false;
                self.parse_scratch.failed = Some(failed);
                self.error_before(src, Some(failed), source).unwrap_or(e)
            }
            Err(o) => o.clone(),
        };
        self.heap.load_built(building);
        let counted = match source {
            Ok(source) if !e.is_memory() => self.acct_failed_load(src, &e, source, splices),
            _ => self.splices_stand(splices),
        };
        self.parse_scratch.record = false;
        self.heap.load_ends(span);
        counted.err().unwrap_or(e)
    }
}
