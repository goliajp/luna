//! Library functions under an allocation context that refuses memory: every
//! failure is the memory error, raised to `pcall` and to the host, and never
//! a panic; a single refusal follows each dialect's PUC (5.1 fails, later
//! dialects retry allocations made through the collector after an emergency
//! collection, while 5.3–5.5 string buffers fail at once).

use std::cell::Cell;
use std::rc::Rc;

use luna_core::__private::VmHostInternals;
use luna_core::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use luna_core::runtime::{ErrorStatus, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// Counts growths once armed; refuses those numbered `from` on (when
/// `once` is false) or only the one numbered `from` (when it is true).
#[derive(Clone, Default)]
struct Plan {
    armed: Rc<Cell<bool>>,
    seen: Rc<Cell<u64>>,
    from: Rc<Cell<u64>>,
    once: bool,
}

impl MemoryPolicy for Plan {
    fn allow(&mut self, _old: usize, _new: usize, _kind: BlockKind, _in_use: usize) -> bool {
        if !self.armed.get() {
            return true;
        }
        let n = self.seen.get();
        self.seen.set(n + 1);
        if self.once {
            n != self.from.get()
        } else {
            n < self.from.get()
        }
    }
}

const SCRIPTS: &[&str] = &[
    r##"local s = string.rep("ab", 300, ",") .. string.format("%5d|%-8s|%q", 42, "x", "a\0b\n")
       local g = string.gsub(s, "a(b)", "<%1>")
       local up, low = string.upper(g), string.lower(g)
       return #s .. ":" .. #g .. ":" .. up:sub(1, 12) .. low:sub(-6)"##,
    r##"local t = {}
       for i = 1, 200 do t[#t + 1] = tostring(i) end
       table.insert(t, 1, "first")
       table.sort(t)
       local c = table.concat(t, ",")
       return #c .. c:sub(1, 20) .. select("#", (table.unpack or unpack)(t, 1, 50))"##,
    r##"local co = coroutine.wrap(function(a)
         local acc = {}
         for i = 1, 30 do acc[i] = coroutine.yield(a .. i) end
         return table.concat(acc)
       end)
       local out = {co("x")}
       for i = 1, 30 do out[#out + 1] = co(tostring(i)) end
       local c = coroutine.create(function() return "done" end)
       return table.concat(out, ";") .. select(2, coroutine.resume(c))"##,
    r##"if not string.pack then return "no pack" end
       local p = string.pack("i4zs1", 7, "zero", "len")
       local a, b, c = string.unpack("i4zs1", p)
       local u = utf8 and utf8.char(72, 228, 8364, 0x10FFFF) or ""
       return #p .. a .. b .. c .. #u"##,
    r##"local d = os.date("*t", 0)
       local s = os.date("!%Y-%m-%d %H:%M:%S", 86400 * 365)
       local f, msg = (loadstring or load)("local x = ... return x * 2, '" .. string.rep("y", 100) .. "'")
       if not f then return "load failed: " .. msg end
       local a, b = f(21)
       return d.year .. s .. a .. #b"##,
];

fn dialects() -> [LuaVersion; 5] {
    [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ]
}

fn text(v: &Value) -> String {
    match v {
        Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        Value::Int(i) => i.to_string(),
        Value::Bool(b) => b.to_string(),
        other => format!("<{}>", other.type_name()),
    }
}

/// What a run produced: its result as text, or its error and whether the
/// host saw the memory status.
enum Outcome {
    Ok(String),
    Err(String, bool),
}

fn run(v: LuaVersion, plan: &Plan, src: &str, protected: bool) -> Outcome {
    let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(plan.clone())));
    let body = if protected {
        format!(
            "local ok, r = pcall(function() {src} end) return tostring(ok) .. '|' .. tostring(r)"
        )
    } else {
        src.to_string()
    };
    plan.seen.set(0);
    plan.armed.set(true);
    let before = vm.special_errors();
    let r = vm.eval(&body);
    plan.armed.set(false);
    match r {
        Ok(vals) => Outcome::Ok(vals.first().map(text).unwrap_or_default()),
        Err(e) => {
            let mem = vm.error_status(e.0, before) == ErrorStatus::Mem;
            Outcome::Err(vm.error_text(&e), mem)
        }
    }
}

/// Whether `msg` is a memory failure as one of the dialects raises it.
fn memory_message(v: LuaVersion, msg: &str) -> bool {
    msg == "not enough memory"
        || (v == LuaVersion::Lua53 && msg.ends_with("not enough memory for buffer allocation"))
        // up to 5.3 `coroutine.wrap` puts its position in front of any
        // string error, the memory one included
        || (v <= LuaVersion::Lua53 && msg.ends_with(": not enough memory"))
}

/// The growths a successful run makes, and its result.
fn baseline(v: LuaVersion, src: &str, protected: bool) -> (u64, String) {
    let plan = Plan {
        from: Rc::new(Cell::new(u64::MAX)),
        ..Plan::default()
    };
    match run(v, &plan, src, protected) {
        Outcome::Ok(out) => (plan.seen.get(), out),
        Outcome::Err(e, _) => panic!("{v:?}: unrestricted run failed: {e}"),
    }
}

/// The refusal points to try: all of them for a short run, an even spread
/// plus the first and last hundred for a long one.
fn points(total: u64) -> Vec<u64> {
    if total <= 400 {
        return (0..=total).collect();
    }
    let mut p: Vec<u64> = (0..100).chain(total - 100..=total).collect();
    let step = (total - 200) / 200 + 1;
    p.extend((100..total - 100).step_by(step as usize));
    p
}

#[test]
fn refusing_from_n_on_is_always_the_memory_error() {
    for v in dialects() {
        for src in SCRIPTS {
            for protected in [false, true] {
                let (total, want) = baseline(v, src, protected);
                for n in points(total) {
                    let plan = Plan {
                        from: Rc::new(Cell::new(n)),
                        ..Plan::default()
                    };
                    match run(v, &plan, src, protected) {
                        Outcome::Ok(out) if out == want => {}
                        Outcome::Ok(out) if protected => {
                            let msg = out.strip_prefix("false|").unwrap_or_else(|| {
                                panic!("{v:?} n={n}: {out:?} is neither {want:?} nor a failure")
                            });
                            assert!(memory_message(v, msg), "{v:?} n={n}: {msg}");
                        }
                        Outcome::Ok(out) => assert!(
                            out == "load failed: not enough memory",
                            "{v:?} n={n}: {out:?} instead of {want:?}"
                        ),
                        Outcome::Err(msg, mem) => {
                            assert!(memory_message(v, &msg), "{v:?} n={n}: {msg}");
                            if msg == "not enough memory" {
                                assert!(mem, "{v:?} n={n}: status is not the memory one");
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_single_refusal_retries_where_puc_does() {
    for v in dialects() {
        for src in SCRIPTS {
            let (total, want) = baseline(v, src, false);
            let mut retried = 0;
            for n in points(total) {
                let plan = Plan {
                    from: Rc::new(Cell::new(n)),
                    once: true,
                    ..Plan::default()
                };
                match run(v, &plan, src, false) {
                    // the script caught the memory error itself
                    // (`coroutine.resume` reports it)
                    Outcome::Ok(out) if out.ends_with("not enough memory") => {}
                    Outcome::Ok(out) => {
                        if n < total {
                            assert_ne!(
                                v,
                                LuaVersion::Lua51,
                                "5.1 has no emergency collection: n={n} {src}"
                            );
                        }
                        assert!(
                            out == want || out == "load failed: not enough memory",
                            "{v:?} n={n}: {out:?} instead of {want:?}"
                        );
                        retried += 1;
                    }
                    Outcome::Err(msg, _) => {
                        assert!(memory_message(v, &msg), "{v:?} n={n}: {msg}");
                    }
                }
            }
            if v >= LuaVersion::Lua52 {
                // most growths go through the collector's allocation, which
                // retries after an emergency collection
                assert!(retried > 0, "{v:?}: no single refusal was retried");
            }
        }
    }
}
