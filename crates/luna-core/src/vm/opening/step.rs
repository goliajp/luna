//! The script's calls by their op byte (see `data.rs`), each read with
//! its operands and made.

use super::run::{Ix, Opener, Rd};
use crate::runtime::Value;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

impl Opener {
    /// One call, `op` with its operands in `r`.
    pub(super) fn step(&mut self, vm: &mut Vm, op: u8, r: &mut Rd<'_>) -> Result<(), LuaError> {
        let s = self.s;
        match op {
            1 => self.createtable(vm, r.z() as usize, r.z() as usize),
            2 => {
                let ix = self.ix(vm, r.z());
                self.setfield(vm, ix, self.str(r.u()).unwrap_or_default())
            }
            3 => {
                let ix = self.ix(vm, r.z());
                self.getfield(vm, ix, self.str(r.u()).unwrap_or_default())
            }
            4 => {
                let Ix::Slot(t) = self.ix(vm, r.z()) else {
                    unreachable!("a stack index")
                };
                self.set_top(vm, t + 1);
                Ok(())
            }
            5 => self.pushvalue(vm, r.z()),
            6 | 7 => self.pushstring(vm, self.str(r.u()).unwrap_or_default()),
            8 => self.push(vm, Value::Float(s.nums[r.u() as usize])),
            9 => self.push(vm, Value::Int(r.z())),
            10 => {
                let f = r.u() as u32;
                self.pushcclosure(vm, f, r.z() as u32)
            }
            11 => {
                let ix = self.ix(vm, r.z());
                self.rawseti(vm, ix, r.z())
            }
            12 => {
                let ix = self.ix(vm, r.z());
                self.rawgeti(vm, ix, r.z())
            }
            13 => {
                let ix = self.ix(vm, r.z());
                self.rawsetp(vm, ix)
            }
            14 => {
                let ix = self.ix(vm, r.z());
                self.setmetatable(vm, ix)
            }
            16 => self.setglobal(vm, self.str(r.u()).unwrap_or_default()),
            17 => {
                let nargs = r.z() as u32;
                self.call(vm, nargs, r.z() as i32)
            }
            18 => {
                let ix = self.ix(vm, r.z());
                self.replace(vm, ix)
            }
            19 => {
                let ix = self.ix(vm, r.z());
                self.setfenv(vm, ix)
            }
            20 => {
                let list = s.lists[r.u() as usize];
                self.setfuncs(vm, list, r.z() as u32)
            }
            21 => {
                let name = self.str(r.u());
                self.register(vm, name, s.lists[r.u() as usize])
            }
            22 => {
                let name = self.str(r.u()).unwrap_or_default();
                let f = r.u() as u32;
                self.requiref(vm, name, f, r.z() != 0)
            }
            23 => {
                let ix = self.ix(vm, r.z());
                self.getsubtable(vm, ix, self.str(r.u()).unwrap_or_default())
                    .map(drop)
            }
            24 => {
                let ix = self.ix(vm, r.z());
                let name = self.str(r.u()).unwrap_or_default();
                self.findtable(vm, ix, name, r.z() as usize).map(drop)
            }
            25 => self
                .newmetatable(vm, self.str(r.u()).unwrap_or_default())
                .map(drop),
            26 => {
                let [field, env, env2, def] = [r.u(), r.u(), r.u(), r.u()].map(|i| self.str(i));
                let field = field.unwrap_or_default();
                let def = def.unwrap_or_default();
                crate::vm::lib_package::puc_setpath(vm, self, field, env, env2, def)
            }
            27 => {
                let k = match r.u() {
                    1 => Err(r.z()),
                    _ => Ok(self.str(r.u())),
                };
                let fname = self.str(r.u()).unwrap_or_default();
                crate::vm::lib_io::puc_createstdfile(vm, self, k, fname)
            }
            28 => crate::vm::lib_math::puc_setrandfunc(vm, self),
            29 => self.newfenv(vm, r.u() as usize),
            _ => unreachable!("an op of the generator"),
        }
    }
}
