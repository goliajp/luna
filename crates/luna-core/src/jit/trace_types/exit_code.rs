//! The value a trace function returns: where the interpreter goes on and
//! how the dispatcher restores the registers. The layout is a contract
//! between the code luna-jit generates and luna-core's dispatcher.
//!
//! | bits   | meaning                                                      |
//! |--------|--------------------------------------------------------------|
//! | 0–31   | the pc the interpreter resumes at                            |
//! | 32–52  | an inline side exit's site + 1, or under [`EXIT_TAGS_INDEX_BIT`] an index into the trace's `per_exit_tags`; 0 for a return through the trace's own tail |
//! | 53     | free                                                         |
//! | 54     | [`EXIT_TAGS_INDEX_BIT`]                                      |
//! | 55     | [`EXIT_KEEP_TFOR_VARS`]                                      |
//! | 56–62  | with bit 63 set: [`SIDE_SENT_DOWNREC_CODE`]; 0 otherwise     |
//! | 63     | a down-recursion stitch return                               |
//!
//! Bits 32–52 hold at most [`EXIT_SITE_LIMIT`] − 1: a trace with more
//! inline sites or tagged exits than that is not compiled.

/// The number of values bits 32–52 can hold.
pub const EXIT_SITE_LIMIT: u32 = 1 << 21;

/// Bits 32–52 of a return value, shifted down.
pub(crate) const EXIT_SITE_MASK: u64 = EXIT_SITE_LIMIT as u64 - 1;

/// Set when bits 32–52 hold an index into `per_exit_tags`.
pub const EXIT_TAGS_INDEX_BIT: u64 = 1 << 54;

/// Set to resume at a generic-for's TForLoop with its loop variables as
/// they are on the stack: the TForCall helper wrote them there with their
/// real tags, which need not be the ones the trace's registers were
/// compiled for.
pub const EXIT_KEEP_TFOR_VARS: u64 = 1 << 55;

/// The code a down-recursion trace's stitch return carries in bits 56–62,
/// with bit 63 set: the `TraceEnd::DownRec` close returns it when the
/// caller-pc guard hits (saved `[base-8]` matches the recorded
/// `target_proto`'s expected return pc), so the dispatcher walks the
/// trace's RetfRecord chain to materialise the inlined frames and goes on
/// in the stitched trace rather than in the interpreter at `head_pc`. No
/// other return sets bit 63.
pub const SIDE_SENT_DOWNREC_CODE: u32 = 0x10;

/// Whether `sentinel_code`, bits 56–62 of a return value with bit 63 set,
/// marks a down-recursion stitch return.
#[inline]
pub(crate) fn is_downrec_sentinel(sentinel_code: u32) -> bool {
    sentinel_code == SIDE_SENT_DOWNREC_CODE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_return_value_fields_do_not_overlap() {
        let fields = [
            0xFFFF_FFFF,
            EXIT_SITE_MASK << 32,
            EXIT_TAGS_INDEX_BIT,
            EXIT_KEEP_TFOR_VARS,
            0x7F << 56,
            1 << 63,
        ];
        let mut seen = 0u64;
        for f in fields {
            assert_eq!(seen & f, 0, "{f:#x} overlaps");
            seen |= f;
        }
        assert_eq!(seen, !(1 << 53));
        assert_eq!(u64::from(SIDE_SENT_DOWNREC_CODE) & !0x7F, 0);
    }
}
