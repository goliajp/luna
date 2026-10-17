//! Building the state the interpreter runs in, as `lua.c`'s `main` and
//! `pmain` do, with luna's options and test knobs.

use luna_jit::VmExt;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

use crate::LunaOpts;

pub(crate) fn new_vm(opts: &LunaOpts, ignore_env: bool, argc: usize) -> Vm {
    // Cranelift (`Vm::new` has the no-op backend), unless --no-jit
    let mut vm = if opts.sandbox {
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
        vm
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
        // lua.c sets it before it opens the libraries
        vm.set_ignore_env(ignore_env);
        enter_pmain(&mut vm, opts.version, argc);
        vm.trace_mark("openlibs");
        vm.open_lua_c_libs().expect("the libraries open");
        vm.trace_mark("/openlibs");
        vm
    };
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
        luna_jit::install_llvm_backend_with(
            &mut vm,
            luna_jit::jit_backend::LlvmBackend { llvm_after },
        );
    }
    // Test knob, also left out of --help: move a trace to the optimizing
    // tier after N loop iterations and entries (0: never)
    if let Some(n) = std::env::var("LUNA_TIER_UP_AT")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        vm.set_trace_tier_up_at(n);
    }
    vm
}

/// lua.c runs everything inside `pmain`, a C function `main` calls
/// protected with `lua_cpcall` (5.1: the function and its userdata on the
/// stack) or `lua_pcall` with `argc` and `argv` as arguments, before it
/// opens the libraries.
fn enter_pmain(vm: &mut Vm, version: LuaVersion, argc: usize) {
    // an address standing for the program's own `argv`, or 5.1's userdata
    static ARGV: u8 = 0;
    let argv = Value::LightUserdata(&ARGV as *const u8 as *const ());
    if version == LuaVersion::Lua51 {
        vm.host_entry_layout(1, 2);
        vm.host_entry_args(&[argv]);
    } else {
        vm.host_entry_layout(1, 3);
        vm.host_entry_args(&[Value::Int(argc as i64), argv]);
    }
}
