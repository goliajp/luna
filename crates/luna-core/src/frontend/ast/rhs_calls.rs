//! RHS call walker and the metamethod-safety gate built on it.

use super::*;

// The compiler skips the Index-LHS object snapshot Move in `assign_stat` when
// the RHS of an assignment cannot re-bind the LHS local through a `__newindex`
// closure. The two helpers below model just the AST-side gate; the consumer
// (`assign_stat_can_skip_obj_snapshot`) combines them with
// `LocalVar.captured` and target-arity checks.

/// Classification of call sites discovered in an RHS expression tree.
///
/// The walker partitions expressions into three buckets; the snapshot gate
/// only accepts the bottom two (`None` and `OnlyKnownPure`) because
/// `UserOrUnknown` call sites can — through `__newindex` metamethod closure
/// capture — re-bind any local the closure has captured, which would
/// invalidate the Index-LHS snapshot elision.
///
/// This is intentionally an enum rather than a `bool`: a future relaxation
/// of the gate may distinguish between the two safe variants (e.g. to count
/// the third bucket for diagnostics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum RhsCallScan {
    /// No `Call` or `MethodCall` AST nodes anywhere in the walked tree.
    /// Pure arith, literals, names, indices, paren and unary/binary chains.
    None,
    /// Calls present, but every callee is a `<stdlib_module>.<field>` shape
    /// (e.g. `math.min`, `string.byte`, `table.unpack`) where
    /// `<stdlib_module>` is a known-immutable stdlib root (see
    /// [`is_known_pure_stdlib_root`]). A known-pure call cannot run
    /// user-supplied Lua code, so it cannot trigger `__newindex` re-binding
    /// of an outer local.
    OnlyKnownPure,
    /// At least one `Call` or `MethodCall` site whose callee is not a
    /// known-pure stdlib lookup — could invoke user-defined Lua that
    /// re-binds locals via captured upvalues. The gate must reject this case.
    UserOrUnknown,
}

impl RhsCallScan {
    fn join(self, other: RhsCallScan) -> RhsCallScan {
        use RhsCallScan::*;
        match (self, other) {
            (UserOrUnknown, _) | (_, UserOrUnknown) => UserOrUnknown,
            (OnlyKnownPure, _) | (_, OnlyKnownPure) => OnlyKnownPure,
            (None, None) => None,
        }
    }
}

/// Stdlib module names whose top-level fields the gate treats as
/// known-pure (no user-Lua callback path through __index / __newindex).
///
/// CONSERVATIVE — the list is restricted to modules whose entries (in
/// luna's stdlib) are direct Rust builtins. `os` / `io` / `debug` /
/// `package` / `_G` / `_ENV` are excluded: they expose callbacks (`io.read`
/// can yield, `debug.sethook` invokes Lua, `os.exit` runs `__close`, etc.)
/// or are user-mutable.
///
/// Left small intentionally; `select` (top-level builtin), `tostring`,
/// `tonumber`, etc. could be added via a flat names list.
pub(crate) fn is_known_pure_stdlib_root(text: &str) -> bool {
    matches!(text, "math" | "string" | "table")
}

/// Returns the [`RhsCallScan`] kind of the call sites reachable from `eid`.
///
/// Walks the AST tree rooted at `eid`, stopping at `Expr::Function` (its
/// body is its own scope and its `Call` ops fire only when the closure is
/// later invoked, not during RHS evaluation of the current statement).
///
/// O(n) in expression tree size — n is bounded by the source-character
/// count of the statement RHS. No allocation.
#[doc(hidden)]
#[allow(dead_code)]
pub fn walk_rhs_for_calls(chunk: &Chunk, eid: ExprId) -> RhsCallScan {
    walk_with(chunk, eid)
}

fn walk_with(chunk: &Chunk, eid: ExprId) -> RhsCallScan {
    use RhsCallScan::*;
    if crate::native_stack::is_low(crate::native_stack::RESERVE) {
        return UserOrUnknown;
    }
    let mut acc = None;
    // every expression to look at; no recursion, so a long chain is no
    // deeper
    let mut todo: Vec<ExprId> = vec![eid];
    while let Some(e) = todo.pop() {
        match chunk.expr(e) {
            // Leaves — no calls.
            Expr::Nil
            | Expr::True
            | Expr::False
            | Expr::Vararg
            | Expr::Int(_)
            | Expr::Float(_)
            | Expr::Str(_)
            | Expr::Name(_) => {}
            // Function literals don't *invoke* their body during RHS eval; the
            // value flowing out is the closure itself. (The closure may capture
            // upvalues but the capture happens at the `Closure` op, after the
            // snapshot site, and the captured local is rebound via the upvalue
            // *only* if the closure is later called — which the gate handles
            // by inspecting RHS Call ops, not Function literals.)
            Expr::Function(_) => {}
            Expr::Index { obj, key } => todo.extend([*obj, *key]),
            Expr::Paren(inner) => todo.push(*inner),
            Expr::UnOp { operand, .. } => todo.push(*operand),
            Expr::BinOp { lhs, rhs, .. } => todo.extend([*lhs, *rhs]),
            Expr::Table { fields, .. } => {
                for f in chunk.list(*fields) {
                    match f {
                        TableField::Item(e) | TableField::Named(_, e) => todo.push(*e),
                        TableField::Keyed(k, v) => todo.extend([*k, *v]),
                    }
                }
            }
            // the callee is classified in place, not walked
            Expr::Call { func, args, .. } => {
                acc = acc.join(classify_callee(chunk, *func));
                todo.extend_from_slice(chunk.list(*args));
            }
            // `obj:method(args)` is morally `obj.method(obj, args)`. Even if
            // `obj` is a known-pure stdlib root the *method dispatch* itself
            // may hit a __index path, so MethodCall is unconditionally
            // UserOrUnknown for the gate. Conservative; can be relaxed
            // later if obj is a literal stdlib lookup.
            Expr::MethodCall { obj, args, .. } => {
                acc = UserOrUnknown;
                todo.push(*obj);
                todo.extend_from_slice(chunk.list(*args));
            }
        }
        if acc == UserOrUnknown {
            return acc;
        }
    }
    acc
}

/// Classifies the callee of a `Call` node in isolation (does NOT recurse
/// into the args, which is the caller's job).
fn classify_callee(chunk: &Chunk, callee: ExprId) -> RhsCallScan {
    match chunk.expr(callee) {
        // `math.min(...)` shape: callee is Index{ Name(known_root), Str(field) }.
        Expr::Index { obj, key } => {
            let root_ok = matches!(
                chunk.expr(*obj),
                Expr::Name(n) if is_known_pure_stdlib_root(chunk.name(*n))
            );
            let key_is_str = matches!(chunk.expr(*key), Expr::Str(_));
            if root_ok && key_is_str {
                RhsCallScan::OnlyKnownPure
            } else {
                RhsCallScan::UserOrUnknown
            }
        }
        // Bare name callee (`f(...)`) or anything else: unknown.
        // `_ENV.math.min` and similar dotted globals do NOT match — keep
        // the gate strict, the consumer can opt in later.
        _ => RhsCallScan::UserOrUnknown,
    }
}

/// Metamethod-safety gate for the Index-LHS snapshot elision
///
/// Returns `true` only when, based purely on AST shape:
///
/// 1. `obj_eid` is a bare local-name reference (`Expr::Name`). The
///    elision requires the LHS object to be a stable, non-captured local. The
///    consumer is still expected to verify `captured == false` against
///    `LocalVar` at the call site — this gate only handles the AST half.
/// 2. `rhs_eid`'s call sites are all classified as
///    [`RhsCallScan::None`] or [`RhsCallScan::OnlyKnownPure`].
///
/// Returns `false` whenever the gate cannot prove safety (closed-world
/// pessimism — unknown = unsafe).
///
/// ## Conservative gaps (intentional, deferred)
///
/// - Closure-capture modeling is NOT performed. Even an
///   `OnlyKnownPure` RHS could in principle re-bind via a closure
///   stored on the metatable, but luna's stdlib does not call back
///   into Lua, so the gate is sound for the current stdlib surface.
/// - `_ENV.math.min` (dotted global through the env upvalue) is treated
///   as unsafe even though it ultimately resolves to the same builtin.
/// - Local `f` aliased to `math.min` (e.g. `local m = math.min; m(x)`)
///   is treated as unsafe. Variable-tracking is a separate subsystem.
/// - `obj` that is itself an Index (e.g. `t.a.b = v`) is rejected —
///   only direct local Index-LHS is in scope.
#[doc(hidden)]
#[allow(dead_code)]
pub fn metamethod_safe_for_index_lhs(chunk: &Chunk, obj_eid: ExprId, rhs_eid: ExprId) -> bool {
    matches!(chunk.expr(obj_eid), Expr::Name(_)) && rhs_calls_nothing_unknown(chunk, rhs_eid)
}

/// True when evaluating `rhs_eid` calls no function other than the known
/// pure builtins.
#[doc(hidden)]
pub fn rhs_calls_nothing_unknown(chunk: &Chunk, rhs_eid: ExprId) -> bool {
    matches!(
        walk_with(chunk, rhs_eid),
        RhsCallScan::None | RhsCallScan::OnlyKnownPure
    )
}
