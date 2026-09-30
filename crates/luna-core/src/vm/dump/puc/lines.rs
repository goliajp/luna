//! Line information of translated PUC 5.4 / 5.5 chunks.

/// Per-pc source lines from PUC 5.4/5.5's compressed form
/// (`luaG_getfuncline`): start at `line_defined` and add each signed delta,
/// except that `ABSLINEINFO` (-128) takes the line of the `abslineinfo`
/// entry recorded for that pc. Empty for a stripped chunk.
pub(super) fn rle_lines(
    dialect: &str,
    deltas: &[u8],
    abs: &[(u32, u32)],
    line_defined: u32,
    n_code: usize,
) -> Result<Vec<u32>, String> {
    if deltas.is_empty() {
        return Ok(Vec::new());
    }
    if deltas.len() != n_code {
        return Err(format!(
            "{dialect} chunk: {} line entries for {n_code} instructions",
            deltas.len()
        ));
    }
    let mut out = Vec::with_capacity(n_code);
    let mut line = line_defined as i64;
    let mut abs = abs.iter();
    for (pc, &d) in deltas.iter().enumerate() {
        if d as i8 == -128 {
            match abs.next() {
                Some(&(apc, aline)) if apc as usize == pc => line = aline as i64,
                _ => return Err(format!("{dialect} chunk: no absolute line for pc {pc}")),
            }
        } else {
            line += (d as i8) as i64;
        }
        out.push(u32::try_from(line).map_err(|_| format!("{dialect} chunk: line {line}"))?);
    }
    Ok(out)
}
