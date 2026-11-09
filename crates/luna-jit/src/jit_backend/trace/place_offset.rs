//! `LUNA_JIT_PLACE_OFFSET=<k>` moves where each piece of trace code starts
//! by `k % 8` times 16 bytes, for timings: a loop's speed can hang on where
//! its code falls in the fetch blocks, the same in every run of one binary,
//! and runs under several values average that out. Unset, or 0, nothing
//! moves.

/// How far each piece of trace code starts past where it would, in bytes;
/// read once.
pub(crate) fn place_offset() -> usize {
    static OFF: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| of_var(std::env::var("LUNA_JIT_PLACE_OFFSET").ok().as_deref()))
}

/// The offset the variable's value `v` asks for.
fn of_var(v: Option<&str>) -> usize {
    v.and_then(|s| s.trim().parse::<usize>().ok())
        .map_or(0, |k| k % 8 * 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_variable_moves_nothing() {
        assert_eq!(of_var(None), 0);
        assert_eq!(of_var(Some("0")), 0);
        assert_eq!(of_var(Some("x")), 0);
        if std::env::var_os("LUNA_JIT_PLACE_OFFSET").is_none() {
            assert_eq!(place_offset(), 0);
        }
    }

    #[test]
    fn steps_of_sixteen_bytes() {
        assert_eq!(of_var(Some("1")), 16);
        assert_eq!(of_var(Some(" 7 ")), 112);
        assert_eq!(of_var(Some("9")), 16);
    }
}
