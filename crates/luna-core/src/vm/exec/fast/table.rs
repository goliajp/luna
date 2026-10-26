//! The opcodes of the fast loop, each with the name of its arm's function
//! and the arm's macro, then the opcodes run on the frame-op path and the
//! ones the loop head runs. `fast_arms!(cb, args)` hands the lists to the
//! macro `cb`, which builds the dispatch (`run_fast`) or the arms'
//! functions (`arm_fns`) from them.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_arms {
    ($cb:ident, $($args:tt)*) => {
        $cb! {
            ($($args)*)
            [
                Move arm_move op_move!();
                LoadI arm_load_i op_load_i!();
                LoadF arm_load_f op_load_f!();
                LoadK arm_load_k op_load_k!();
                LoadFalse arm_load_false op_load_false!();
                LFalseSkip arm_l_false_skip op_l_false_skip!();
                LTrueSkip arm_l_true_skip op_l_true_skip!();
                LoadTrue arm_load_true op_load_true!();
                LoadNil arm_load_nil op_load_nil!();
                GetUpval arm_get_upval op_get_upval!();
                SetUpval arm_set_upval op_set_upval!();
                GetTabUp arm_get_tab_up op_get_tab_up!();
                GetTable arm_get_table op_get_table!();
                GetField arm_get_field op_get_field!();
                GetI arm_get_i op_get_i!();
                SetTabUp arm_set_tab_up op_set_tab_up!();
                SetTable arm_set_table op_set_table!();
                SetField arm_set_field op_set_field!();
                SetI arm_set_i op_set_i!();
                GetTableK arm_get_table_k op_get_table_k!();
                SetTableK arm_set_table_k op_set_table_k!();
                GetTabUpR arm_get_tab_up_r op_get_tab_up_r!();
                GetGlobal arm_get_global op_get_global!();
                SetGlobal arm_set_global op_set_global!();
                SetTabUpR arm_set_tab_up_r op_set_tab_up_r!();
                SetTabUpK arm_set_tab_up_k op_set_tab_up_k!();
                SelfOp arm_self_op op_self_op!();
                Add arm_add op_add!();
                Sub arm_sub op_sub!();
                Mul arm_mul op_mul!();
                Mod arm_mod op_mod!();
                IDiv arm_i_div op_i_div!();
                Div arm_div op_div!();
                BAnd arm_b_and op_b_and!();
                BOr arm_b_or op_b_or!();
                BXor arm_b_xor op_b_xor!();
                Shl arm_shl op_shl!();
                Shr arm_shr op_shr!();
                AddI arm_add_i op_add_i!();
                SubI arm_sub_i op_sub_i!();
                AddK arm_add_k op_add_k!();
                SubK arm_sub_k op_sub_k!();
                MulK arm_mul_k op_mul_k!();
                ModK arm_mod_k op_mod_k!();
                IDivK arm_i_div_k op_i_div_k!();
                DivK arm_div_k op_div_k!();
                PowK arm_pow_k op_pow_k!();
                BAndK arm_b_and_k op_b_and_k!();
                BOrK arm_b_or_k op_b_or_k!();
                BXorK arm_b_xor_k op_b_xor_k!();
                ShrI arm_shr_i op_shr_i!();
                ShlI arm_shl_i op_shl_i!();
                Unm arm_unm op_unm!();
                BNot arm_b_not op_b_not!();
                Not arm_not op_not!();
                Len arm_len op_len!();
                Jmp arm_jmp op_jmp!();
                Eq arm_eq op_eq!();
                EqK arm_eq_k op_eq_k!();
                Lt arm_lt order_arm!(<, false);
                Le arm_le order_arm!(<=, true);
                EqI arm_eq_i op_eq_i!();
                LtI arm_lt_i order_imm_arm!(<, false, false);
                LeI arm_le_i order_imm_arm!(<=, false, true);
                GtI arm_gt_i order_imm_arm!(>, true, false);
                GeI arm_ge_i order_imm_arm!(>=, true, true);
                LtK arm_lt_k order_k_arm!(<, false);
                LeK arm_le_k order_k_arm!(<=, true);
                Test arm_test op_test!();
                TestSet arm_test_set op_test_set!();
                ForPrep arm_for_prep op_for_prep!();
                ForPrep55 arm_for_prep55 op_for_prep55!();
                ForLoop arm_for_loop op_for_loop!(false);
                ForLoop55 arm_for_loop55 op_for_loop55!(false);
                ForLoopF arm_for_loop_f op_for_loop!(true);
                ForLoop55F arm_for_loop55_f op_for_loop55!(true);
                TForLoop arm_t_for_loop op_t_for_loop!(4, true);
                TForLoop53 arm_t_for_loop53 op_t_for_loop!(3, true);
                TForLoop55 arm_t_for_loop55 op_t_for_loop!(3, false);
                VargIdx arm_varg_idx op_varg_idx!();
                ErrNNil arm_err_n_nil op_err_n_nil!();
                Call arm_call op_call!();
                Return0 arm_return0 op_return0!();
                Return1 arm_return1 op_return1!();
            ]
            // they stay in this frame: run out of line, then go on
            [
                LoadKx NewTable SetList Pow Concat TForPrep TForPrep53 TForPrep55 Closure Vararg GetVarg ShlK ShrK AddKK SubKK MulKK ModKK PowKK DivKK IDivKK BAndKK BOrKK BXorKK ShlKK ShrKK EqKK LtKK LeKK
            ]
            // listed rather than `_`, so that the jump table covers every
            // opcode without a range check
            [
                Close JmpClose JmpCloseBack Tbc TailCall Return TForCall TForCall53 TForCall55 ExtraArg
            ]
        }
    };
}
pub(super) use fast_arms;
