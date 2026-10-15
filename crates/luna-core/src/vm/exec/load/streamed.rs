//! Loads whose text a reader hands over piece by piece.

use super::*;

impl Vm {
    /// Start loading a text chunk that a reader hands over piece by piece
    /// (`lua_load`, `load` with a function): what the parse needs from the
    /// vm, so that the reader can use the vm while the parse runs. `None`
    /// for MacroLua, whose macro pass needs the whole source first.
    #[doc(hidden)]
    pub fn text_load(&mut self) -> Option<TextLoad> {
        if self.version.is_macro_lua() {
            return None;
        }
        let mut scratch = self.parse_scratch.take();
        // a counted load records what its scanner makes
        scratch.record = self.heap.puc.silent == 0;
        Some(TextLoad {
            version: self.version,
            depth: self.g.nccalls,
            budget: self.loader_input_budget,
            scratch,
        })
    }

    /// 5.4 on, `lua_load` checks for a collection before it reads (5.1:
    /// `f_parser`, once the first character is read).
    pub(crate) fn lua_load_checkpoint(&mut self) {
        // 5.1's `f_parser` checks before it parses or undumps
        if self.version >= LuaVersion::Lua54 || self.version == LuaVersion::Lua51 {
            self.native_checkpoint();
        }
    }

    /// `lua_load` with a reader starts: the check made before it reads,
    /// once for the load whichever way it goes on; 5.2 on, the thread is
    /// not yieldable while the chunk is parsed (`incnny` in
    /// `luaD_protectedparser`).
    #[doc(hidden)]
    pub fn host_load_begin(&mut self) {
        self.lua_load_checkpoint();
        self.heap.puc.load_checked = true;
        if self.version >= LuaVersion::Lua52 {
            self.nny += 1;
        }
    }

    /// The parse of the `lua_load` [`Vm::host_load_begin`] began has ended.
    #[doc(hidden)]
    pub fn host_load_end(&mut self) {
        self.heap.puc.load_checked = false;
        if self.version >= LuaVersion::Lua52 {
            self.nny -= 1;
        }
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
        splices: Vec<(u32, crate::runtime::heap::Deferred)>,
    ) -> Result<Gc<LuaClosure>, SyntaxError> {
        self.heap.puc.load_checked = false;
        let counted = matches!(&parsed.0, Ok(p) if p.recorded().is_some());
        let mut splices = splices;
        if !counted {
            // nothing of the parse is replayed: the reader's counts stand
            // where they are
            for (_, d) in splices.drain(..) {
                self.heap.defer_apply(d);
            }
        }
        self.guarded_load(|vm| {
            let mut parsed = match parsed.0 {
                Ok(p) => p,
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
            vm.heap.puc.silent += 1;
            let made = vm.compile_streamed(&mut parsed, chunkname, name);
            vm.heap.puc.silent -= 1;
            let cl = made?;
            let src = parsed
                .src
                .take()
                .unwrap_or_else(|| LVec::new(vm.heap.mem()));
            let rec = parsed.recorded().expect("counted");
            let r = vm.acct_text_load(&src, &parsed.chunk, rec, cl, splices);
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
        let long = chunkname.len() > crate::runtime::string::MAX_SHORT_LEN
            && self.heap.table_dialect != crate::runtime::table::Dialect::L51;
        let source = match name {
            Some(n) if !long => n,
            _ => self.heap.try_intern(chunkname)?,
        };
        let proto = self
            .compile_parsed(&parsed.chunk, &parsed.end_lines, source)
            .map_err(|e| match &parsed.src {
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
}
