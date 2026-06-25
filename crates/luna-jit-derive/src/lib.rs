//! Procedural macros for the luna-jit Lua runtime (v1.3 Phase UD3).
//!
//! This crate ships two macros that, together, let an embedder turn a
//! plain Rust `struct` + `impl` block into a Lua-callable userdata
//! without typing the `luna_core::vm::UserdataMethods` builder
//! boilerplate:
//!
//! 1. `#[derive(LuaUserdata)]` — applied to a struct, emits the
//!    `luna_core::vm::LuaUserdata` trait impl with `type_name()`
//!    (overridable via `#[lua_type_name = "Foo"]`) and an
//!    `add_methods()` body that forwards to a hidden registry fn
//!    populated by [`macro@lua_userdata_methods`].
//! 2. `#[lua_userdata_methods]` — applied to an `impl T { ... }`
//!    block, walks the inner `fn` items and, for each one tagged with
//!    one of the helper attributes below, emits the matching
//!    `m.add_method(...)` / `m.add_field_method_get(...)` / etc. call.
//!
//! ## Helper attributes
//!
//! Applied to `fn` items inside the `#[lua_userdata_methods]` impl
//! block:
//!
//! | Attribute | Sig pattern | Lowers to |
//! |---|---|---|
//! | `#[lua_method("name")]` | `fn(&self, &mut Vm, A) -> Result<R, LuaError>` | `m.add_method` |
//! | `#[lua_method_mut("name")]` | `fn(&mut self, &mut Vm, A) -> Result<R, LuaError>` | `m.add_method_mut` |
//! | `#[lua_function("name")]` | `fn(&mut Vm, A) -> Result<R, LuaError>` (no receiver) | `m.add_function` |
//! | `#[lua_meta_method(Add)]` | `fn(&self, &mut Vm, A) -> Result<R, LuaError>` | `m.add_meta_method` |
//! | `#[lua_meta_method_mut(Concat)]` | `fn(&mut self, ...)` | `m.add_meta_method_mut` |
//! | `#[lua_field_get("name")]` | `fn(&self, &mut Vm) -> Result<R, LuaError>` (no args) | `m.add_field_method_get` |
//! | `#[lua_field_set("name")]` | `fn(&mut self, &mut Vm, A) -> Result<(), LuaError>` | `m.add_field_method_set` |
//! | `#[lua_skip]` | (any fn) | nothing — keeps fn in impl as a Rust-only helper |
//!
//! Names default to the Rust fn ident when omitted.
//!
//! ## ZST constraint
//!
//! The v1.2 trampoline accepts only ZST / fn-pointer-sized closures
//! (`luna_core::vm::userdata_trait::pack_zst_or_fnptr`). The derive
//! therefore emits **non-capturing** forwarding closures of the form
//! `|__vm, __this, __args| Self::name(__this, __vm, __args)` — these
//! are ZSTs whose only state is the static fn pointer to the named
//! associated function.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    Attribute, DeriveInput, FnArg, ImplItem, ImplItemFn, Item, ItemImpl, Lit, LitStr, Meta,
    MetaNameValue, ReturnType, Type, parse_macro_input, spanned::Spanned,
};

// ─────────────────────────────────────────────────────────────────────
// #[derive(LuaUserdata)]
// ─────────────────────────────────────────────────────────────────────

/// Emits the `luna_core::vm::LuaUserdata` trait impl for a struct.
///
/// The companion attribute macro [`macro@lua_userdata_methods`] (applied
/// to an `impl` block of the same struct) injects a hidden
/// `__luna_userdata_register` associated fn that the derive's
/// `add_methods()` body forwards to.
///
/// Accepts `#[lua_type_name = "Foo"]` at the struct level to override
/// the default `type_name()` (which falls back to the struct's Rust
/// ident). Embedders who don't need to override can omit the attr.
///
/// ## Example
///
/// ```ignore
/// use luna_jit_derive::{LuaUserdata, lua_userdata_methods};
/// use luna_core::vm::{LuaError, MetaMethod, UserdataMethods, Vm};
///
/// #[derive(LuaUserdata)]
/// #[lua_type_name = "Counter"]
/// struct Counter { value: i64 }
///
/// #[lua_userdata_methods]
/// impl Counter {
///     #[lua_method("get")]
///     fn get(&self, _vm: &mut Vm, _: ()) -> Result<i64, LuaError> {
///         Ok(self.value)
///     }
/// }
/// ```
#[proc_macro_derive(LuaUserdata, attributes(lua_type_name))]
pub fn derive_lua_userdata(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // Reject derive on enum/union — userdata payloads are struct-shaped
    // by convention (the v1.2 `UserdataPayload::Host(Box<dyn Any>)`
    // can hold any type, but `add_methods` is implementation-defined
    // and we want a clear error rather than runtime confusion).
    match &input.data {
        syn::Data::Struct(_) => {}
        syn::Data::Enum(_) => {
            return syn::Error::new(
                input.ident.span(),
                "#[derive(LuaUserdata)] only supports structs; got an enum. \
                 Wrap the enum in a newtype struct for now (v1.3 limitation).",
            )
            .to_compile_error()
            .into();
        }
        syn::Data::Union(_) => {
            return syn::Error::new(
                input.ident.span(),
                "#[derive(LuaUserdata)] only supports structs; got a union.",
            )
            .to_compile_error()
            .into();
        }
    }

    // Optional #[lua_type_name = "..."] override.
    let type_name_override = parse_lua_type_name(&input.attrs);

    // We emit a stub `add_methods` that forwards to a hidden
    // `__luna_userdata_register` fn. The companion attribute macro
    // injects that hidden fn when applied to the type's impl block;
    // if the embedder skipped that attribute (no methods), we provide
    // a default-empty registration by relying on the trait's default
    // `add_methods` — done by simply NOT emitting the body when the
    // registry fn is absent. But we don't know that at derive time, so
    // emit the forwarding call unconditionally and let the compiler
    // bark with a clear "no method named `__luna_userdata_register`" if
    // the embedder forgot.
    //
    // To make the zero-method case still work, gate the forwarding
    // call behind a `cfg(luna_userdata_register_present)` style trick?
    // Too fragile. Simpler: emit the registry call inside a trait
    // method that *defaults* to a no-op via a marker trait — but that
    // also adds complexity. Cleanest is to require pairing — document
    // it and let rustc give a clear error.

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let type_name_body = match type_name_override {
        Some(lit) => quote! { #lit },
        None => {
            let n = name.to_string();
            quote! { #n }
        }
    };

    let expanded = quote! {
        impl #impl_generics ::luna_core::vm::LuaUserdata for #name #ty_generics #where_clause {
            fn type_name() -> &'static str { #type_name_body }
            fn add_methods<__M: ::luna_core::vm::UserdataMethods<Self>>(__m: &mut __M) {
                <Self>::__luna_userdata_register(__m);
            }
        }
    };
    expanded.into()
}

fn parse_lua_type_name(attrs: &[Attribute]) -> Option<LitStr> {
    for attr in attrs {
        if !attr.path().is_ident("lua_type_name") {
            continue;
        }
        if let Meta::NameValue(MetaNameValue {
            value: syn::Expr::Lit(syn::ExprLit {
                lit: Lit::Str(s), ..
            }),
            ..
        }) = &attr.meta
        {
            return Some(s.clone());
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────
// #[lua_userdata_methods] — attribute macro on impl blocks
// ─────────────────────────────────────────────────────────────────────

/// Walks the methods of an `impl T { ... }` block and, for each one
/// tagged with a helper attribute (`#[lua_method("name")]` etc.),
/// emits the corresponding `UserdataMethods` builder call inside a
/// hidden `__luna_userdata_register` associated fn.
///
/// The `UserdataMethods` trait lives in
/// `::luna_core::vm::UserdataMethods` — the emitted code references it
/// by absolute path so the derive works for pure luna-core embedders
/// too. The intra-doc-link form is omitted because this proc-macro
/// crate cannot see luna_core in its `cargo doc` scope.
///
/// The original impl block is re-emitted unchanged (minus the helper
/// attributes themselves), so the named fns are still directly
/// callable from Rust.
#[proc_macro_attribute]
pub fn lua_userdata_methods(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemImpl);
    let self_ty = &*input.self_ty;

    let mut registrations: Vec<TokenStream2> = Vec::new();
    let mut errors: Vec<syn::Error> = Vec::new();

    for impl_item in &mut input.items {
        if let ImplItem::Fn(method) = impl_item {
            match classify_method(method) {
                Ok(MethodKind::Skip) => {
                    strip_helper_attrs(&mut method.attrs);
                }
                Ok(MethodKind::Register(reg)) => {
                    strip_helper_attrs(&mut method.attrs);
                    registrations.push(reg.emit(self_ty));
                }
                Ok(MethodKind::Plain) => {
                    // Unannotated fn — leave as plain Rust helper.
                }
                Err(e) => errors.push(e),
            }
        }
    }

    if !errors.is_empty() {
        let combined = errors
            .into_iter()
            .map(|e| e.to_compile_error())
            .collect::<TokenStream2>();
        return combined.into();
    }

    // Hidden registry fn — invoked from the derive-emitted
    // `add_methods` body. `#[doc(hidden)]` keeps it out of rustdoc.
    let register_fn = quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub fn __luna_userdata_register<__M: ::luna_core::vm::UserdataMethods<Self>>(__m: &mut __M) {
            #(#registrations)*
        }
    };

    // Append the registry fn to the impl block.
    let register_item: ImplItem = syn::parse2(register_fn).expect("registry fn parse");
    input.items.push(register_item);

    quote! { #input }.into()
}

// ─────────────────────────────────────────────────────────────────────
// Method classification
// ─────────────────────────────────────────────────────────────────────

enum MethodKind {
    Plain,
    Skip,
    Register(Registration),
}

struct Registration {
    builder_method: &'static str, // "add_method" / "add_method_mut" / ...
    name: LitStr,
    fn_ident: syn::Ident,
    meta_variant: Option<syn::Ident>, // for add_meta_method / _mut
    has_receiver: bool,
}

impl Registration {
    fn emit(&self, self_ty: &Type) -> TokenStream2 {
        let fn_ident = &self.fn_ident;
        let lua_name = &self.name;
        let builder = format_ident!("{}", self.builder_method);

        // Non-capturing forwarding closure — required by the v1.2
        // `pack_zst_or_fnptr` ZST/fn-pointer-only constraint. The
        // closure references `Self::ident` (a fn item, which IS a
        // ZST), so the closure itself stays ZST.
        if let Some(variant) = &self.meta_variant {
            // add_meta_method[_mut](MetaMethod::Variant, fwd)
            if self.has_receiver {
                quote! {
                    __m.#builder(
                        ::luna_core::vm::MetaMethod::#variant,
                        |__vm, __this, __args| <#self_ty>::#fn_ident(__this, __vm, __args),
                    );
                }
            } else {
                // Meta method without receiver doesn't make semantic
                // sense, but emit conservatively for clarity.
                quote! {
                    __m.#builder(
                        ::luna_core::vm::MetaMethod::#variant,
                        |__vm, _, __args| <#self_ty>::#fn_ident(__vm, __args),
                    );
                }
            }
        } else if self.builder_method == "add_function" {
            // No receiver; closure shape is (&mut Vm, A).
            quote! {
                __m.#builder(#lua_name, |__vm, __args| <#self_ty>::#fn_ident(__vm, __args));
            }
        } else if self.builder_method == "add_field_method_get" {
            // (&mut Vm, &T) — no args.
            quote! {
                __m.#builder(#lua_name, |__vm, __this| <#self_ty>::#fn_ident(__this, __vm));
            }
        } else {
            // add_method / add_method_mut / add_field_method_set —
            // closure shape is (&mut Vm, &T/&mut T, A).
            quote! {
                __m.#builder(#lua_name, |__vm, __this, __args| {
                    <#self_ty>::#fn_ident(__this, __vm, __args)
                });
            }
        }
    }
}

fn classify_method(method: &ImplItemFn) -> Result<MethodKind, syn::Error> {
    let mut found: Option<(&'static str, Option<LitStr>, Option<syn::Ident>)> = None;

    for attr in &method.attrs {
        let path = attr.path();

        if path.is_ident("lua_skip") {
            return Ok(MethodKind::Skip);
        }

        let mut try_simple = |bm: &'static str| -> Result<(), syn::Error> {
            let name_lit = attr_string_arg_opt(attr)?;
            found = Some((bm, name_lit, None));
            Ok(())
        };

        if path.is_ident("lua_method") {
            try_simple("add_method")?;
        } else if path.is_ident("lua_method_mut") {
            try_simple("add_method_mut")?;
        } else if path.is_ident("lua_function") {
            try_simple("add_function")?;
        } else if path.is_ident("lua_field_get") {
            try_simple("add_field_method_get")?;
        } else if path.is_ident("lua_field_set") {
            try_simple("add_field_method_set")?;
        } else if path.is_ident("lua_meta_method") {
            let variant = attr_ident_arg(attr)?;
            found = Some(("add_meta_method", None, Some(variant)));
        } else if path.is_ident("lua_meta_method_mut") {
            let variant = attr_ident_arg(attr)?;
            found = Some(("add_meta_method_mut", None, Some(variant)));
        }
    }

    let (builder_method, name_lit, meta_variant) = match found {
        Some(t) => t,
        None => return Ok(MethodKind::Plain),
    };

    // Default to the Rust fn ident as the Lua-side name.
    let name = name_lit
        .unwrap_or_else(|| LitStr::new(&method.sig.ident.to_string(), method.sig.ident.span()));

    // Sanity-check the receiver against the kind. We accept anything
    // for `lua_skip` (already handled). For `lua_function` we expect
    // NO receiver; for everything else with self, we expect one.
    let has_receiver = matches!(method.sig.inputs.first(), Some(FnArg::Receiver(_)));
    let expects_receiver = !matches!(builder_method, "add_function");
    if expects_receiver && !has_receiver {
        return Err(syn::Error::new(
            method.sig.ident.span(),
            format!(
                "#[lua_*] attribute lowering to `{}` requires a `&self` or `&mut self` receiver",
                builder_method
            ),
        ));
    }
    if !expects_receiver && has_receiver {
        return Err(syn::Error::new(
            method.sig.ident.span(),
            "#[lua_function] must NOT have a `self` receiver — it lowers to a static \
             `add_function` call. Use #[lua_method] for receiver-bearing methods.",
        ));
    }

    // Light return-type sanity (informational; full type-check happens
    // at the use site when the generated forwarding closure compiles).
    if let ReturnType::Default = method.sig.output {
        return Err(syn::Error::new(
            method.sig.output.span(),
            "luna userdata methods must return `Result<R, LuaError>`; got `()`",
        ));
    }

    Ok(MethodKind::Register(Registration {
        builder_method,
        name,
        fn_ident: method.sig.ident.clone(),
        meta_variant,
        has_receiver,
    }))
}

/// Parse `#[lua_method("name")]` → `Some("name")`, `#[lua_method]`
/// (no arg) → `None`.
fn attr_string_arg_opt(attr: &Attribute) -> Result<Option<LitStr>, syn::Error> {
    match &attr.meta {
        Meta::Path(_) => Ok(None),
        Meta::List(_) => {
            // Try to parse as a single string literal inside the parens.
            let s: LitStr = attr.parse_args().map_err(|e| {
                syn::Error::new(
                    attr.span(),
                    format!(
                        "expected a single string-literal argument, e.g. \
                         #[lua_method(\"name\")]; got: {e}"
                    ),
                )
            })?;
            Ok(Some(s))
        }
        Meta::NameValue(_) => Err(syn::Error::new(
            attr.span(),
            "expected #[lua_method(\"name\")] or bare #[lua_method], \
             not #[lua_method = \"...\"]",
        )),
    }
}

/// Parse `#[lua_meta_method(Add)]` → `Add` ident.
fn attr_ident_arg(attr: &Attribute) -> Result<syn::Ident, syn::Error> {
    attr.parse_args().map_err(|e| {
        syn::Error::new(
            attr.span(),
            format!(
                "expected a single MetaMethod ident, e.g. #[lua_meta_method(Add)]; \
                 got: {e}"
            ),
        )
    })
}

fn strip_helper_attrs(attrs: &mut Vec<Attribute>) {
    attrs.retain(|a| {
        let p = a.path();
        !(p.is_ident("lua_method")
            || p.is_ident("lua_method_mut")
            || p.is_ident("lua_function")
            || p.is_ident("lua_field_get")
            || p.is_ident("lua_field_set")
            || p.is_ident("lua_meta_method")
            || p.is_ident("lua_meta_method_mut")
            || p.is_ident("lua_skip"))
    });
}

// Suppress dead-code warning for `Item` import; reserved for a future
// "derive on impl block" diagnostic.
#[allow(dead_code)]
fn _reserved(_: Item) {}

// ─────────────────────────────────────────────────────────────────────
// v2.0 Phase 5 Track CV gap fill — direct unit tests for the helper
// fns that the proc-macro entry points call into.
//
// Proc-macro crates can't be `use`d from integration tests (rustc
// loads them at compile time, not as runtime libraries), so the
// helper fns get coverage here. Integration coverage for the
// `#[derive(LuaUserdata)]` + `#[lua_userdata_methods]` expansion
// already lives in `crates/luna-jit/tests/userdata_derive.rs`; this
// module fills the gap on the parse / classify / strip helpers.
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
}
