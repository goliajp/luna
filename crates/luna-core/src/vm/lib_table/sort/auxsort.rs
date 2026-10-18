//! 5.3+'s sort on `unsigned int` indices.

use super::*;

impl Sorter {
    /// 5.3+ `partition`: pivot P on top of the stack, a[lo] <= P == a[up-1]
    /// <= a[up].
    fn partition(&self, vm: &mut Vm, lo: u32, up: u32) -> Result<u32, LuaError> {
        let mut i = lo;
        let mut j = up - 1;
        loop {
            i += 1;
            self.geti(vm, i.into())?;
            while self.lt(vm, 1, 2)? {
                if i == up - 1 {
                    return Err(self.invalid_order(vm));
                }
                Self::pop(vm, 1);
                i += 1;
                self.geti(vm, i.into())?;
            }
            j -= 1;
            self.geti(vm, j.into())?;
            while self.lt(vm, 3, 1)? {
                if j < i {
                    return Err(self.invalid_order(vm));
                }
                Self::pop(vm, 1);
                j -= 1;
                self.geti(vm, j.into())?;
            }
            if j < i {
                Self::pop(vm, 1);
                self.set2(vm, (up - 1).into(), i.into())?;
                return Ok(i);
            }
            self.set2(vm, i.into(), j.into())?;
        }
    }

    /// 5.3+ `auxsort` on `unsigned int` indices, with PUC's randomized pivot
    /// once a partition comes out badly unbalanced.
    pub(super) fn auxsort(
        &self,
        vm: &mut Vm,
        mut lo: u32,
        mut up: u32,
        mut rnd: u32,
    ) -> Result<(), LuaError> {
        while lo < up {
            self.geti(vm, lo.into())?;
            self.geti(vm, up.into())?;
            if self.lt(vm, 1, 2)? {
                self.set2(vm, lo.into(), up.into())?;
            } else {
                Self::pop(vm, 2);
            }
            if up - lo == 1 {
                return Ok(());
            }
            let mut p = if up - lo < 100 || rnd == 0 {
                (lo + up) / 2
            } else {
                let r4 = (up - lo) / 4;
                let r = if vm.version() == V::Lua53 {
                    rnd
                } else {
                    rnd ^ lo ^ up
                };
                r % (r4 * 2) + (lo + r4)
            };
            self.geti(vm, p.into())?;
            self.geti(vm, lo.into())?;
            if self.lt(vm, 2, 1)? {
                self.set2(vm, p.into(), lo.into())?;
            } else {
                Self::pop(vm, 1);
                self.geti(vm, up.into())?;
                if self.lt(vm, 1, 2)? {
                    self.set2(vm, p.into(), up.into())?;
                } else {
                    Self::pop(vm, 2);
                }
            }
            if up - lo == 2 {
                return Ok(());
            }
            self.geti(vm, p.into())?;
            let pivot = Self::at(vm, 1);
            Self::push(vm, pivot)?;
            self.geti(vm, (up - 1).into())?;
            self.set2(vm, p.into(), (up - 1).into())?;
            p = self.partition(vm, lo, up)?;
            let n;
            if p - lo < up - p {
                self.auxsort(vm, lo, p - 1, rnd)?;
                n = p - lo;
                lo = p + 1;
            } else {
                self.auxsort(vm, p + 1, up, rnd)?;
                n = up - p;
                up = p - 1;
            }
            if up.wrapping_sub(lo) / 128 > n {
                rnd = randomize_pivot();
            }
        }
        Ok(())
    }
}

/// PUC `l_randomizePivot`: any cheap varying value (5.3/5.4 mix `clock()`
/// and `time()`).
fn randomize_pivot() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() ^ d.as_secs() as u32)
}
