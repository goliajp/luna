//! The base functions that read files: `loadfile` and `dofile`.

use super::*;

/// PUC `luaL_loadfilex`: compile the file `name` (stdin when `None`) and
/// return the function, or the message `loadfile` returns after its nil.
/// `mode` limits the chunk to text and/or binary (`None` allows both).
pub(crate) fn load_path(
    vm: &mut Vm,
    name: Option<&[u8]>,
    mode: Option<&[u8]>,
) -> Result<Value, Value> {
    let (read, chunkname) = match name {
        Some(n) => {
            crate::vm::lib_io::reset_errno(vm);
            let mut chunkname = vec![b'@'];
            chunkname.extend_from_slice(n);
            let read = match crate::vm::lib_io::open_file(vm.crt_text, n, b"r") {
                Ok(mut o) => {
                    let mut buf = Vec::new();
                    o.file
                        .read_to_end(&mut buf)
                        .map(|_| buf)
                        .map_err(|e| ("read", e))
                }
                Err(e) => Err(("open", e)),
            };
            (read, chunkname)
        }
        None => match crate::vm::lib_io::read_stdin_chunk(vm) {
            // through the C library's `stdin`, as `getF` reads it
            Some(src) => (Ok(src), b"=stdin".to_vec()),
            None => {
                let mut buf = Vec::new();
                let r = std::io::stdin().read_to_end(&mut buf);
                (r.map(|_| buf).map_err(|e| ("read", e)), b"=stdin".to_vec())
            }
        },
    };
    // PUC pushes the chunk name before it opens the file, a push that
    // checks for a collection; the name stays on the stack while it loads
    let base = vm.lib_roots.mark();
    let made = with_rules!(vm, P => vm.api_pushing::<P>(|vm| match name {
        Some(n) => vm.pushfstring(b"@%s", &[crate::runtime::puc::fmt::FmtArg::S(n)]),
        None => vm.alloc_str(&chunkname),
    }));
    let r = match made {
        Ok(name_str) => load_read(vm, read, &chunkname, name_str, name.is_some(), mode),
        Err(e) => Err(e.0),
    };
    vm.lib_roots.release(base);
    r
}

/// [`load_path`] for a chunk read from standard input, given its text.
pub(crate) fn load_stdin_text(vm: &mut Vm, src: Vec<u8>) -> Result<Value, Value> {
    let name_str = vm
        .heap
        .try_intern(b"=stdin")
        .map_err(|e| LuaError::from(e).0)?;
    load_read(vm, Ok(src), b"=stdin", name_str, false, None)
}

/// The rest of [`load_path`]: what was read (or how reading failed) under
/// `chunkname`, already the heap string `name_str`.
fn load_read(
    vm: &mut Vm,
    read: Result<Vec<u8>, (&str, std::io::Error)>,
    chunkname: &[u8],
    name_str: Gc<crate::runtime::LuaStr>,
    file: bool,
    mode: Option<&[u8]>,
) -> Result<Value, Value> {
    // `errfile`: the name shown is the chunk name without its '@' / '='.
    let shown = String::from_utf8_lossy(&chunkname[1..]).into_owned();
    let src = match read {
        // `luaL_loadfilex` reads a text chunk through a stream in text mode
        Ok(src) if !crate::vm::dump::is_binary_chunk(&src) && file && vm.crt_text => {
            crate::vm::lib_io::translate_all(&src)
        }
        Ok(src) => src,
        Err((what, e)) => {
            crate::vm::lib_io::note_failure(&e);
            let text = crate::vm::lib_io::strerror(&e);
            let msg = vm.fmt_str(format_args!("cannot {what} {shown}: {text}"));
            return Err(msg.map_or_else(|e| e.0, Value::Str));
        }
    };
    // "no useful error number until here"
    crate::vm::lib_io::reset_errno(vm);
    let src = crate::frontend::lexer::Lexer::strip_shebang_bom(&src);
    // PUC `luaL_loadfilex`: when a `#` comment line precedes a binary
    // chunk, the leading line-terminator left by the comment skip is
    // dropped so undump sees a clean `\x1bLua…` head (files.lua :594).
    let src: &[u8] = match src {
        [b'\n', rest @ ..] | [b'\r', b'\n', rest @ ..] | [b'\r', rest @ ..]
            if rest.first() == Some(&0x1b) =>
        {
            rest
        }
        _ => src,
    };
    load_named_chunk(vm, src, chunkname, Some(name_str), mode)
}

/// PUC `luaL_loadbufferx`: compile `src` under `chunkname`, the chunk kind
/// limited by `mode` as in `load_path`; a syntax error comes back as its
/// positioned message.
pub(crate) fn load_chunk(
    vm: &mut Vm,
    src: &[u8],
    chunkname: &[u8],
    mode: Option<&[u8]>,
) -> Result<Value, Value> {
    load_named_chunk(vm, src, chunkname, None, mode)
}

/// [`load_chunk`] with the chunk name already a heap string (`name`).
fn load_named_chunk(
    vm: &mut Vm,
    src: &[u8],
    chunkname: &[u8],
    name: Option<Gc<crate::runtime::LuaStr>>,
    mode: Option<&[u8]>,
) -> Result<Value, Value> {
    // `checkmode` (ldo.c): the kind of chunk must be allowed by the mode.
    let binary = crate::vm::dump::is_binary_chunk(src);
    if let Some(mode) = mode
        && !mode.contains(if binary { &b'b' } else { &b't' })
    {
        let kind: &[u8] = if binary { b"binary" } else { b"text" };
        use crate::runtime::puc::fmt::FmtArg;
        if let Err(e) = vm.parser_anchor_55() {
            return Err(e.0);
        }
        let msg = vm.pushfstring(
            b"attempt to load a %s chunk (mode is '%s')",
            &[FmtArg::S(kind), FmtArg::S(mode)],
        );
        // `luaD_protectedparser`'s `luaD_pcall` caught it
        vm.shrink_caught_error();
        return Err(msg.map_or_else(|e| e.0, Value::Str));
    }
    match vm.load_named(src, chunkname, name) {
        Ok(cl) => Ok(Value::Closure(cl)),
        Err(e) => Err(vm.load_error_value(&e, chunkname)),
    }
}

/// `loadfile([filename [, mode [, env]]])`; 5.1 takes the filename only.
pub(crate) fn nat_loadfile(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::version::LuaVersion;
    use crate::vm::argcheck::{self, Args};
    let a = Args::new(fs, nargs);
    let name = argcheck::opt_string(vm, a, 0)?;
    let mode = if vm.version() >= LuaVersion::Lua52 {
        argcheck::opt_string(vm, a, 1)?
    } else {
        None
    };
    // 5.5 `getMode`: Lua code cannot ask for a fixed-buffer ('B') chunk.
    if vm.version() >= LuaVersion::Lua55 && mode.is_some_and(|m| m.as_bytes().contains(&b'B')) {
        return Err(arg_error(vm, 2, "invalid mode"));
    }
    match load_path(
        vm,
        name.as_ref().map(|n| n.as_bytes()),
        mode.as_ref().map(|m| m.as_bytes()),
    ) {
        Ok(Value::Closure(cl)) => {
            // `load_aux`: a given env (even nil) becomes the first upvalue,
            // when the function has one.
            if vm.version() >= LuaVersion::Lua52 && !a.is_none(2) && !cl.upvals().is_empty() {
                // the upvalue the load made gets the value: no new one
                let env = a.get(vm, 2);
                vm.upvalue_set_value(cl, 0, env);
            }
            vm.nat_ret(fs, &[Value::Closure(cl)])
        }
        Ok(other) => vm.nat_ret(fs, &[other]),
        Err(msg) => vm.nat_ret(fs, &[Value::Nil, msg]),
    }
}

/// `dofile([filename])`: a load failure is raised as is (`lua_error`, no
/// position added). From 5.2 the chunk may yield (PUC calls it with
/// `lua_callk`), and the resume finishes `dofile` with the chunk's results.
pub(crate) fn nat_dofile(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::argcheck::{self, Args};
    let name = argcheck::opt_string(vm, Args::new(fs, nargs), 0)?;
    if vm.version() >= crate::version::LuaVersion::Lua52 {
        vm.native_settop(1);
    }
    match load_path(vm, name.as_ref().map(|n| n.as_bytes()), None) {
        Ok(f) => {
            let results = if vm.version() >= crate::version::LuaVersion::Lua52 {
                vm.call_value_k(f, &[])?
            } else {
                vm.call_value(f, &[])?
            };
            vm.nat_ret(fs, &results)
        }
        Err(msg) => {
            // the message `luaL_loadfile` pushed
            vm.native_push(1);
            Err(LuaError(msg))
        }
    }
}
