//! The interpreter state and the error handling `lua.c` wraps around it.

use super::*;

/// The interpreter: the state `lua.c` keeps around its `lua_State`.
pub(crate) struct Interp {
    pub(crate) vm: Vm,
    /// `progname`: `argv[0]`; none while the REPL runs.
    pub(crate) progname: Option<String>,
}

impl Interp {
    pub(crate) fn version(&self) -> LuaVersion {
        self.vm.version()
    }

    /// `lua.c`'s `l_message`.
    pub(crate) fn message(&self, msg: &[u8]) {
        let mut line = Vec::new();
        if let Some(p) = &self.progname {
            line.extend_from_slice(p.as_bytes());
            line.extend_from_slice(b": ");
        }
        // printed with "%s": a C string ends at its first NUL
        let msg = msg.split(|&b| b == 0).next().unwrap_or_default();
        line.extend_from_slice(msg);
        line.push(b'\n');
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(&line); // nowhere left to report a failed write
    }

    /// `lua.c`'s `report` of each dialect, for a chunk that failed with
    /// `err` (the message handler's result, or the load error).
    pub(crate) fn report(&mut self, err: Value) {
        let v = self.version();
        // 5.1 and 5.2 print nothing for a nil error object
        if err.is_nil() && v <= LuaVersion::Lua52 {
            return;
        }
        let msg = match lua_tostring(&mut self.vm, err) {
            Some(m) => m,
            None => match v {
                LuaVersion::Lua51 | LuaVersion::Lua52 => b"(error object is not a string)".to_vec(),
                // 5.3 hands printf a NULL string, which the C libraries luna
                // is compared against (glibc, macOS) print as "(null)"
                LuaVersion::Lua53 => b"(null)".to_vec(),
                _ => b"(error message not a string)".to_vec(),
            },
        };
        self.message(&msg);
    }

    /// `lua.c`'s `docall`: call `f` with `args` under the message handler.
    pub(crate) fn docall(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, Value> {
        let msgh = self.vm.native(msghandler);
        // lua.c makes the call from inside `pmain`, the C function the
        // whole interpreter runs in: a traceback ends with that level
        self.vm
            .call_value_with_handler_in_c(f, args, msgh)
            .map_err(|e| e.0)
    }

    /// `dochunk`: run a loaded chunk, reporting a failure to load or run it.
    /// The values it returned when it ran to the end.
    fn dochunk(&mut self, loaded: Result<Value, LuaError>, args: &[Value]) -> Option<Vec<Value>> {
        let result = match loaded {
            Ok(f) => self.docall(f, args),
            Err(e) => Err(e.0),
        };
        result.map_err(|e| self.report(e)).ok()
    }

    /// `dostring`: a chunk from the command line or the environment. 5.5
    /// takes text only.
    fn dostring(&mut self, src: &[u8], chunkname: &[u8]) -> Option<Vec<Value>> {
        let mode = (self.version() >= LuaVersion::Lua55).then_some(&b"t"[..]);
        let loaded = self.vm.load_buffer(src, chunkname, mode);
        self.dochunk(loaded, &[])
    }

    /// `dofile`: a file, or stdin when `name` is `None`.
    pub(crate) fn dofile(&mut self, name: Option<&[u8]>) -> Option<Vec<Value>> {
        let loaded = self.vm.load_file(name, None);
        self.dochunk(loaded, &[])
    }

    /// `handle_luainit`: run `LUA_INIT` (from 5.2 on `LUA_INIT_5_x` first),
    /// a chunk or, after an `@`, a file to run. False when it failed.
    pub(crate) fn handle_luainit(&mut self) -> bool {
        let versioned = match self.version() {
            LuaVersion::Lua51 => None,
            LuaVersion::Lua52 => Some("LUA_INIT_5_2"),
            LuaVersion::Lua53 => Some("LUA_INIT_5_3"),
            LuaVersion::Lua54 | LuaVersion::MacroLua => Some("LUA_INIT_5_4"),
            LuaVersion::Lua55 => Some("LUA_INIT_5_5"),
        };
        let found = versioned
            .into_iter()
            .chain(["LUA_INIT"])
            .find_map(|name| std::env::var_os(name).map(|init| (name, init)));
        let Some((name, init)) = found else {
            return true;
        };
        let init = os_bytes(init);
        let done = match init.strip_prefix(b"@") {
            Some(file) => self.dofile(Some(file)),
            None => self.dostring(&init, format!("={name}").as_bytes()),
        };
        done.is_some()
    }

    /// `dolibrary`: `-l name`, `require(module)`, whose result 5.2 on store
    /// in a global. From 5.4 on, `g=mod` names the global, and without it a
    /// `-suffix` of the module name is left out of the global's.
    fn dolibrary(&mut self, spec: &str) -> bool {
        let (global, module) = match spec.split_once('=') {
            Some((g, m)) if self.version() >= LuaVersion::Lua54 => (g, m),
            _ if self.version() >= LuaVersion::Lua54 => {
                (spec.split('-').next().unwrap_or_default(), spec)
            }
            _ => (spec, spec),
        };
        let require = self.vm.globals().get(self.str_value("require"));
        let name = self.str_value(module);
        match self.docall(require, &[name]) {
            Ok(_) if self.version() == LuaVersion::Lua51 => true,
            Ok(vals) => {
                let v = vals.first().copied().unwrap_or(Value::Nil);
                // the globals table is not ours to refuse; a failure here
                // would be luna's own
                self.vm.set_global(global, v).expect("set the -l global");
                true
            }
            Err(e) => {
                self.report(e);
                false
            }
        }
    }

    pub(crate) fn str_value(&mut self, s: &str) -> Value {
        Value::Str(self.vm.heap.intern(s.as_bytes()))
    }

    /// `createargtable` / `getargs`: `arg[0]` is the script (`argv[0]` when
    /// there is none), the script's arguments count up from 1 and what
    /// comes before it down from -1.
    pub(crate) fn set_arg(&mut self, argv: &[String], script: usize) {
        let t = self.vm.heap.new_table();
        for (i, a) in argv.iter().enumerate() {
            let k = Value::Int(i as i64 - script as i64);
            let v = self.str_value(a);
            // SAFETY: CLI driver — `t` was just allocated and is reachable
            // only from here until it is stored as `arg`.
            unsafe { t.as_mut() }
                .set(&mut self.vm.heap, k, v)
                .expect("integer keys are valid table keys");
        }
        self.vm
            .set_global("arg", Value::Table(t))
            .expect("set the arg global");
    }

    /// `handle_script`: load the script (stdin for `-` unless it follows
    /// `--`) and run it with its arguments.
    pub(crate) fn handle_script(&mut self, argv: &[String], script: usize) -> bool {
        let v = self.version();
        if v <= LuaVersion::Lua52 {
            self.set_arg(argv, script);
        }
        let stdin = argv[script] == "-" && argv[script - 1] != "--";
        let name = (!stdin).then(|| argv[script].as_str());
        // 5.2 on load either kind of chunk; 5.1 had no mode
        let f = match self.vm.load_file(name.map(str::as_bytes), None) {
            Ok(f) => f,
            Err(e) => {
                self.report(e.0);
                return false;
            }
        };
        let args = if v <= LuaVersion::Lua52 {
            argv[script + 1..]
                .iter()
                .map(|a| self.str_value(a))
                .collect()
        } else {
            match self.pushargs() {
                Ok(args) => args,
                Err(msg) => {
                    self.report(msg);
                    return false;
                }
            }
        };
        self.dochunk(Ok(f), &args).map(show).is_some()
    }

    /// 5.3's `pushargs`: the script's arguments are `arg[1..#arg]`, as they
    /// are after `-e` / `-l` ran.
    fn pushargs(&mut self) -> Result<Vec<Value>, Value> {
        let arg = self.vm.globals().get(self.str_value("arg"));
        let Value::Table(t) = arg else {
            return Err(self.str_value("'arg' is not a table"));
        };
        Ok((1..=t.len()).map(|i| t.get(Value::Int(i))).collect())
    }

    /// `runargs`: the `-e`, `-l` and (5.4 on) `-W` options before `optlim`,
    /// in order; false when one failed.
    pub(crate) fn runargs(&mut self, argv: &[String], optlim: usize) -> bool {
        let mut i = 1;
        while i < optlim {
            let a = &argv[i];
            match a.as_bytes()[1] {
                o @ (b'e' | b'l') => {
                    let extra = if a.len() > 2 {
                        a[2..].to_string()
                    } else {
                        i += 1;
                        argv[i].clone()
                    };
                    let ok = if o == b'e' {
                        self.dostring(extra.as_bytes(), b"=(command line)")
                            .map(show)
                            .is_some()
                    } else {
                        self.dolibrary(&extra)
                    };
                    if !ok {
                        return false;
                    }
                }
                b'W' if self.version() >= LuaVersion::Lua54 => {
                    let warn = self.vm.globals().get(self.str_value("warn"));
                    let on = self.str_value("@on");
                    self.vm
                        .call_value(warn, &[on])
                        .expect("warn(\"@on\") only switches the warning state");
                }
                _ => {}
            }
            i += 1;
        }
        true
    }
}

/// luna's addition to `lua.c`: the values a chunk returned, printed.
pub(crate) fn show(vals: Vec<Value>) {
    for v in vals {
        luna_core::stdio::write_stdout(format!("=> {}\n", render(v)).as_bytes());
    }
}

/// An environment variable's value as the C library hands it over.
fn os_bytes(s: std::ffi::OsString) -> Vec<u8> {
    #[cfg(unix)]
    {
        std::os::unix::ffi::OsStringExt::into_vec(s)
    }
    #[cfg(not(unix))]
    {
        s.to_string_lossy().into_owned().into_bytes()
    }
}

/// `lua_tostring` of an error object: strings, and numbers converted; None
/// for anything else.
pub(crate) fn lua_tostring(vm: &mut Vm, v: Value) -> Option<Vec<u8>> {
    match v {
        Value::Str(s) => Some(s.as_bytes().to_vec()),
        Value::Int(_) | Value::Float(_) => Some(vm.error_display(&LuaError(v)).into_bytes()),
        _ => None,
    }
}

/// `lua.c`'s message handler of each dialect: the error message with a
/// traceback of where it was raised (level 1 skips the handler itself).
fn msghandler(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let err = vm.nat_arg(fs, nargs, 0);
    let out = match vm.version() {
        LuaVersion::Lua51 => traceback_51(vm, err)?,
        LuaVersion::Lua52 => match lua_tostring(vm, err) {
            Some(msg) => traceback(vm, &msg),
            None if err.is_nil() => err,
            // `luaL_callmeta`: whatever `__tostring` returns
            None => match vm.metafield(err, "__tostring") {
                Value::Nil => Value::Str(vm.heap.intern(b"(no error message)")),
                mm => vm
                    .call_value(mm, &[err])?
                    .first()
                    .copied()
                    .unwrap_or(Value::Nil),
            },
        },
        _ => match lua_tostring(vm, err) {
            Some(msg) => traceback(vm, &msg),
            None => {
                let mm = vm.metafield(err, "__tostring");
                let text = if mm.is_nil() {
                    None
                } else {
                    vm.call_value(mm, &[err])?.first().copied()
                };
                match text {
                    // a string from `__tostring` is the message, as it is
                    Some(s @ Value::Str(_)) => s,
                    _ => {
                        let msg = format!("(error object is a {} value)", err.type_name());
                        traceback(vm, msg.as_bytes())
                    }
                }
            }
        },
    };
    Ok(vm.nat_return(fs, &[out]))
}

/// `luaL_traceback(L, L, msg, 1)` from the message handler.
fn traceback(vm: &mut Vm, msg: &[u8]) -> Value {
    let tb = vm.traceback(Some(msg), 1);
    Value::Str(vm.heap.intern(&tb))
}

/// 5.1 lua.c's `traceback`: a string (or number) message goes through the
/// global `debug.traceback(msg, 2)` when there is one; anything else, or no
/// such function, leaves the message as it is.
fn traceback_51(vm: &mut Vm, err: Value) -> Result<Value, LuaError> {
    if !matches!(err, Value::Str(_) | Value::Int(_) | Value::Float(_)) {
        return Ok(err);
    }
    let key = Value::Str(vm.heap.intern(b"debug"));
    let Value::Table(debug) = vm.globals().get(key) else {
        return Ok(err);
    };
    let key = Value::Str(vm.heap.intern(b"traceback"));
    let tb = debug.get(key);
    if !matches!(tb, Value::Closure(_) | Value::Native(_)) {
        return Ok(err);
    }
    Ok(vm
        .call_value(tb, &[err, Value::Int(2)])?
        .first()
        .copied()
        .unwrap_or(Value::Nil))
}
