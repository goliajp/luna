//! v1.2 Track B — `LuaUserdata` trait sugar with arithmetic metamethods.
//!
//! Demonstrates a value-style Vec3 host type whose `+` / `-` / `tostring`
//! all work from Lua via the trait builder's `add_meta_method`. The
//! type is exposed under the global name `Vec3` so scripts can write
//! `Vec3.new(1, 2, 3)`.
//!
//! Run: `cargo run --example userdata_vec3 -p luna-jit`

use luna_core::runtime::Value;
use luna_core::vm::{LuaUserdata, MetaMethod, UserdataMethods};
use luna_jit::Lua;

#[derive(Copy, Clone, Debug)]
struct Vec3 {
    x: f64,
    y: f64,
    z: f64,
}

impl LuaUserdata for Vec3 {
    fn type_name() -> &'static str {
        "Vec3"
    }

    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        // Component accessors (call-syntax — `v:x()`).
        m.add_method("x", |_vm, this, ()| Ok::<_, _>(this.x));
        m.add_method("y", |_vm, this, ()| Ok::<_, _>(this.y));
        m.add_method("z", |_vm, this, ()| Ok::<_, _>(this.z));

        // length, dot
        m.add_method("len", |_vm, this, ()| {
            Ok::<_, _>((this.x * this.x + this.y * this.y + this.z * this.z).sqrt())
        });

        // Static constructor reachable as Vec3.new(x, y, z).
        m.add_function(
            "new",
            |vm, (x, y, z): (f64, f64, f64)| -> Result<luna_core::runtime::Value, _> {
                Ok(vm.create_userdata(Vec3 { x, y, z }))
            },
        );

        // __add: v + w  — both operands are Vec3 userdata.
        m.add_meta_method(
            MetaMethod::Add,
            |vm,
             this,
             (rhs,): (luna_core::runtime::Value,)|
             -> Result<luna_core::runtime::Value, _> {
                let rhs = extract_vec3(rhs)
                    .ok_or_else(|| vm.rt_err("__add expected Vec3 on the right-hand side"))?;
                Ok(vm.create_userdata(Vec3 {
                    x: this.x + rhs.x,
                    y: this.y + rhs.y,
                    z: this.z + rhs.z,
                }))
            },
        );

        // __sub: v - w
        m.add_meta_method(
            MetaMethod::Sub,
            |vm,
             this,
             (rhs,): (luna_core::runtime::Value,)|
             -> Result<luna_core::runtime::Value, _> {
                let rhs = extract_vec3(rhs)
                    .ok_or_else(|| vm.rt_err("__sub expected Vec3 on the right-hand side"))?;
                Ok(vm.create_userdata(Vec3 {
                    x: this.x - rhs.x,
                    y: this.y - rhs.y,
                    z: this.z - rhs.z,
                }))
            },
        );

        // __tostring: nice printing.
        m.add_meta_method(MetaMethod::ToString, |_vm, this, ()| {
            Ok::<_, _>(format!("Vec3({}, {}, {})", this.x, this.y, this.z))
        });
    }
}

fn extract_vec3(v: Value) -> Option<Vec3> {
    match v {
        Value::Userdata(g) => {
            // SAFETY: single-threaded heap, pointer is live.
            let r = unsafe { &*g.as_ptr() };
            r.downcast::<Vec3>().copied()
        }
        _ => None,
    }
}

fn main() {
    let mut lua = Lua::new();
    lua.open_base();

    // Expose Vec3 as a global table (= the metatable itself), so
    // scripts can do `Vec3.new(1, 2, 3)` to call the static fn.
    {
        let mt = lua.vm().register_userdata::<Vec3>().expect("register Vec3");
        lua.vm()
            .set_global("Vec3", Value::Table(mt))
            .expect("expose Vec3");
    }

    // Construct + arithmetic from Lua.
    let s: String = lua
        .eval(
            r#"
            local a = Vec3.new(1.0, 2.0, 3.0)
            local b = Vec3.new(4.0, 5.0, 6.0)
            local c = a + b
            return tostring(c)
        "#,
        )
        .unwrap();
    assert_eq!(s, "Vec3(5, 7, 9)");
    println!("1. (1,2,3) + (4,5,6) = {s}");

    let s: String = lua
        .eval(
            r#"
            local a = Vec3.new(10.0, 0.0, 0.0)
            local b = Vec3.new(3.0, 4.0, 0.0)
            return tostring(a - b)
        "#,
        )
        .unwrap();
    assert_eq!(s, "Vec3(7, -4, 0)");
    println!("2. (10,0,0) - (3,4,0) = {s}");

    let l: f64 = lua.eval("return Vec3.new(3.0, 4.0, 0.0):len()").unwrap();
    assert!((l - 5.0).abs() < 1e-9);
    println!("3. |(3,4,0)| = {l}");

    println!("\nuserdata_vec3: all checks passed.");
}
