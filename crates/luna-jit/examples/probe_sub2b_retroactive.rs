//! v2.1 Path D Phase 1E.2 — retroactive prototype: does the Phase 1C
//! ISLE redundant-store DCE rule fire on the **real** sub-2B Phase F
//! shape? The Phase 1D integration test
//! (`crates/luna-jit/tests/path_d_isle_dse_integration.rs`) covered a
//! deliberately minimal fixture (block0-only, two trusted stores, one
//! `imul` between). Sub-2B Phase F's actual `move_then_mul` body had
//! extra structural features the minimal fixture didn't model:
//!
//! 1. **Multi-block layout**: the dual-write happened in block1, with a
//!    block0 prelude that pre-loaded Variable shadows. The Phase 1C rule
//!    is "same-block only" per the Phase 1D handoff doc §9 deferral
//!    list, so this prototype verifies the rule still fires when the
//!    dual-write is itself contained in block1.
//! 2. **Op::Move LOAD before the prior store**: sub-2B Phase F IR (§2.4)
//!    showed `v8 = load.i64 notrap aligned v7+24` immediately preceding
//!    the prior store. That load is to a DIFFERENT offset (`+24` vs
//!    `+0`) so it shouldn't conflict with same-MemoryLoc tracking, but
//!    we model it to make sure the rule's alias filter actually treats
//!    different-offset loads as non-conflicting.
//! 3. **Variable-shadowed `imul` operands**: sub-2B's Op::Mul read from
//!    Variable shadows that were def_var'd in block0. FunctionBuilder's
//!    use_var resolves to the SSA Value from block0 — no IR is emitted
//!    at the use site. So between the two stores in block1 there are
//!    literally just `imul` (pure, no can_trap). Same shape as the
//!    Phase 1D fixture but reached via a less-direct path.
//! 4. **Trailing deopt store-back loop**: sub-2B Phase F had 4 more
//!    trusted stores after the dual-write to `v0+8`, `v0+16`, `v0+24`,
//!    `v0+32`. Different offsets from the dual-write target (`v0+0`),
//!    so they're different MemoryLocs and the rule shouldn't see them
//!    as candidates. Modelled to confirm.
//!
//! ### Run
//!
//! ```sh
//! cargo run --release --example probe_sub2b_retroactive -p luna-jit
//! ```
//!
//! Exit code 0 = rule fired (prior store DCE'd). Non-zero = rule did
//! NOT fire (prior store survives). The probe prints the post-optimize
//! IR either way so the Phase 1E verdict doc can paste the extract
//! directly.
//!
//! ### Phase 1F handoff
//!
//! If the rule fires here, sub-2B's predicted 4-5 surviving stores per
//! migrated arm × 1000-iter loop = ~6-7 µs token_bucket_1k regression
//! is **retroactively recoverable**: the dual-write pattern can be
//! re-applied and the redundant prior stores will get DCE'd, so the
//! shape we previously had to revert at Phase F empirical NEGATIVE now
//! becomes Phase 1F-viable. Phase 1F should re-apply the sub-2B Phase
//! C/D/E op-arm migrations on a temp branch and run the
//! `token_bucket_1k` bench against sub-1 baseline.

use cranelift_codegen::{
    Context,
    control::ControlPlane,
    ir::{AbiParam, Function, InstBuilder, MemFlags, Opcode, Signature, UserFuncName, types},
    isa::CallConv,
    settings::{self, Configurable},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

fn build_isa() -> cranelift_codegen::isa::OwnedTargetIsa {
    let mut flag_builder = settings::builder();
    flag_builder.set("opt_level", "speed").unwrap();
    cranelift_native::builder()
        .expect("native ISA builder")
        .finish(settings::Flags::new(flag_builder))
        .expect("finish ISA")
}

/// Count `store` opcodes across the whole function.
fn count_stores(func: &Function) -> usize {
    func.layout
        .blocks()
        .flat_map(|b| func.layout.block_insts(b))
        .filter(|inst| matches!(func.dfg.insts[*inst].opcode(), Opcode::Store))
        .count()
}

/// Builds a faithful reproduction of sub-2B Phase F's `move_then_mul`
/// body. See the module doc above for the side-by-side comparison vs.
/// the minimal Phase 1D fixture.
fn build_sub2b_phase_f_faithful() -> Function {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64)); // reg_state ptr
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);

    let mut fbctx = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut func, &mut fbctx);

    // Declare 5 Variables (one per slot 0..4) for the Variable shadow path
    // that sub-2B used. These are scalars in SSA; use_var emits no IR.
    // Vendored fork API: `declare_var(ty) -> Variable` (auto-assigned id).
    let reg0 = bcx.declare_var(types::I64);
    let reg1 = bcx.declare_var(types::I64);
    let reg2 = bcx.declare_var(types::I64);
    let reg3 = bcx.declare_var(types::I64);
    let reg4 = bcx.declare_var(types::I64);

    let block0 = bcx.create_block();
    let block1 = bcx.create_block();
    bcx.append_block_params_for_function_params(block0);
    bcx.switch_to_block(block0);
    bcx.seal_block(block0);

    let reg_state = bcx.block_params(block0)[0];

    // ----- Prelude loads + Variable shadow init (block0) -----
    let v_prelude_0 = bcx
        .ins()
        .load(types::I64, MemFlags::trusted(), reg_state, 0);
    let v_prelude_1 = bcx
        .ins()
        .load(types::I64, MemFlags::trusted(), reg_state, 8);
    let v_prelude_2 = bcx
        .ins()
        .load(types::I64, MemFlags::trusted(), reg_state, 16);
    let v_prelude_3 = bcx
        .ins()
        .load(types::I64, MemFlags::trusted(), reg_state, 24);
    let v_prelude_4 = bcx
        .ins()
        .load(types::I64, MemFlags::trusted(), reg_state, 32);
    bcx.def_var(reg0, v_prelude_0);
    bcx.def_var(reg1, v_prelude_1);
    bcx.def_var(reg2, v_prelude_2);
    bcx.def_var(reg3, v_prelude_3);
    bcx.def_var(reg4, v_prelude_4);
    bcx.ins().jump(block1, &[]);

    // ----- block1: Op::Move + Op::Mul dual-write + deopt store-back -----
    bcx.switch_to_block(block1);
    bcx.seal_block(block1);

    // Op::Move R[0] = R[3] — current_base_addr LOAD path from sub-2B Phase C.
    let v_move_src = bcx
        .ins()
        .load(types::I64, MemFlags::trusted(), reg_state, 24);
    // PRIOR store — the one sub-2B Phase F empirically observed surviving.
    bcx.ins()
        .store(MemFlags::trusted(), v_move_src, reg_state, 0);

    // Op::Mul R[0] = R[1] * R[2] — reads Variable shadow, no IR at use site.
    let v_lhs = bcx.use_var(reg1);
    let v_rhs = bcx.use_var(reg2);
    let v_mul = bcx.ins().imul(v_lhs, v_rhs);
    // CURRENT store — must survive.
    bcx.ins().store(MemFlags::trusted(), v_mul, reg_state, 0);

    // Deopt store-back loop: 4 trailing trusted stores to different offsets.
    bcx.ins().store(MemFlags::trusted(), v_lhs, reg_state, 8);
    bcx.ins().store(MemFlags::trusted(), v_rhs, reg_state, 16);
    bcx.ins()
        .store(MemFlags::trusted(), v_move_src, reg_state, 24);
    bcx.ins()
        .store(MemFlags::trusted(), v_prelude_4, reg_state, 32);

    let v_zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[v_zero]);

    bcx.finalize();
    func
}

fn main() {
    let isa = build_isa();
    let func = build_sub2b_phase_f_faithful();

    println!("=== sub-2B Phase F faithful fixture — PRE-OPTIMIZE IR ===");
    println!("{}", func.display());

    let stores_before = count_stores(&func);
    println!("Stores before optimize: {stores_before}\n");

    let mut ctx = Context::for_function(func);
    let mut ctrl_plane = ControlPlane::default();
    ctx.optimize(isa.as_ref(), &mut ctrl_plane)
        .expect("optimize");

    println!("=== POST-OPTIMIZE IR ===");
    println!("{}", ctx.func.display());

    let stores_after = count_stores(&ctx.func);
    println!("Stores after optimize: {stores_after}");
    println!("Expected: 5 (PRIOR store DCE'd; CURRENT + 4 deopt store-backs survive)");

    // Stronger check: the SURVIVING store to reg_state+0 must carry the
    // imul result, not the load-of-+24 result. If the rule fires backward
    // (erases CURRENT instead of PRIOR), this assertion catches it.
    //
    // Look across all stores in all blocks; the unique store to offset 0
    // is the target. Stored value must be defined by Imul.
    let stores_to_offset0: Vec<_> = ctx
        .func
        .layout
        .blocks()
        .flat_map(|b| ctx.func.layout.block_insts(b))
        .filter(|inst| {
            let dfg = &ctx.func.dfg;
            if !matches!(dfg.insts[*inst].opcode(), Opcode::Store) {
                return false;
            }
            // The store opcode carries an `offset` field in InstructionData::Store.
            match dfg.insts[*inst] {
                cranelift_codegen::ir::InstructionData::Store { offset, .. } => {
                    let off_i32: i32 = offset.into();
                    off_i32 == 0
                }
                _ => false,
            }
        })
        .collect();

    println!("\nStores to offset 0: {}", stores_to_offset0.len());

    let rule_fired = stores_to_offset0.len() == 1;
    if rule_fired {
        let store_inst = stores_to_offset0[0];
        let stored_value = ctx.func.dfg.inst_args(store_inst)[0];
        let def_inst = ctx
            .func
            .dfg
            .value_def(stored_value)
            .inst()
            .expect("stored value should be inst-defined");
        let def_op = ctx.func.dfg.insts[def_inst].opcode();
        println!("Surviving store to offset 0 has stored value def opcode: {def_op:?}");

        if matches!(def_op, Opcode::Imul) {
            println!("\n*** SUCCESS: Phase 1C rule FIRED on sub-2B Phase F shape ***");
            println!(
                "PRIOR (Op::Move STORE) DCE'd; CURRENT (Op::Mul STORE with Imul value) survives."
            );
            println!("R3.3+ sub-2B is RETROACTIVELY VIABLE under the vendored fork.");
            std::process::exit(0);
        } else {
            eprintln!(
                "\n!!! WRONG-STORE DCE: only 1 store to offset 0 survives but its value \
                 is defined by {def_op:?} (expected Imul). Rule fired backward."
            );
            std::process::exit(2);
        }
    } else {
        eprintln!(
            "\n!!! Phase 1C rule did NOT fire: expected 1 store to offset 0, got {}.",
            stores_to_offset0.len()
        );
        eprintln!("Sub-2B retroactive viability: NEGATIVE on the multi-block + deopt-tail shape.");
        eprintln!(
            "Investigate: is the rule blocked by the trailing deopt stores' alias-budget consumption, or by the multi-block layout?"
        );
        std::process::exit(1);
    }
}
