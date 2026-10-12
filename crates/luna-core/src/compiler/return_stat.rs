//! `return` statements, including tail calls.

use super::*;

impl<'a> Compiler<'a> {
    /// The register a `return` of no values names (PUC `luaK_ret`'s first
    /// register, which a 5.4+ return hook reports through `ftransfer`):
    /// the first one above the active locals from 5.4 on, 0 before.
    fn return0_base(&self) -> u32 {
        if self.version >= LuaVersion::Lua54 {
            self.lr().freereg
        } else {
            0
        }
    }

    /// The implicit `return` that ends a function, on line `line`, and the
    /// end of its outermost block. 5.2+ return while the function's locals
    /// are still active (PUC `close_func`: `luaK_ret`, then `leaveblock`),
    /// so a return hook sees them; 5.1 removes them first. The block's
    /// upvalues are closed by the return (see `mark_closing_returns`), not
    /// by a CLOSE of the block.
    pub(super) fn final_return(&mut self, line: u32) -> Result<(), SyntaxError> {
        if self.version <= LuaVersion::Lua51 {
            self.leave_block()?;
            self.last_line = line;
            self.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
        } else {
            self.last_line = line;
            let a = self.return0_base();
            self.emit(Inst::iabc(Op::Return0, a, 0, 0, false));
            self.leave_block()?;
        }
        self.finish_jumps()?;
        self.jump_error()
    }

    pub(super) fn return_stat(&mut self, exprs: &[ExprId]) -> Result<(), SyntaxError> {
        match exprs.len() {
            0 => {
                let a = self.return0_base();
                self.emit(Inst::iabc(Op::Return0, a, 0, 0, false));
            }
            1 => {
                // tail call: `return f(...)` (not parenthesized), but NOT in
                // the scope of a to-be-closed variable — the function must
                // return so its __close handlers run (PUC suppresses tail
                // calls inside tbc scope)
                let in_tbc = self.lr().blocks.iter().any(|b| b.tbc_scope);
                let is_call = matches!(
                    self.ast.expr(exprs[0]),
                    Expr::Call { .. } | Expr::MethodCall { .. }
                );
                if is_call {
                    let base = self.lr().freereg;
                    let e = self.expr(exprs[0])?;
                    let Exp::Open { pc, base: cb } = e else {
                        unreachable!()
                    };
                    debug_assert_eq!(cb, base);
                    if in_tbc {
                        // suppressed tail call: ordinary call returning all
                        // results, so __close handlers can run on RETURN
                        self.patch_wanted(pc, 0);
                        self.emit(Inst::iabc(Op::Return, base, 0, 0, false));
                    } else {
                        let call = self.l().code[pc];
                        self.l().code[pc] = Inst::iabc(Op::TailCall, call.a(), call.b(), 0, false);
                        // Fallback Return for the TailCall→native case: PUC
                        // never actually tail-calls a C function (the C frame
                        // has no Lua activation to fold into), so `OP_TAILCALL`
                        // there runs the native under the current Lua frame and
                        // returns the native's results to the caller. luna's
                        // `Op::TailCall` keeps the frame for native targets, so
                        // after the native completes (or yields-then-resumes)
                        // the run loop needs an explicit Return to forward the
                        // results — the Lua-target path pops the frame and so
                        // never reaches this op.
                        self.emit(Inst::iabc(Op::Return, base, 0, 0, false));
                    }
                    self.set_freereg(base);
                    return Ok(());
                }
                if matches!(self.ast.expr(exprs[0]), Expr::Vararg) {
                    let base = self.lr().freereg;
                    let e = self.expr(exprs[0])?;
                    let Exp::Open { pc, .. } = e else {
                        unreachable!()
                    };
                    self.patch_wanted(pc, 0);
                    self.emit(Inst::iabc(Op::Return, base, 0, 0, false));
                    self.set_freereg(base);
                    return Ok(());
                }
                let saved = self.lr().freereg;
                let e = self.expr(exprs[0])?;
                let r = self.exp_to_anyreg(e)?;
                self.set_freereg(saved);
                self.emit(Inst::iabc(Op::Return1, r, 0, 0, false));
            }
            n if n > 254 => {
                // `OP_RETURN`'s B field is a byte (`b = nret + 1`): at most
                // 254 fixed values. PUC places every value in a register
                // first, so the register limit speaks first unless the
                // values fit: only 5.5 allows 255 registers and then checks
                // the count, with `errorlimit` (5.5 calls.lua :591).
                let base = self.lr().freereg as usize;
                if self.version >= LuaVersion::Lua55 && base + n <= 255 {
                    return Err(self.limit_err("returns", 255));
                }
                return Err(self.regs_error(self.last_line));
            }
            n => {
                let base = self.lr().freereg;
                let mut open = false;
                for (i, &eid) in exprs.iter().enumerate() {
                    let dst = base + i as u32;
                    self.set_freereg(dst);
                    let e = self.expr(eid)?;
                    if i == n - 1
                        && let Exp::Open { pc, base: ob } = e
                    {
                        debug_assert_eq!(ob, dst);
                        self.patch_wanted(pc, 0);
                        open = true;
                        break;
                    }
                    self.set_freereg(dst);
                    let got = self.exp_to_nextreg(e)?;
                    debug_assert_eq!(got, dst);
                }
                let b = if open { 0 } else { n as u32 + 1 };
                self.emit(Inst::iabc(Op::Return, base, b, 0, false));
                self.set_freereg(base);
            }
        }
        Ok(())
    }
}
