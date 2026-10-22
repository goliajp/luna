//! Building the state the interpreter runs in, as `lua.c`'s `main` and
//! `pmain` do, with luna's options and test knobs.

use luna_jit::VmExt;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::{Standalone, StandaloneOptions, Vm};

use crate::LunaOpts;

/// The state the interpreter runs in: lua.c's, or the sandbox's (no
/// lua.c around it).
pub(crate) enum Host {
    LuaC(Standalone),
    Sandbox(Vm),
}

impl Host {
    pub(crate) fn vm(&mut self) -> &mut Vm {
        match self {
            Host::LuaC(s) => s.vm_mut(),
            Host::Sandbox(vm) => vm,
        }
    }

    pub(crate) fn vm_ref(&self) -> &Vm {
        match self {
            Host::LuaC(s) => s.vm(),
            Host::Sandbox(vm) => vm,
        }
    }

    /// What lua.c does in `pmain` after it opened the libraries, before
    /// it runs anything: from 5.3 `arg`, from 5.4 the collector restarted
    /// in generational mode (lua.c's state did that as it was made).
    pub(crate) fn pmain_start(&mut self, argv: &[Vec<u8>], script: usize) {
        if let Host::Sandbox(vm) = self {
            if vm.version() >= LuaVersion::Lua53 {
                sandbox_arg(vm, argv, script);
            }
            let op = match vm.version() {
                LuaVersion::Lua55 => 7,
                v if v >= LuaVersion::Lua54 => 10,
                _ => return,
            };
            let _ = vm.host_gc(1, [0; 3]);
            let _ = vm.host_gc(op, [0; 3]);
        }
    }

    /// 5.1 and 5.2's `getargs` before the script runs.
    pub(crate) fn script_args(&mut self, argv: &[Vec<u8>], script: usize) {
        match self {
            Host::LuaC(s) => {
                s.script_args().expect("set the arg global");
            }
            Host::Sandbox(vm) if vm.version() <= LuaVersion::Lua52 => sandbox_arg(vm, argv, script),
            Host::Sandbox(_) => {}
        }
    }
}

/// `arg` in the sandbox: `arg[0]` the script, its arguments from 1, what
/// comes before it down from -1.
fn sandbox_arg(vm: &mut Vm, argv: &[Vec<u8>], script: usize) {
    let mut t = vm.new_table();
    for (i, a) in argv.iter().enumerate() {
        t = t.with(i as i64 - script as i64, a.as_slice());
    }
    let t = t.build();
    vm.set_global("arg", Value::Table(t))
        .expect("set the arg global");
}

pub(crate) fn new_vm(
    opts: &LunaOpts,
    ignore_env: bool,
    argv: &[Vec<u8>],
    script: Option<usize>,
) -> Host {
    // Cranelift (`Vm::new` has the no-op backend), unless --no-jit
    let mut host = if opts.sandbox {
        // SandboxBuilder has no JIT backend but its trace flags start on:
        // --no-jit switches them off, otherwise Cranelift goes in after it
        let mut vm = luna_jit::vm::Vm::sandbox(opts.version)
            .open_base()
            .open_math()
            .open_string()
            .open_table()
            .open_coroutine()
            .build();
        if opts.no_jit {
            vm.install_null_jit();
        } else {
            vm.install_default_jit();
        }
        Host::Sandbox(vm)
    } else {
        // Test knob, deliberately left out of --help: hash strings with a
        // fixed seed, so that two runs, and two builds, do the same work
        // (instruction counts otherwise vary with the random seed).
        // Another: take 5.5's `#t` random steps from a fixed seed, to
        // compare with a PUC 5.5 built with that seed.
        if let Some(s) = std::env::var("LUNA_LEN_SEED")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
        {
            luna_jit::runtime::table::set_len_search_seed(Some(s));
        }
        let seed = std::env::var("LUNA_HASH_SEED")
            .ok()
            .and_then(|s| s.parse::<u32>().ok());
        let mut vm = match seed {
            Some(s) => luna_jit::vm::Vm::new_minimal_with_hash_seed(opts.version, s),
            None => luna_jit::vm::Vm::new_minimal(opts.version),
        };
        if opts.no_jit {
            vm.install_null_jit();
        } else {
            luna_jit::install_default_jit(&mut vm);
        }
        let lua_c = StandaloneOptions::default().ignore_env(ignore_env);
        Host::LuaC(
            vm.into_standalone(argv, script, lua_c)
                .expect("lua.c's state"),
        )
    };
    let vm = host.vm();
    if let Some(n) = opts.budget {
        vm.set_instr_budget(Some(n));
    }
    // lua.exe opens files through the MSVC C library, in text mode unless
    // the mode has a `b`
    vm.set_crt_text_mode(cfg!(windows));
    // Test knob, deliberately left out of --help: record traces after N
    // back-edges / calls instead of 64, so that short programs (the
    // differential corpora) run through the trace JIT.
    if let Some(n) = std::env::var("LUNA_JIT_HOT")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        vm.jit.trace_hot_threshold = n;
        vm.jit.call_hot_threshold = n;
    }
    // Test knob, also left out of --help: with the LLVM backend, how long a
    // hot trace runs Cranelift's code before LLVM compiles it (0: at once)
    #[cfg(feature = "llvm-jit")]
    if !opts.no_jit
        && std::env::var("LUNA_JIT_BACKEND").as_deref() == Ok("llvm")
        && let Some(ms) = std::env::var("LUNA_LLVM_AFTER_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
    {
        let llvm_after = (ms > 0).then(|| std::time::Duration::from_millis(ms));
        luna_jit::install_llvm_backend_with(vm, luna_jit::jit_backend::LlvmBackend { llvm_after });
    }
    // Test knob, also left out of --help: move a trace to the optimizing
    // tier after N loop iterations and entries (0: never)
    if let Some(n) = std::env::var("LUNA_TIER_UP_AT")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        vm.set_trace_tier_up_at(n);
    }
    host
}
