//! 5.1's buffer: the C buffer's contents become strings on the stack,
//! joined as `adjuststack` joins them.

use super::*;

/// 5.1 `adjuststack`'s `LIMIT` (`LUA_MINSTACK / 2`).
const LIMIT_51: usize = 10;

impl PucBuffer {
    /// 5.1 `luaL_prepbuffer`
    pub(super) fn prepbuffer_51(&mut self, vm: &mut Vm, out: &[u8]) -> Result<(), LuaError> {
        let end = self.start + self.n;
        if self.empty_51(vm, &out[..end])? {
            self.adjust_51(vm)?;
        }
        Ok(())
    }

    /// 5.1 `emptybuffer`: the C buffer's bytes, the end of `out`, become a
    /// string on the stack.
    pub(super) fn empty_51(&mut self, vm: &mut Vm, out: &[u8]) -> Result<bool, LuaError> {
        if self.n == 0 {
            return Ok(false);
        }
        vm.api_check_before()?;
        let s = vm.alloc_str(&out[self.start..self.start + self.n])?;
        vm.lib_roots.push(Value::Str(s));
        self.start += self.n;
        self.n = 0;
        Ok(true)
    }

    /// 5.1 `adjuststack`
    pub(super) fn adjust_51(&mut self, vm: &mut Vm) -> Result<(), LuaError> {
        let lvl = vm.lib_roots.len() - self.base.at();
        if lvl <= 1 {
            return Ok(());
        }
        let len = |vm: &Vm, i: usize| match vm.lib_roots[vm.lib_roots.len() - i] {
            Value::Str(s) => s.len(),
            _ => unreachable!("the buffer's strings"),
        };
        let mut toget = 1;
        let mut toplen = len(vm, 1);
        loop {
            let l = len(vm, toget + 1);
            if lvl - toget + 1 >= LIMIT_51 || toplen > l {
                toplen += l;
                toget += 1;
            } else {
                break;
            }
            if toget >= lvl {
                break;
            }
        }
        self.concat_51(vm, toget)
    }

    /// `lua_concat(L, n)` of the top `n` strings of the buffer
    pub(super) fn concat_51(&mut self, vm: &mut Vm, n: usize) -> Result<(), LuaError> {
        let from = vm.lib_roots.len() - n;
        let mut st: Vec<Value> = vm.lib_roots[from..].to_vec();
        vm.api_check_before()?;
        vm.concat_pieces(&mut st, n)?;
        vm.lib_roots.pop_n(n);
        vm.lib_roots.push(st[0]);
        Ok(())
    }
}
