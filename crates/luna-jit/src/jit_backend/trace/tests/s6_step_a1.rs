//! `ExitTag::Nil` variant exists and
//! `kinds_to_exit_tags` produces it for `RegKind::Nil`, which the
//! LoadNil emit relies on (it writes Nil to a slot whose entry
//! tag may not be Nil).
use super::*;

#[test]
fn regkind_nil_maps_to_exittag_nil() {
    let kinds = vec![
        RegKind::Unset,
        RegKind::Int,
        RegKind::Nil,
        RegKind::Float,
        RegKind::Nil,
    ];
    let tags = kinds_to_exit_tags(&kinds);
    assert_eq!(tags.len(), 5);
    assert!(matches!(tags[0], ExitTag::Untouched));
    assert!(matches!(tags[1], ExitTag::Int));
    assert!(matches!(tags[2], ExitTag::Nil));
    assert!(matches!(tags[3], ExitTag::Float));
    assert!(matches!(tags[4], ExitTag::Nil));
}
