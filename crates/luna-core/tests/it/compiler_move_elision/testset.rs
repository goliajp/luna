//! A value of `and` / `or` reaches its target through `TestSet`.

use super::*;

/// `and`'s short circuit past the right operand carries the left operand's
/// value into the target itself (PUC's `TESTSET`), and the right operand is
/// computed into the target: no Move on either path.
#[test]
fn and_short_circuit_carries_its_value_into_the_target() {
    let src = "local x, a, b, c = 0, ... x = a and b + c return x";
    let code = compile_main(src);
    assert!(
        code.iter()
            .any(|i| i.op() == Op::TestSet && i.a() == 0 && i.b() == 1),
        "{code:?}"
    );
    assert_eq!(count_moves(&code), 0, "{code:?}");
    for (a, want) in [("false", 0), ("1", 5)] {
        let src = format!("local x, a, b, c = 9, {a}, 2, 3 x = a and b + c return x or 0");
        assert_eq!(eval_int_all(&src), [want; 5], "{src}");
    }
}
