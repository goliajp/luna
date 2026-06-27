//! v2.1 Path D Phase 1D — integration test for the ISLE redundant-store DCE
//! rule landed in Phase 1C (`crates/vendored/wasmtime/cranelift/codegen/src/
//! opts/skeleton.isle:48-71`).
//!
//! Builds a Cranelift `Function` that replicates the sub-2B Phase F shape
//! (`.dev/rfcs/v2.0-track-r-r3-3-sub2B-verdict.md` §2.4):
//!
//! ```text
//! block1:
//!     store notrap aligned v_stale, v_addr   ;; Op::Move STORE
//!     v_mul = imul.i64 ...                   ;; Op::Mul body (pure)
//!     store v_mul, v_addr                    ;; Op::Mul STORE
//! ```
//!
//! Two consecutive `MemFlags::trusted()` stores to the same address (offset 0)
//! with only pure-arithmetic between them. Phase 1C's rule should erase the
//! prior store, leaving only the `imul` store behind.
//!
//! ### Why IR inspection instead of `EgraphPass::stats().skeleton_inst_dse`
//!
//! The `skeleton_inst_dse` counter on `cranelift_codegen::egraph::Stats` is
//! `pub(crate)`, and Phase 1D's scope hard-bars editing existing Cranelift
//! Rust source (only the new filetest `.clif` is allowed in the fork this
//! phase). Direct IR-shape inspection is **strictly stronger evidence** than
//! a counter bump anyway: it proves the actual layout transformation, not
//! just that some internal book-keeping incremented. The companion filetest
//! at `crates/vendored/wasmtime/cranelift/filetests/filetests/egraph/dse.clif`
//! provides a second, independent verification surface via `clif-util`.

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

fn count_stores(func: &Function) -> usize {
    func.layout
        .blocks()
        .flat_map(|b| func.layout.block_insts(b))
        .filter(|inst| matches!(func.dfg.insts[*inst].opcode(), Opcode::Store))
        .count()
}

/// Mirrors the sub-2B Phase F `move_then_mul_propagates_via_move` block1
/// shape. `v_arg` is a runtime block param so the `imul` cannot be constant-
/// folded into an `iconst` (which would defeat the value-provenance check
/// below).
fn build_move_then_mul_fixture() -> Function {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64)); // addr_ptr (slot 0 base)
    sig.params.push(AbiParam::new(types::I64)); // v_arg (Mul lhs, prevents folding)
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);

    let mut fbctx = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut func, &mut fbctx);
    let block0 = bcx.create_block();
    bcx.append_block_params_for_function_params(block0);
    bcx.switch_to_block(block0);
    bcx.seal_block(block0);

    let addr_ptr = bcx.block_params(block0)[0];
    let v_arg = bcx.block_params(block0)[1];

    // PRIOR store — the "Op::Move STORE" that sub-2B Phase F observed
    // surviving with cranelift 0.124's stock store-DCE.
    let v_stale = bcx.ins().iconst(types::I64, 999);
    bcx.ins().store(MemFlags::trusted(), v_stale, addr_ptr, 0);

    // Pure arithmetic between the two stores. No load to addr_ptr+0 so the
    // observed-bit on the prior store's `mem_values` entry must stay `false`.
    let v_six = bcx.ins().iconst(types::I64, 6);
    let v_mul = bcx.ins().imul(v_arg, v_six);

    // CURRENT store — the "Op::Mul STORE" that should subsume the prior.
    bcx.ins().store(MemFlags::trusted(), v_mul, addr_ptr, 0);

    bcx.ins().return_(&[v_mul]);
    bcx.finalize();
    func
}

#[test]
fn isle_dse_rule_fires_on_move_then_mul_shape() {
    let isa = build_isa();
    let func = build_move_then_mul_fixture();

    let stores_before = count_stores(&func);
    assert_eq!(
        stores_before,
        2,
        "fixture should start with exactly 2 stores; got {stores_before}\nIR:\n{}",
        func.display()
    );

    let mut ctx = Context::for_function(func);
    let mut ctrl_plane = ControlPlane::default();
    ctx.optimize(isa.as_ref(), &mut ctrl_plane)
        .expect("optimize");

    let stores_after = count_stores(&ctx.func);
    assert_eq!(
        stores_after,
        1,
        "Phase 1C ISLE rule should have removed the prior (dead) store; \
         got {stores_after} stores remaining.\nIR after optimize:\n{}",
        ctx.func.display()
    );

    // Stronger check: the surviving store must carry the `imul` result (the
    // SECOND store), not the `iconst 999` (the prior). Catches a wrong-store
    // erase regression where the rule would fire backwards.
    let surviving_store = ctx
        .func
        .layout
        .blocks()
        .flat_map(|b| ctx.func.layout.block_insts(b))
        .find(|inst| matches!(ctx.func.dfg.insts[*inst].opcode(), Opcode::Store))
        .expect("at least one store after optimize");
    let stored_value = ctx.func.dfg.inst_args(surviving_store)[0];
    let def_inst = ctx
        .func
        .dfg
        .value_def(stored_value)
        .inst()
        .expect("surviving stored value should be defined by an inst, not a block param");
    let def_op = ctx.func.dfg.insts[def_inst].opcode();
    assert_eq!(
        def_op,
        Opcode::Imul,
        "surviving store should write the imul result (the second store), \
         not the iconst 999 (the prior store); got def_op={def_op:?}\n\
         IR after optimize:\n{}",
        ctx.func.display()
    );
}

/// Negative control: an intervening must-aliased load between the two stores
/// flips the `observed` bit on the prior store's `mem_values` entry, which
/// Phase 1C's `find_dead_store_at` rejects (precondition 5). Both stores must
/// survive even though cranelift's load-to-store forwarding will erase the
/// intervening load itself.
#[test]
fn isle_dse_rule_does_not_fire_with_intervening_load() {
    let isa = build_isa();

    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 1), sig);

    let mut fbctx = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut func, &mut fbctx);
    let block0 = bcx.create_block();
    bcx.append_block_params_for_function_params(block0);
    bcx.switch_to_block(block0);
    bcx.seal_block(block0);
    let addr_ptr = bcx.block_params(block0)[0];

    let v_a = bcx.ins().iconst(types::I64, 7);
    bcx.ins().store(MemFlags::trusted(), v_a, addr_ptr, 0);
    // Intervening load forwards from the prior store -> flips observed bit.
    let v_forwarded = bcx.ins().load(types::I64, MemFlags::trusted(), addr_ptr, 0);
    let v_b = bcx.ins().iadd_imm(v_forwarded, 1);
    bcx.ins().store(MemFlags::trusted(), v_b, addr_ptr, 0);
    bcx.ins().return_(&[v_b]);
    bcx.finalize();

    let mut ctx = Context::for_function(func);
    let mut ctrl_plane = ControlPlane::default();
    ctx.optimize(isa.as_ref(), &mut ctrl_plane)
        .expect("optimize");

    let stores_after = count_stores(&ctx.func);
    assert_eq!(
        stores_after,
        2,
        "intervening must-aliased load should mark prior store observed; \
         both stores must survive. IR after optimize:\n{}",
        ctx.func.display()
    );
}

// -------------------------------------------------------------------------
// Phase 1G.B.5 — cross-block DSE integration cases.
//
// These exercise the Phase 1G.B.2 strict-chain check and Phase 1G.B.3
// deopt-safe relaxation that ship in vendored cranelift's
// `AliasAnalysis::find_dead_store_at`. The same-block Phase 1C cases
// above remain the regression floor.
// -------------------------------------------------------------------------

/// Positive — linear two-block strict chain. block0 stores `iconst 999`
/// and falls through (jump) to block1, which stores an `imul` result.
/// The Phase 1G.B.2 strict-chain check must accept (no can_trap, no
/// off-chain branches), so the prior store in block0 is dead.
#[test]
fn isle_dse_cross_block_strict_chain_fires() {
    let isa = build_isa();

    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64)); // addr
    sig.params.push(AbiParam::new(types::I64)); // v_arg
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 100), sig);

    let mut fbctx = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut func, &mut fbctx);

    let block0 = bcx.create_block();
    let block1 = bcx.create_block();
    bcx.append_block_params_for_function_params(block0);
    bcx.switch_to_block(block0);
    bcx.seal_block(block0);

    let addr = bcx.block_params(block0)[0];
    let v_arg = bcx.block_params(block0)[1];

    let v_stale = bcx.ins().iconst(types::I64, 999);
    bcx.ins().store(MemFlags::trusted(), v_stale, addr, 0);
    bcx.ins().jump(block1, &[]);

    bcx.switch_to_block(block1);
    bcx.seal_block(block1);
    let v_six = bcx.ins().iconst(types::I64, 6);
    let v_mul = bcx.ins().imul(v_arg, v_six);
    bcx.ins().store(MemFlags::trusted(), v_mul, addr, 0);
    bcx.ins().return_(&[v_mul]);
    bcx.finalize();

    let stores_before = count_stores(&func);
    assert_eq!(
        stores_before, 2,
        "fixture should start with 2 cross-block stores; got {stores_before}"
    );

    let mut ctx = Context::for_function(func);
    let mut ctrl_plane = ControlPlane::default();
    ctx.optimize(isa.as_ref(), &mut ctrl_plane)
        .expect("optimize");

    let stores_after = count_stores(&ctx.func);
    assert_eq!(
        stores_after,
        1,
        "Phase 1G.B.2 strict-chain check should drop the prior store \
         across the block0 -> block1 fall-through; got {stores_after}.\n\
         IR after optimize:\n{}",
        ctx.func.display()
    );
}

/// Positive — deopt-safe relaxation. block0 stores prior + brifs into
/// (block1=continue, block2=deopt). block1 holds the current store;
/// block2 contains a plain (non-notrap) Store to the same `(addr, 0)`
/// before returning. Phase 1G.B.3 must accept the off-chain block2 as
/// deopt-safe, so the prior store is dead.
#[test]
fn isle_dse_cross_block_deopt_safe_relaxation_fires() {
    let isa = build_isa();

    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64)); // addr
    sig.params.push(AbiParam::new(types::I64)); // v_arg
    sig.params.push(AbiParam::new(types::I32)); // cond
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 101), sig);

    let mut fbctx = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut func, &mut fbctx);

    let block0 = bcx.create_block();
    let block1 = bcx.create_block();
    let block2 = bcx.create_block();
    bcx.append_block_params_for_function_params(block0);
    bcx.switch_to_block(block0);
    bcx.seal_block(block0);

    let addr = bcx.block_params(block0)[0];
    let v_arg = bcx.block_params(block0)[1];
    let cond = bcx.block_params(block0)[2];

    // PRIOR store (block0).
    let v_stale = bcx.ins().iconst(types::I64, 999);
    bcx.ins().store(MemFlags::trusted(), v_stale, addr, 0);
    bcx.ins().brif(cond, block1, &[], block2, &[]);

    // continue block (block1) — on dom chain.
    bcx.switch_to_block(block1);
    bcx.seal_block(block1);
    let v_six = bcx.ins().iconst(types::I64, 6);
    let v_mul = bcx.ins().imul(v_arg, v_six);
    bcx.ins().store(MemFlags::trusted(), v_mul, addr, 0); // CURRENT
    bcx.ins().return_(&[v_mul]);

    // deopt block (block2) — off dom chain. Plain Store overwrites
    // the slot before any external observer (matching luna's
    // `emit_store_back_and_return_pc` semantics).
    bcx.switch_to_block(block2);
    bcx.seal_block(block2);
    let v_writeback = bcx.ins().iconst(types::I64, 42);
    bcx.ins().store(MemFlags::new(), v_writeback, addr, 0);
    bcx.ins().return_(&[v_writeback]);

    bcx.finalize();

    let stores_before = count_stores(&func);
    assert_eq!(
        stores_before, 3,
        "fixture should start with 3 stores (prior + current + deopt); \
         got {stores_before}"
    );

    let mut ctx = Context::for_function(func);
    let mut ctrl_plane = ControlPlane::default();
    ctx.optimize(isa.as_ref(), &mut ctrl_plane)
        .expect("optimize");

    let stores_after = count_stores(&ctx.func);
    assert_eq!(
        stores_after,
        2,
        "Phase 1G.B.3 deopt-safe relaxation should drop the prior store \
         (deopt path overwrites via plain Store); got {stores_after}.\n\
         IR after optimize:\n{}",
        ctx.func.display()
    );
}

/// Negative — off-chain side exit reads slot via trusted load before
/// overwriting. The load observes the prior store's memory value, so
/// `is_deopt_safe_side_exit` must reject and both notrap stores must
/// survive.
#[test]
fn isle_dse_cross_block_rejects_when_side_exit_loads() {
    let isa = build_isa();

    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64)); // addr
    sig.params.push(AbiParam::new(types::I64)); // v_arg
    sig.params.push(AbiParam::new(types::I32)); // cond
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 102), sig);

    let mut fbctx = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut func, &mut fbctx);

    let block0 = bcx.create_block();
    let block1 = bcx.create_block();
    let block2 = bcx.create_block();
    bcx.append_block_params_for_function_params(block0);
    bcx.switch_to_block(block0);
    bcx.seal_block(block0);

    let addr = bcx.block_params(block0)[0];
    let v_arg = bcx.block_params(block0)[1];
    let cond = bcx.block_params(block0)[2];

    let v_stale = bcx.ins().iconst(types::I64, 999);
    bcx.ins().store(MemFlags::trusted(), v_stale, addr, 0); // PRIOR
    bcx.ins().brif(cond, block1, &[], block2, &[]);

    bcx.switch_to_block(block1);
    bcx.seal_block(block1);
    let v_six = bcx.ins().iconst(types::I64, 6);
    let v_mul = bcx.ins().imul(v_arg, v_six);
    bcx.ins().store(MemFlags::trusted(), v_mul, addr, 0); // CURRENT
    bcx.ins().return_(&[v_mul]);

    // Side exit reads slot via trusted load — observes prior store.
    bcx.switch_to_block(block2);
    bcx.seal_block(block2);
    let v_observed = bcx.ins().load(types::I64, MemFlags::trusted(), addr, 0);
    let v_plus = bcx.ins().iadd_imm(v_observed, 1);
    bcx.ins().store(MemFlags::new(), v_plus, addr, 0);
    bcx.ins().return_(&[v_observed]);

    bcx.finalize();

    let mut ctx = Context::for_function(func);
    let mut ctrl_plane = ControlPlane::default();
    ctx.optimize(isa.as_ref(), &mut ctrl_plane)
        .expect("optimize");

    // Count notrap (trusted) stores — both the prior in block0 AND the
    // current in block1 must survive. The plain Store in block2 is a
    // separate slot's overwrite that we leave alone.
    let trusted_stores: usize = ctx
        .func
        .layout
        .blocks()
        .flat_map(|b| ctx.func.layout.block_insts(b))
        .filter(|inst| matches!(ctx.func.dfg.insts[*inst].opcode(), Opcode::Store))
        .filter(|inst| {
            ctx.func.dfg.insts[*inst]
                .memflags()
                .map(|f| f.notrap())
                .unwrap_or(false)
        })
        .count();
    assert_eq!(
        trusted_stores,
        2,
        "side exit's trusted load observes prior store; both trusted stores \
         must survive. got {trusted_stores}.\nIR after optimize:\n{}",
        ctx.func.display()
    );
}
