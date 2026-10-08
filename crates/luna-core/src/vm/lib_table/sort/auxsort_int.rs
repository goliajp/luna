//! ≤5.2's sort on C `int` indices.

use super::*;

impl Sorter {
    /// ≤5.2 `auxsort` on C `int` indices, computed in `i64` so that the
    /// middle of a range ending at `INT_MAX` does not overflow. 5.1 detects a bad comparator only
    /// once the scan has run past the range (`i > u`, `j < l`); 5.2 one
    /// step earlier.
    pub(crate) fn auxsort_int(&self, vm: &mut Vm, mut l: i64, mut u: i64) -> Result<(), LuaError> {
        let strict = vm.version() == V::Lua52;
        while l < u {
            self.geti(vm, l)?;
            self.geti(vm, u)?;
            if self.lt(vm, 1, 2)? {
                self.set2(vm, l, u)?;
            } else {
                Self::pop(vm, 2);
            }
            if u - l == 1 {
                break;
            }
            let mut i = (l + u) / 2;
            self.geti(vm, i)?;
            self.geti(vm, l)?;
            if self.lt(vm, 2, 1)? {
                self.set2(vm, i, l)?;
            } else {
                Self::pop(vm, 1);
                self.geti(vm, u)?;
                if self.lt(vm, 1, 2)? {
                    self.set2(vm, i, u)?;
                } else {
                    Self::pop(vm, 2);
                }
            }
            if u - l == 2 {
                break;
            }
            self.geti(vm, i)?;
            let pivot = Self::at(vm, 1);
            Self::push(vm, pivot);
            self.geti(vm, u - 1)?;
            self.set2(vm, i, u - 1)?;
            i = l;
            let mut j = u - 1;
            loop {
                i += 1;
                self.geti(vm, i)?;
                while self.lt(vm, 1, 2)? {
                    if if strict { i >= u } else { i > u } {
                        return Err(invalid_order(vm));
                    }
                    Self::pop(vm, 1);
                    i += 1;
                    self.geti(vm, i)?;
                }
                j -= 1;
                self.geti(vm, j)?;
                while self.lt(vm, 3, 1)? {
                    if if strict { j <= l } else { j < l } {
                        return Err(invalid_order(vm));
                    }
                    Self::pop(vm, 1);
                    j -= 1;
                    self.geti(vm, j)?;
                }
                if j < i {
                    Self::pop(vm, 3);
                    break;
                }
                self.set2(vm, i, j)?;
            }
            self.geti(vm, u - 1)?;
            self.geti(vm, i)?;
            self.set2(vm, u - 1, i)?;
            // recurse into the smaller half [j..i], loop on the larger [l..u]
            if i - l < u - i {
                j = l;
                i -= 1;
                l = i + 2;
            } else {
                j = i + 1;
                i = u;
                u = j - 2;
            }
            self.auxsort_int(vm, j, i)?;
        }
        Ok(())
    }
}
