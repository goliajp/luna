//! Fast-loop arms for varargs, the nil-check error, calls and returns.
//! The generator defines one macro per opcode inside `run_fast`; it takes
//! the loop's locals and label as arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_call_arms {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $pc:ident,
        $code:ident, $kptr:ident, $trace_on:ident, $pre53:ident, $entry_depth:ident,
        $frames:lifetime
    ) => {
        macro_rules! op_varg_idx {
            () => {{
                // R[A] := vararg[R[C]] without allocating: integer key in
                // [1,n] → that vararg, "n" → the count, else nil.
                let key = reg!($inst.c());
                // the extras sit just below the base
                // SAFETY: `fr` is the running frame
                let (first, n) = unsafe { ((*$fr).base - (*$fr).n_varargs, (*$fr).n_varargs) };
                let v = match key {
                    Value::Int(k) if k >= 1 && (k as u64) <= n as u64 => {
                        $vm.stack[(first + k as u32 - 1) as usize]
                    }
                    Value::Float(f) if f.fract() == 0.0 && f >= 1.0 && f <= n as f64 => {
                        $vm.stack[(first + f as u32 - 1) as usize]
                    }
                    Value::Str(s) if s.as_bytes() == b"n" => Value::Int(n as i64),
                    _ => Value::Nil,
                };
                set_reg!($inst.a(), v);
                next!()
            }};
        }
        macro_rules! op_err_n_nil {
            () => {{
                let v = $vm.r(base!(), $inst.a());
                if !matches!(v, Value::Nil) {
                    let bx = $inst.bx();
                    let name = if bx == 0 {
                        "?".to_string()
                    } else {
                        match cl!().proto.consts[(bx - 1) as usize] {
                            Value::Str(s) => {
                                String::from_utf8_lossy(s.as_bytes()).into_owned()
                            }
                            _ => "?".to_string(),
                        }
                    };
                    save!();
                    return Err($vm.rt_err(&format!("global '{name}' already defined")));
                }
                next!()
            }};
        }
        macro_rules! op_call {
            () => {{
                save!();
                let abs = base!() + $inst.a();
                let nargs = if $inst.b() == 0 {
                    None
                } else {
                    Some($inst.b() - 1)
                };
                let wanted = $inst.c() as i32 - 1;
                let pf = $regs.wrapping_add($inst.a() as usize);
                // a plain native returns to this frame, so a loop watching
                // for trace heads runs it here too, unless a recording or a
                // trap wants every instruction at the loop head
                if !WATCH || $vm.jit.active_trace.is_none() && !$vm.trap {
                    // SAFETY: the called register is in the frame, and a
                    // closure or native tag means its payload points at a
                    // live object of that type
                    let (lua, native) = unsafe {
                        match raw_tag(pf) {
                            tag::CLOSURE if !$trace_on => (
                                Some(Gc::from_ptr_unchecked(raw_gc(pf) as *mut LuaClosure)),
                                None,
                            ),
                            tag::NATIVE => (
                                None,
                                Some(Gc::from_ptr_unchecked(
                                    raw_gc(pf) as *mut crate::runtime::NativeClosure,
                                )),
                            ),
                            _ => (None, None),
                        }
                    };
                    if let Some(callee) = lua {
                        let n = nargs.unwrap_or_else(|| $vm.top - (abs + 1));
                        if let Some(nf) = $vm.push_lua_frame_fast(callee, abs, n, wanted) {
                            $fr = nf;
                            continue $frames;
                        }
                    } else if let Some(nc) = native
                        && nc.kind == NativeKind::Plain
                    {
                        let n = nargs.unwrap_or_else(|| $vm.top - (abs + 1));
                        $vm.call_native_plain(nc, abs, n, wanted)?;
                        resume!()
                    }
                }
                $vm.begin_call(abs, nargs, wanted, false)?;
                reenter!()
            }};
        }
        // the common returns: to a Lua caller or a metamethod's
        // continuation in this activation, with nothing to close and
        // no hook (see `return_fast`); the loop head's `Return` arm
        // does the rest
        macro_rules! op_return0 {
            () => {{
                let base = base!();
                let done = $vm.return_fast::<WATCH>(base, base, 0, $entry_depth, $inst.k());
                returned!(done)
            }};
        }
        macro_rules! op_return1 {
            () => {{
                let base = base!();
                let done = $vm.return_fast::<WATCH>(
                    base,
                    base + $inst.a(),
                    1,
                    $entry_depth,
                    $inst.k(),
                );
                returned!(done)
            }};
        }
    };
}
pub(super) use fast_call_arms;
