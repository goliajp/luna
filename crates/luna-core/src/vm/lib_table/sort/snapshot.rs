//! Sorting a copy of the elements when no comparison can be observed.

use super::*;

/// The elements, when sorting them cannot be observed: no comparator,
/// and `t[1..n]` all non-NaN numbers or all strings. Then no comparison
/// calls a metamethod or fails, and no element access reaches `__index`
/// or `__newindex` (every slot is present), so running the same
/// algorithm over a copy and storing the result gives the same table as
/// sorting in place, without a table access per step.
pub(super) fn pure_snapshot(vm: &Vm, tv: Value, n: i64) -> Result<Option<LVec<Value>>, LuaError> {
    let Value::Table(t) = tv else {
        return Ok(None);
    };
    // `n` may come from `__len`; only a real sequence fills the vector
    let mut out = LVec::new(vm.heap.mem());
    let Ok(room) = usize::try_from(n.min(t.len())) else {
        return Ok(None);
    };
    out.reserve(room)?;
    let mut strings = None;
    for i in 1..=n {
        let v = t.get(Value::Int(i));
        let is_str = match v {
            Value::Int(_) => false,
            Value::Float(f) if !f.is_nan() => false,
            Value::Str(_) => true,
            _ => return Ok(None),
        };
        if *strings.get_or_insert(is_str) != is_str {
            return Ok(None);
        }
        out.push(v)?;
    }
    Ok(Some(out))
}
