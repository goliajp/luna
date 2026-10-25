//! `collectgarbage` — the option set, argument handling, return shapes and
//! pacing-parameter storage of each PUC version.
//!
//! luna's collector is its own, so what `collectgarbage` must reproduce is the
//! *interface*: which options a dialect accepts, what each returns, and how a
//! parameter written reads back. PUC stores the parameters in version-specific
//! encodings that lose precision, and a program can observe that
//! (`setpause(150)` reads back 148 on 5.4, `param("pause", 123)` 118 on 5.5),
//! so the values are kept in the dialect's encoding.
//!
//! The parameters still drive luna's own pacing. A value is mapped onto luna's
//! knob in proportion to the dialect's default, so the default leaves luna's
//! pacing unchanged and doubling a parameter doubles its effect.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// luna's own pacing defaults: heap growth before a new cycle (%), sweep work
/// per safe point (see `Vm::checkpoint`), and objects per step.
pub(crate) const LUNA_PAUSE: i64 = 200;
pub(crate) const LUNA_STEPMUL: i64 = 100;
const LUNA_STEPSIZE: i64 = 13;

pub(crate) use crate::runtime::puc::Param;

/// `collectgarbage([opt [, ...]])`.
pub(crate) fn nat_collectgarbage(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let opt = argcheck::check_option(vm, a, 0, Some("collect"), options(v))?;
    let opt = options(v)[opt];
    // ≤5.3 read the second argument as an int for every option.
    let ex = if v <= LuaVersion::Lua53 {
        argcheck::opt_integer(vm, a, 1, 0)? as i32
    } else {
        0
    };
    // 5.4+ `lua_gc` refuses to run inside a finalizer and the library reports
    // fail. ≤5.3 answer normally; luna's collector is not reentrant, so a
    // collection requested there does not run.
    if vm.gc_is_finalizing() && v >= LuaVersion::Lua54 {
        // 5.5 pushes `lua_gc`'s -1 for `param` as it is
        let r = if opt == "param" {
            param_args(vm, a)?;
            Value::Int(-1)
        } else {
            Value::Nil
        };
        return vm.nat_ret(fs, &[r]);
    }
    let out: Vec<Value> = match opt {
        "collect" => {
            // 5.1–5.3 propagate the first `__gc` error to the caller;
            // 5.4 warns and continues (gc.lua 5.1 :255, 5.2 :346, 5.3 :360).
            if !vm.gc_is_finalizing() && v <= LuaVersion::Lua53 {
                vm.collect_garbage_propagating()?;
            } else if !vm.gc_is_finalizing() {
                vm.collect_garbage();
            }
            vec![Value::Int(0)]
        }
        "count" => {
            let bytes = vm.gc_count_bytes();
            let kb = Value::Float(bytes as f64 / 1024.0);
            if v == LuaVersion::Lua52 {
                // 5.2 alone also returns LUA_GCCOUNTB (gc.lua 5.2 :139 asserts
                // `k*1024 == floor(k)*1024 + b`).
                vec![kb, Value::Int((bytes % 1024) as i64)]
            } else {
                vec![kb]
            }
        }
        "step" => {
            let n = if v <= LuaVersion::Lua53 {
                i64::from(ex)
            } else {
                argcheck::opt_integer(vm, a, 1, 0)?
            };
            vec![Value::Bool(step(vm, n)?)]
        }
        "stop" | "restart" => {
            vm.heap.gc_set_stopped(opt == "stop");
            vec![Value::Int(0)]
        }
        // 5.2/5.3 stop the collector while a finalizer runs (GCTM clears
        // gcrunning), so a finalizer sees it as not running.
        "isrunning" => vec![Value::Bool(
            !vm.heap.gc_is_stopped() && !vm.gc_is_finalizing(),
        )],
        "setpause" => vec![Value::Int(set_param(vm, a, Param::Pause, ex)?)],
        "setstepmul" => vec![Value::Int(set_param(vm, a, Param::StepMul, ex)?)],
        "setmajorinc" => vec![Value::Int(set_param(vm, a, Param::MajorInc, ex)?)],
        "incremental" | "generational" => switch_mode(vm, a, opt)?,
        "param" => vec![param(vm, a)?],
        _ => unreachable!("check_option returned an index into options()"),
    };
    vm.nat_ret(fs, &out)
}

fn options(v: LuaVersion) -> &'static [&'static str] {
    match v {
        LuaVersion::Lua51 => &[
            "stop",
            "restart",
            "collect",
            "count",
            "step",
            "setpause",
            "setstepmul",
        ],
        LuaVersion::Lua52 => &[
            "stop",
            "restart",
            "collect",
            "count",
            "step",
            "setpause",
            "setstepmul",
            "setmajorinc",
            "isrunning",
            "generational",
            "incremental",
        ],
        LuaVersion::Lua53 => &[
            "stop",
            "restart",
            "collect",
            "count",
            "step",
            "setpause",
            "setstepmul",
            "isrunning",
        ],
        LuaVersion::Lua54 | LuaVersion::MacroLua => &[
            "stop",
            "restart",
            "collect",
            "count",
            "step",
            "setpause",
            "setstepmul",
            "isrunning",
            "generational",
            "incremental",
        ],
        LuaVersion::Lua55 => &[
            "stop",
            "restart",
            "collect",
            "count",
            "step",
            "isrunning",
            "generational",
            "incremental",
            "param",
        ],
    }
}

/// Advance the collector. A step of `n` sweeps a budgeted chunk and returns
/// true once a full cycle finishes; in generational mode a step is a full
/// minor collection.
///
/// `n` = 0 (also what an absent argument means) is PUC's basic step: a fixed
/// small one on ≤5.3 (`GCSTEPSIZE`; gc.lua's `dosteps(0) > 10` pins that it
/// takes many of them to finish a cycle), and one of the `stepsize`
/// parameter's size from 5.4 on, where stepsize 0 completes the cycle.
pub(crate) fn step(vm: &mut Vm, n: i64) -> Result<bool, LuaError> {
    if crate::runtime::puc::step::lua_step_skipped(&vm.heap) {
        return Ok(false);
    }
    if vm.gc_mode_is_generational() {
        vm.collect_garbage();
        return Ok(false);
    }
    crate::runtime::puc::step::gc_lua_step(vm, n)
}

/// `setpause` / `setstepmul` / `setmajorinc`: store the new value (≤5.3 take
/// it from the already-read second argument, 5.4 reads it here; an absent
/// value is 0 and is stored too) and return the previous one.
fn set_param(vm: &mut Vm, a: Args, param: Param, ex: i32) -> Result<i64, LuaError> {
    let new = if vm.version() <= LuaVersion::Lua53 {
        ex
    } else {
        argcheck::opt_integer(vm, a, 1, 0)? as i32
    };
    let prev = vm.heap.gc_params().get(param);
    vm.heap.gc_params_mut().set(param, new);
    sync_pacing(vm);
    Ok(prev)
}

/// `incremental` / `generational`. 5.2 switches and returns 0; 5.4 also sets
/// any nonzero parameters given; 5.4/5.5 return the previous mode's name.
fn switch_mode(vm: &mut Vm, a: Args, opt: &'static str) -> Result<Vec<Value>, LuaError> {
    let v = vm.version();
    if v == LuaVersion::Lua54 || v == LuaVersion::MacroLua {
        let n = if opt == "incremental" { 3 } else { 2 };
        let mut values = Vec::with_capacity(n);
        for i in 0..n as u32 {
            values.push(argcheck::opt_integer(vm, a, 1 + i, 0)? as i32);
        }
        set_mode_params(vm, opt, &values);
    }
    let prev = vm.gc_switch_mode(opt);
    Ok(vec![if v == LuaVersion::Lua52 {
        Value::Int(0)
    } else {
        // `pushmode`'s `lua_pushstring`
        vm.api_push_string(prev.as_bytes())?
    }])
}

/// 5.4 `lua_gc(LUA_GCINC, pause, stepmul, stepsize)` and `(LUA_GCGEN,
/// minormul, majormul)`: set each nonzero parameter given.
pub(crate) fn set_mode_params(vm: &mut Vm, mode: &str, values: &[i32]) {
    let params: &[Param] = if mode == "incremental" {
        &[Param::Pause, Param::StepMul, Param::StepSize]
    } else {
        &[Param::MinorMul, Param::MajorMul]
    };
    for (&param, &value) in params.iter().zip(values) {
        if value != 0 {
            vm.heap.gc_params_mut().set(param, value);
        }
    }
    sync_pacing(vm);
}

/// 5.5's parameters in `LUA_GCP*` order.
pub(crate) const PARAM_ORDER: [Param; 6] = [
    Param::MinorMul,
    Param::MajorMinor,
    Param::MinorMajor,
    Param::Pause,
    Param::StepMul,
    Param::StepSize,
];

/// 5.5 `param`'s parameter names, in `LUA_GCP*` order.
const NAMES: [&str; 6] = [
    "minormul",
    "majorminor",
    "minormajor",
    "pause",
    "stepmul",
    "stepsize",
];

/// 5.5 `param`'s arguments: the parameter and the new value (-1: none).
fn param_args(vm: &mut Vm, a: Args) -> Result<(Param, i32), LuaError> {
    let which = PARAM_ORDER[argcheck::check_option(vm, a, 1, None, &NAMES)?];
    Ok((which, argcheck::opt_integer(vm, a, 2, -1)? as i32))
}

/// 5.5 `param`: read a parameter, or set it (a negative value leaves it
/// unchanged) and return the previous value.
fn param(vm: &mut Vm, a: Args) -> Result<Value, LuaError> {
    let (which, value) = param_args(vm, a)?;
    let prev = vm.heap.gc_params().get(which);
    if value >= 0 {
        vm.heap.gc_params_mut().set(which, value);
        sync_pacing(vm);
    }
    Ok(Value::Int(prev))
}

/// Carry the Lua-visible parameters over to luna's collector knobs.
pub(crate) fn sync_pacing(vm: &mut Vm) {
    let p = vm.heap.gc_params();
    let pause = p.luna_scaled(Param::Pause, LUNA_PAUSE);
    let stepmul = p.luna_scaled(Param::StepMul, LUNA_STEPMUL);
    let stepsize = match p.default(Param::StepSize) {
        // stepsize 0 means "a step completes the cycle" and must stay 0
        Some(_) if p.get(Param::StepSize) == 0 => 0,
        Some(_) => p.luna_scaled(Param::StepSize, LUNA_STEPSIZE).max(1),
        None => LUNA_STEPSIZE,
    };
    vm.set_gc_pacing(pause, stepmul, stepsize);
}
