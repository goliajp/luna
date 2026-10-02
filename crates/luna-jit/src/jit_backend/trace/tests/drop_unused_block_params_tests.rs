use super::*;
use cranelift_codegen::ir::{Function, Signature, UserFuncName};
use cranelift_codegen::isa::{CallConv, TargetFrontendConfig};

/// `loop(dead, live)`: `dead` is only passed back unchanged, as the
/// registers a trace loop only writes were; `live` is read. The first
/// parameter and its arguments go; the second stays.
#[test]
fn a_parameter_only_passed_back_to_itself_is_removed() {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut fctx = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut func, &mut fctx);
    let entry = b.create_block();
    let head = b.create_block();
    let out = b.create_block();
    b.append_block_params_for_function_params(entry);
    let dead = b.append_block_param(head, types::I64);
    let live = b.append_block_param(head, types::I64);
    b.switch_to_block(entry);
    let x = b.block_params(entry)[0];
    b.ins().jump(head, &[x.into(), x.into()]);
    b.switch_to_block(head);
    let one = b.ins().iconst(types::I64, 1);
    let again = b.ins().icmp_imm_s(IntCC::SignedLessThan, live, 10);
    let next_live = b.ins().iadd(live, one);
    b.ins()
        .brif(again, head, &[dead.into(), next_live.into()], out, &[]);
    b.switch_to_block(out);
    b.ins().return_(&[live]);
    b.seal_all_blocks();
    b.finalize(TargetFrontendConfig {
        default_call_conv: CallConv::SystemV,
        pointer_width: target_lexicon::PointerWidth::U64,
        page_size_align_log2: 12,
    });

    drop_unused_block_params(&mut func);

    assert_eq!(func.dfg.block_params(head), &[live]);
    let branches: Vec<usize> = func
        .layout
        .blocks()
        .flat_map(|blk| func.layout.block_insts(blk).collect::<Vec<_>>())
        .flat_map(|inst| {
            func.dfg.insts[inst]
                .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables)
                .iter()
                .filter(|d| d.block(&func.dfg.value_lists) == head)
                .map(|d| d.len(&func.dfg.value_lists))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(branches, vec![1, 1]);
    cranelift_codegen::verify_function(&func, &settings::Flags::new(settings::builder()))
        .expect("the function still verifies");
}
