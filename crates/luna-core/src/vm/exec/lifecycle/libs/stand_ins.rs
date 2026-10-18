//! The objects of PUC's that luna's library values stand for, from 5.2 on.

use super::*;

impl Vm {
    /// The library objects of luna's, made without counting, that stand
    /// for PUC's C closures with an upvalue and userdata: the record
    /// counted PUC's; a marking counts them and a sweep frees them with
    /// luna's. (5.1's C functions are all closures, counted as luna's.)
    pub(super) fn lib_stand_ins(&mut self) {
        if self.version == LuaVersion::Lua51 {
            return;
        }
        let s = self.heap.sizes;
        let lib = |vm: &Vm, t: &[u8]| match vm.globals.get(Value::Str(vm.heap.find_interned(t)?)) {
            Value::Table(t) => Some(t),
            _ => None,
        };
        let (package, math, io) = (lib(self, b"package"), lib(self, b"math"), lib(self, b"io"));
        let field = |vm: &Vm, t: Option<Gc<Table>>, k: &[u8]| {
            Some(t?.get(Value::Str(vm.heap.find_interned(k)?)))
        };
        // the natives among them leave the fixed list together, in one walk,
        // and then stand for PUC's objects as the userdata do at once
        let mut natives = [(std::ptr::null_mut(), [0; 2], 0); 8];
        let mut n_natives = 0;
        let mut stand = |vm: &mut Vm, v: Option<Value>, sizes: &[usize]| {
            let h = match v {
                Some(Value::Native(n)) => {
                    let mut kept = [0; 2];
                    kept[..sizes.len()].copy_from_slice(sizes);
                    natives[n_natives] = (n.as_ptr() as *mut _, kept, sizes.len());
                    n_natives += 1;
                    return;
                }
                Some(Value::Userdata(u)) => u.as_ptr() as *mut _,
                _ => return,
            };
            if crate::runtime::heap::is_uncounted(h) {
                vm.heap.puc.stand_ins.extend(sizes.iter().map(|&s| (h, s)));
            }
        };
        // `require` and the searchers, with `package` as their upvalue
        let req = self
            .heap
            .find_interned(b"require")
            .map(|k| self.globals.get(Value::Str(k)));
        stand(self, req, &[s.c_closure(1)]);
        // 5.2's `module`, with `package` as its upvalue too
        if self.version == LuaVersion::Lua52 {
            let m = self
                .heap
                .find_interned(b"module")
                .map(|k| self.globals.get(Value::Str(k)));
            stand(self, m, &[s.c_closure(1)]);
        }
        if let Some(Value::Table(searchers)) = field(self, package, b"searchers") {
            for i in 1..=4 {
                stand(self, Some(searchers.get(Value::Int(i))), &[s.c_closure(1)]);
            }
        }
        // 5.4 on, `random` and `randomseed` share the generator's state
        if self.version >= LuaVersion::Lua54 {
            let random = field(self, math, b"random");
            stand(self, random, &[s.userdata(0, 32), s.c_closure(1)]);
            let seed = field(self, math, b"randomseed");
            stand(self, seed, &[s.c_closure(1)]);
        }
        // 5.5 keeps its default search paths as external strings over its
        // own bytes (`LSTRFIX`): a header each, as long as the package
        // table that holds them
        if self.version == LuaVersion::Lua55
            && let Some(pkg) = package
        {
            for name in [&b"path"[..], b"cpath"] {
                if let Some(Value::Str(p)) = field(self, package, name)
                    && crate::runtime::heap::is_uncounted(p.as_ptr() as *mut _)
                {
                    self.heap.acct_stand_on(pkg.header(), s.lngstr_hdr, false);
                }
            }
        }
        // the standard files, each a `LStream`
        for name in [&b"stdin"[..], b"stdout", b"stderr"] {
            let f = field(self, io, name);
            stand(self, f, &[s.userdata(0, 2 * s.ptr)]);
        }
        let natives = &natives[..n_natives];
        let mut hs = [std::ptr::null_mut(); 8];
        for (h, n) in hs.iter_mut().zip(natives) {
            *h = n.0;
        }
        self.heap.unfix_natives(&hs[..natives.len()]);
        for &(h, sizes, n) in natives {
            if crate::runtime::heap::is_uncounted(h) {
                self.heap
                    .puc
                    .stand_ins
                    .extend(sizes[..n].iter().map(|&s| (h, s)));
            }
        }
    }
}
