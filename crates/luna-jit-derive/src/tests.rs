use super::*;
use syn::parse_quote;

/// `#[lua_type_name = "Foo"]` parses to `Some(LitStr("Foo"))`.
/// Bare structs return `None` (caller falls back to ident).
#[test]
fn parse_lua_type_name_extracts_override() {
    let with_override: DeriveInput = parse_quote! {
        #[lua_type_name = "MyType"]
        struct Foo;
    };
    let got = parse_lua_type_name(&with_override.attrs);
    let lit = got.expect("override missing");
    assert_eq!(lit.value(), "MyType");

    let no_override: DeriveInput = parse_quote! {
        struct Bar;
    };
    assert!(
        parse_lua_type_name(&no_override.attrs).is_none(),
        "absent #[lua_type_name] must return None"
    );
}

/// `parse_lua_type_name` ignores attrs whose path doesn't match.
#[test]
fn parse_lua_type_name_ignores_unrelated_attrs() {
    let input: DeriveInput = parse_quote! {
        #[derive(Clone)]
        #[doc = "unrelated"]
        struct Baz;
    };
    assert!(parse_lua_type_name(&input.attrs).is_none());
}

/// `attr_string_arg_opt` parses `#[lua_method("get")]` → `Some("get")`,
/// `#[lua_method]` (bare) → `None`, and rejects `=` form.
#[test]
fn attr_string_arg_opt_parses_three_forms() {
    // Use a containing item then pull `attrs[0]` since attributes
    // on their own aren't a free-standing syn parse target.
    let with_arg: ImplItemFn = parse_quote! {
        #[lua_method("get")]
        fn dummy() {}
    };
    let lit = attr_string_arg_opt(&with_arg.attrs[0])
        .expect("parse")
        .expect("string arg present");
    assert_eq!(lit.value(), "get");

    let bare: ImplItemFn = parse_quote! {
        #[lua_method]
        fn dummy() {}
    };
    let none = attr_string_arg_opt(&bare.attrs[0]).expect("parse");
    assert!(none.is_none(), "bare #[lua_method] must give None");

    // `#[lua_method = "x"]` form should error.
    let eq_form: ImplItemFn = parse_quote! {
        #[lua_method = "x"]
        fn dummy() {}
    };
    let err = match attr_string_arg_opt(&eq_form.attrs[0]) {
        Err(e) => e,
        Ok(_) => panic!("`=` form must be rejected"),
    };
    assert!(
        err.to_string().contains("#[lua_method"),
        "err msg should mention the attribute, got {err}"
    );
}

/// `attr_ident_arg` parses `#[lua_meta_method(Add)]` → `Add` ident.
#[test]
fn attr_ident_arg_parses_meta_method_variant() {
    let method: ImplItemFn = parse_quote! {
        #[lua_meta_method(Add)]
        fn dummy() {}
    };
    let id = attr_ident_arg(&method.attrs[0]).expect("parse Add");
    assert_eq!(id.to_string(), "Add");

    // No arg → must error.
    let bare: ImplItemFn = parse_quote! {
        #[lua_meta_method]
        fn dummy() {}
    };
    assert!(
        attr_ident_arg(&bare.attrs[0]).is_err(),
        "missing ident arg must error"
    );
}

/// `classify_method` correctly classifies `#[lua_skip]` → Skip,
/// plain fn → Plain, `#[lua_method(...)]` → Register with the
/// proper builder name.
#[test]
fn classify_method_handles_skip_plain_and_register() {
    let skipped: ImplItemFn = parse_quote! {
        #[lua_skip]
        fn helper(&self) -> i64 { 0 }
    };
    assert!(matches!(
        classify_method(&skipped).expect("parse"),
        MethodKind::Skip
    ));

    let plain: ImplItemFn = parse_quote! {
        fn helper(&self) -> i64 { 0 }
    };
    assert!(matches!(
        classify_method(&plain).expect("parse"),
        MethodKind::Plain
    ));

    let with_method: ImplItemFn = parse_quote! {
        #[lua_method("get")]
        fn get(&self, _vm: &mut Vm, _args: ()) -> Result<i64, LuaError> { Ok(0) }
    };
    match classify_method(&with_method).expect("classify") {
        MethodKind::Register(reg) => {
            assert_eq!(reg.builder_method, "add_method");
            assert_eq!(reg.name.value(), "get");
            assert!(reg.has_receiver);
            assert!(reg.meta_variant.is_none());
        }
        _ => panic!("expected Register"),
    }
}

/// `classify_method` rejects `#[lua_method]` on a receiver-less fn
/// (only `#[lua_function]` is allowed without `self`).
#[test]
fn classify_method_rejects_method_without_receiver() {
    let bad: ImplItemFn = parse_quote! {
        #[lua_method("oops")]
        fn no_self(_vm: &mut Vm, _args: ()) -> Result<i64, LuaError> { Ok(0) }
    };
    let err = match classify_method(&bad) {
        Err(e) => e,
        Ok(_) => panic!("missing self must error"),
    };
    assert!(
        err.to_string().contains("receiver"),
        "err msg should mention receiver requirement, got: {err}"
    );
}

/// `classify_method` rejects `#[lua_function]` on a fn with `self`
/// (the dual is enforced — function = static, method = bound).
#[test]
fn classify_method_rejects_function_with_receiver() {
    let bad: ImplItemFn = parse_quote! {
        #[lua_function("oops")]
        fn has_self(&self, _vm: &mut Vm, _args: ()) -> Result<i64, LuaError> { Ok(0) }
    };
    let err = match classify_method(&bad) {
        Err(e) => e,
        Ok(_) => panic!("self on lua_function must error"),
    };
    assert!(
        err.to_string().contains("self"),
        "err msg should mention 'self', got: {err}"
    );
}

/// `classify_method` rejects fns that return `()` (no `Result`).
#[test]
fn classify_method_rejects_unit_return() {
    let bad: ImplItemFn = parse_quote! {
        #[lua_method("bad")]
        fn no_ret(&self, _vm: &mut Vm, _args: ()) {}
    };
    let err = match classify_method(&bad) {
        Err(e) => e,
        Ok(_) => panic!("unit return must error"),
    };
    assert!(
        err.to_string().contains("Result"),
        "err msg should mention 'Result', got: {err}"
    );
}

/// `classify_method` defaults the Lua-side name to the Rust fn
/// ident when the attribute carries no string literal.
#[test]
fn classify_method_defaults_name_to_fn_ident() {
    let m: ImplItemFn = parse_quote! {
        #[lua_method]
        fn width(&self, _vm: &mut Vm, _args: ()) -> Result<i64, LuaError> { Ok(0) }
    };
    match classify_method(&m).expect("classify") {
        MethodKind::Register(reg) => {
            assert_eq!(
                reg.name.value(),
                "width",
                "default name must match fn ident"
            );
        }
        _ => panic!("expected Register"),
    }
}

/// `classify_method` for `#[lua_meta_method(Add)]` captures the
/// variant ident and uses the `add_meta_method` builder.
#[test]
fn classify_method_meta_method_path() {
    let m: ImplItemFn = parse_quote! {
        #[lua_meta_method(Add)]
        fn __add(&self, _vm: &mut Vm, _args: ()) -> Result<i64, LuaError> { Ok(0) }
    };
    match classify_method(&m).expect("classify") {
        MethodKind::Register(reg) => {
            assert_eq!(reg.builder_method, "add_meta_method");
            assert_eq!(
                reg.meta_variant.as_ref().map(|i| i.to_string()).as_deref(),
                Some("Add")
            );
        }
        _ => panic!("expected Register"),
    }
}

/// `strip_helper_attrs` removes all `#[lua_*]` helper attrs in
/// place, leaving non-helper attrs untouched.
#[test]
fn strip_helper_attrs_removes_only_helpers() {
    let mut m: ImplItemFn = parse_quote! {
        #[lua_method("get")]
        #[inline]
        #[lua_skip]
        #[doc = "kept"]
        fn dummy() {}
    };
    assert_eq!(m.attrs.len(), 4);
    strip_helper_attrs(&mut m.attrs);
    // Only #[inline] and #[doc = "kept"] should remain.
    assert_eq!(m.attrs.len(), 2);
    let keep_paths: Vec<String> = m
        .attrs
        .iter()
        .map(|a| {
            a.path()
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default()
        })
        .collect();
    assert!(
        keep_paths.contains(&"inline".to_string()),
        "kept attrs: {keep_paths:?}"
    );
    assert!(
        keep_paths.contains(&"doc".to_string()),
        "kept attrs: {keep_paths:?}"
    );
}
