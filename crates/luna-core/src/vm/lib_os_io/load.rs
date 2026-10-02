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
            let mut chunkname = vec![b'@'];
            chunkname.extend_from_slice(n);
            (
                std::fs::read(String::from_utf8_lossy(n).as_ref()),
                chunkname,
            )
        }
        None => {
            let mut buf = Vec::new();
            let r = std::io::stdin().read_to_end(&mut buf).map(|_| buf);
            (r, b"=stdin".to_vec())
        }
    };
    // `errfile`: the name shown is the chunk name without its '@' / '='.
    let shown = String::from_utf8_lossy(&chunkname[1..]).into_owned();
    let src = match read {
        Ok(src) => src,
        Err(e) => {
            let msg = format!("cannot open {shown}: {}", os_error_text(&e));
            return Err(Value::Str(vm.heap.intern(msg.as_bytes())));
        }
    };
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
    load_chunk(vm, src, &chunkname, mode)
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
    // `checkmode` (ldo.c): the kind of chunk must be allowed by the mode.
    let binary = crate::vm::dump::is_binary_chunk(src);
    if let Some(mode) = mode
        && !mode.contains(if binary { &b'b' } else { &b't' })
    {
        let kind = if binary { "binary" } else { "text" };
        let msg = format!(
            "attempt to load a {kind} chunk (mode is '{}')",
            String::from_utf8_lossy(mode)
        );
        return Err(Value::Str(vm.heap.intern(msg.as_bytes())));
    }
    match vm.load(src, chunkname) {
        Ok(cl) => Ok(Value::Closure(cl)),
        Err(e) => Err(vm.load_error_value(&e, chunkname)),
    }
}

/// C `strerror` for an OS error: Rust renders it as
/// "<strerror text> (os error N)"; PUC prints the text alone.
fn os_error_text(e: &std::io::Error) -> String {
    let full = e.to_string();
    match (e.raw_os_error(), full.rfind(" (os error ")) {
        (Some(_), Some(at)) => full[..at].to_string(),
        _ => full,
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
                let env = a.get(vm, 2);
                let uv = vm.heap.new_upvalue(crate::runtime::UpvalState::Closed(env));
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { cl.as_mut() }.upvals_mut()[0] = uv;
            }
            Ok(vm.nat_return(fs, &[Value::Closure(cl)]))
        }
        Ok(other) => Ok(vm.nat_return(fs, &[other])),
        Err(msg) => Ok(vm.nat_return(fs, &[Value::Nil, msg])),
    }
}

/// `dofile([filename])`: a load failure is raised as is (`lua_error`, no
/// position added).
pub(super) fn nat_dofile(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::argcheck::{self, Args};
    let name = argcheck::opt_string(vm, Args::new(fs, nargs), 0)?;
    match load_path(vm, name.as_ref().map(|n| n.as_bytes()), None) {
        Ok(f) => {
            let results = vm.call_value(f, &[])?;
            Ok(vm.nat_return(fs, &results))
        }
        Err(msg) => Err(LuaError(msg)),
    }
}
