//! A Vm set up as PUC's standalone interpreter `lua` (`lua.c`) sets its
//! state up, for a host that runs scripts the way `lua` does and wants
//! the same allocations, collections and call levels.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::{LuaError, Vm};

/// The options of `lua.c` that change how it opens the libraries.
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct StandaloneOptions {
    /// `lua -E`: the package library ignores `LUA_PATH` and `LUA_CPATH`
    /// and the registry holds `LUA_NOENV` (5.2 on; 5.1 has no `-E`).
    pub ignore_env: bool,
}

impl StandaloneOptions {
    /// The options with `-E` set or not.
    pub fn ignore_env(mut self, on: bool) -> Self {
        self.ignore_env = on;
        self
    }
}

/// A Vm in the state `lua.c` is in inside its `pmain`, its libraries
/// open: made by [`Vm::into_standalone`].
pub struct Standalone {
    vm: Vm,
    argv: Vec<Vec<u8>>,
    script: Option<usize>,
    /// the number of the script's arguments, once the `arg` table is made
    args: Option<usize>,
}

impl Vm {
    /// The state PUC's standalone interpreter `lua` (`lua.c`) has when it
    /// starts running anything, made as `lua.c` makes it: `main` calls
    /// `pmain` protected (5.1 `lua_cpcall` with a C closure and `argv`,
    /// later `lua_pcall` with `argc` and `argv`), and `pmain` opens the
    /// libraries; from 5.3 it then makes the `arg` table, from 5.4 it
    /// restarts the collector in generational mode. `argv` is the
    /// program's arguments, `argv[0]` its name; `script` the index of the
    /// script among them, if there is one.
    ///
    /// Compared with [`Vm::open_all_libs`]:
    ///
    /// - The count of the collector holds what `lua.c` allocates first:
    ///   5.1's C closure for `pmain`, from 5.2 one more call level (5.2
    ///   checks for a collection before `pmain` runs).
    /// - Everything runs one call level down, inside `pmain`: recursion
    ///   limits and "C stack overflow" fall where they fall in `lua`, and a
    ///   traceback ends with that level.
    /// - The collector is stopped while the libraries open (5.3's is not);
    ///   5.1 and 5.2 restart it after them, 5.4 and 5.5 once `arg` is made,
    ///   in generational mode.
    ///
    /// 5.1 and 5.2 make `arg` later, when `lua` runs the script:
    /// [`Standalone::script_args`].
    ///
    /// # Errors
    ///
    /// An error of a finalizer that 5.2's collection check before `pmain`
    /// runs, or a memory error making `arg` (from 5.3) that its emergency
    /// collection does not cure. `lua.c` ends the process then; here the
    /// Vm, taken by value, is dropped with the error (no half-made state
    /// comes back).
    ///
    /// # Panics
    ///
    /// When `argv` is empty or `script` is not an index of it.
    ///
    /// # Example
    ///
    /// ```
    /// use luna_core::runtime::Value;
    /// use luna_core::version::LuaVersion;
    /// use luna_core::vm::{StandaloneOptions, Vm};
    ///
    /// let argv = ["lua", "script.lua", "a", "b"];
    /// let int = |v: &Value| match v {
    ///     Value::Int(n) => *n,
    ///     _ => panic!("an integer"),
    /// };
    ///
    /// // 5.4 (`pmain` gets `argc` and `argv`): `arg` is made with the state
    /// let mut s = Vm::new_minimal(LuaVersion::Lua54)
    ///     .into_standalone(&argv, Some(1), StandaloneOptions::default())
    ///     .unwrap();
    /// let r = s.vm_mut().eval("return #arg, arg[0] == 'script.lua', arg[-1] == 'lua'").unwrap();
    /// assert_eq!(int(&r[0]), 2);
    /// assert!(matches!(r[1..], [Value::Bool(true), Value::Bool(true)]));
    /// // made already: the same number, nothing made again
    /// assert_eq!(s.script_args().unwrap(), 2);
    /// assert_eq!(s.script_args().unwrap(), 2);
    ///
    /// // 5.1 (`lua_cpcall`): `arg` only once the script is about to run
    /// let mut s = Vm::new_minimal(LuaVersion::Lua51)
    ///     .into_standalone(&argv, Some(1), StandaloneOptions::default())
    ///     .unwrap();
    /// assert!(matches!(s.vm_mut().eval("return arg").unwrap()[..], [Value::Nil]));
    /// assert_eq!(s.script_args().unwrap(), 2);
    /// assert_eq!(s.script_args().unwrap(), 2);
    /// let r = s.vm_mut().eval("return #arg, arg[0] == 'script.lua'").unwrap();
    /// assert_eq!(int(&r[0]), 2);
    /// assert!(matches!(r[1], Value::Bool(true)));
    /// ```
    pub fn into_standalone(
        mut self,
        argv: &[impl AsRef<[u8]>],
        script: Option<usize>,
        opts: StandaloneOptions,
    ) -> Result<Standalone, LuaError> {
        assert!(!argv.is_empty(), "argv holds at least the program's name");
        assert!(
            script.is_none_or(|i| i < argv.len()),
            "the script is one of the arguments"
        );
        let argv: Vec<Vec<u8>> = argv.iter().map(|a| a.as_ref().to_vec()).collect();
        // lua.c sets it before it opens the libraries
        self.set_ignore_env(opts.ignore_env);
        self.enter_pmain(argv.len());
        self.open_lua_c_libs()?;
        let mut s = Standalone {
            vm: self,
            argv,
            script,
            args: None,
        };
        if s.vm.version() >= LuaVersion::Lua53 {
            s.make_arg()?;
        }
        s.vm.gc_enter_generational();
        Ok(s)
    }

    /// The call `main` makes of `pmain`: protected, 5.1 with `lua_cpcall`
    /// (the function and its userdata on the stack), later with `lua_pcall`
    /// and `argc` and `argv` as arguments.
    fn enter_pmain(&mut self, argc: usize) {
        // a value standing for the program's own `argv`, or 5.1's userdata:
        // fixed, as the registry keys are (`light_key`)
        let argv = Value::LightUserdata(std::ptr::without_provenance(0x1030));
        if self.version() == LuaVersion::Lua51 {
            self.host_entry_layout(1, 2);
            self.host_entry_args(&[argv]);
        } else {
            self.host_entry_layout(1, 3);
            self.host_entry_args(&[Value::Int(argc as i64), argv]);
        }
    }
}

impl Standalone {
    /// 5.1 and 5.2: the `arg` table, made as `lua.c` makes it before it
    /// runs the script (`getargs`); the number of the script's arguments.
    /// Made once: a later call returns the same number and makes nothing.
    /// From 5.3 `arg` was made with the state, and this only returns the
    /// number. With no script, 5.1 and 5.2 make no `arg` (0).
    ///
    /// # Errors
    ///
    /// A memory error, or 5.1 and 5.2's "too many arguments to script"
    /// when the stack cannot hold them.
    pub fn script_args(&mut self) -> Result<usize, LuaError> {
        if let Some(n) = self.args {
            return Ok(n);
        }
        if self.script.is_none() {
            self.args = Some(0);
            return Ok(0);
        }
        self.make_arg()
    }

    fn make_arg(&mut self) -> Result<usize, LuaError> {
        let script = self.script.unwrap_or(0);
        self.vm.host_set_arg(&self.argv, script)?;
        let n = self.argv.len() - script - 1;
        self.args = Some(n);
        Ok(n)
    }

    /// The Vm.
    pub fn vm(&self) -> &Vm {
        &self.vm
    }

    /// The Vm, to run code in.
    pub fn vm_mut(&mut self) -> &mut Vm {
        &mut self.vm
    }

    /// The Vm, still inside `pmain`; 5.1 and 5.2 make no `arg` after this.
    pub fn into_vm(self) -> Vm {
        self.vm
    }
}
