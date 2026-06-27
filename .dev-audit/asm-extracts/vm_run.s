.section __TEXT,__text,regular,pure_instructions
	.p2align	2
luna_core::vm::exec::Vm::run:
Lfunc_begin568:
	.cfi_startproc
	.cfi_personality 155, _rust_eh_personality
	.cfi_lsda 16, Lexception153
	stp x28, x27, [sp, #-96]!
	.cfi_def_cfa_offset 96
	stp x26, x25, [sp, #16]
	stp x24, x23, [sp, #32]
	stp x22, x21, [sp, #48]
	stp x20, x19, [sp, #64]
	stp x29, x30, [sp, #80]
	add x29, sp, #80
	.cfi_def_cfa w29, 16
	.cfi_offset w30, -8
	.cfi_offset w29, -16
	.cfi_offset w19, -24
	.cfi_offset w20, -32
	.cfi_offset w21, -40
	.cfi_offset w22, -48
	.cfi_offset w23, -56
	.cfi_offset w24, -64
	.cfi_offset w25, -72
	.cfi_offset w26, -80
	.cfi_offset w27, -88
	.cfi_offset w28, -96
	.cfi_remember_state
	sub sp, sp, #2592
	str xzr, [sp]
	mov x26, x1
	add x9, sp, #560
	add x10, sp, #1984
	add x11, x1, #872
	add x8, x1, #824
	stp x8, x11, [sp, #184]
	add x8, x1, #304
	str x8, [sp, #232]
	add x8, sp, #2296
	orr x11, x8, #0x1
	add x8, x1, #1544
	stp x8, x2, [sp, #88]
	orr x8, x10, #0x1
	stp x8, x0, [sp, #72]
	add x8, sp, #1664
	orr x8, x8, #0x1
	stp x8, x11, [sp, #48]
	add x8, sp, #1600
	orr x10, x8, #0x1
	add x8, sp, #1424
	orr x8, x8, #0x1
	stp x8, x10, [sp, #32]
	add x8, sp, #288
	orr x10, x8, #0x1
	orr x8, x9, #0x1
	stp x8, x10, [sp, #104]
	sub x8, x29, #112
	orr x8, x8, #0x1
	str x8, [sp, #64]
Lloh3857:
	adrp x8, l_anon.89dbc2968085ea1691689a13183de4a7.963@PAGE
Lloh3858:
	add x8, x8, l_anon.89dbc2968085ea1691689a13183de4a7.963@PAGEOFF
	str x8, [sp, #24]
	str x1, [sp, #216]
	b LBB568_2
LBB568_1:
	add x0, sp, #840
	bl alloc::rc::Rc<T,A>::drop_slow
LBB568_2:
	ldr x9, [x26, #280]
	ldr x8, [x26]
	orr x10, x9, x8
	cbz x10, LBB568_8
	tbz w9, #0, LBB568_5
	ldr x9, [x26, #288]
	sub x9, x9, #1
	str x9, [x26, #288]
	cmp x9, #1
	b.lt LBB568_1254
LBB568_5:
	tbz w8, #0, LBB568_8
	ldr x19, [x26, #8]
	ldr x8, [x26, #240]
	cmp x8, x19
	b.ls LBB568_8
	ldr w8, [x26, #1792]
	str w8, [x26, #1816]
	mov x0, x26
	bl luna_core::vm::exec::Vm::collect_garbage
	ldr x8, [x26, #240]
	cmp x8, x19
	b.hi LBB568_1259
LBB568_8:
	ldp x8, x11, [x26, #328]
	add x9, x8, x11, lsl #6
	ldur w10, [x9, #-64]
	tbnz w10, #0, LBB568_185
	ldur x23, [x9, #-40]
	ldp w25, w24, [x9, #-24]
	ldp w10, w13, [x9, #-32]
	str x10, [sp, #224]
	str w13, [sp, #284]
	ldur w10, [x9, #-12]
	stp x10, x13, [sp, #200]
	ldr x12, [x23, #16]
	ldr x10, [x12, #16]
	ldr w28, [x10, x13, lsl #2]
	ldrb w10, [x26, #989]
	tbz w10, #0, LBB568_44
	ldr x10, [x26, #960]
	cbz x10, LBB568_44
	ldr x14, [x26, #968]
	cmp x11, x14
	b.ls LBB568_16
	mvn x13, x14
	add x21, x11, x13
	cmp x21, #16
	cset w13, hi
	cbz x21, LBB568_125
LBB568_13:
	ldrb w15, [x26, #990]
	cmp w15, #1
	b.ne LBB568_15
	ldr x15, [x10, #88]
	cmp x12, x15
	b.eq LBB568_190
LBB568_15:
	tbz w13, #0, LBB568_101
LBB568_16:
	ldr x8, [x26, #960]
	ldr x9, [x8, #88]
	ldr x10, [x9, #24]
	ldr x11, [x8, #56]
	cmp x10, x11, lsl #1
	cset w11, hi
	ldr w10, [x9, #176]
	ldrb w12, [x8, #143]
	and w11, w11, w12
	cmp w10, #4
	b.ls LBB568_20
	tbz w11, #0, LBB568_31
	ldrb w10, [x9, #180]
	tbnz w10, #0, LBB568_31
	mov w8, #1
	strb w8, [x9, #180]
	ldr x8, [x26, #960]
	b LBB568_31
LBB568_20:
	tbz w11, #0, LBB568_31
	add w8, w10, #1
	str w8, [x9, #176]
	ldr x8, [x26, #656]
	add x8, x8, #1
	str x8, [x26, #656]
	ldr x8, [x26, #960]
	ldrb w19, [x8, #143]
	ldr x20, [x8, #56]
	ldr x21, [x26, #600]
	ldr x8, [x26, #584]
	cmp x21, x8
	b.eq LBB568_860
LBB568_22:
	ldr x8, [x26, #592]
	add x8, x8, x21, lsl #4
	strb w19, [x8]
	str x20, [x8, #8]
	add x8, x21, #1
	str x8, [x26, #600]
	add x0, x26, #536
Lloh3859:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.962@PAGE
Lloh3860:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.962@PAGEOFF
	mov w2, #24
	bl luna_core::vm::jit_state::JitCounters::bump_close_cause
	ldr x22, [x26, #960]
	cbz x22, LBB568_30
	ldr x1, [x22, #16]
	cbz x1, LBB568_25
	ldr x0, [x22, #24]
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_25:
	ldr x8, [x22, #40]
	cbz x8, LBB568_27
	ldr x0, [x22, #48]
	lsl x1, x8, #5
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_27:
	ldr x8, [x22, #64]
	cbz x8, LBB568_29
	ldr x0, [x22, #72]
	lsl x1, x8, #4
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_29:
	mov x0, x22
	mov w1, #152
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_30:
	str xzr, [x26, #960]
	b LBB568_44
LBB568_31:
	str x24, [sp, #176]
	mov w9, #1
	strb w9, [x8, #142]
	ldr x24, [x26, #960]
	ldr w19, [x24, #136]
	str xzr, [x26, #960]
	ldr x8, [x26, #656]
	add x8, x8, #1
	str x8, [x26, #656]
	ldrb w20, [x24, #143]
	ldr x21, [x24, #56]
	ldr x22, [x26, #600]
	ldr x8, [x26, #584]
	cmp x22, x8
	b.ne LBB568_33
	add x0, x26, #584
	bl alloc::raw_vec::RawVec<T,A>::grow_one
LBB568_33:
	ldr x8, [x26, #592]
	add x8, x8, x22, lsl #4
	strb w20, [x8]
	str x21, [x8, #8]
	add x8, x22, #1
	str x8, [x26, #600]
	ldr x26, [x24, #88]
	ldr x8, [x26, #184]
	mov x9, #9223372036854775807
	cmp x8, x9
	b.hs LBB568_1323
	add x9, x8, #1
	mov x21, x26
	ldr x10, [x21, #208]!
	stur x9, [x21, #-24]
	sub x20, x21, #16
	ldur x9, [x21, #-8]
	lsl x10, x10, #3
LBB568_35:
	cbz x10, LBB568_95
	ldr x11, [x9], #8
	ldr w11, [x11, #232]
	sub x10, x10, #8
	cmp w11, w19
	b.ne LBB568_35
	str x8, [x26, #184]
	ldr x26, [sp, #216]
	ldr x1, [x24, #16]
	cbz x1, LBB568_39
LBB568_38:
	ldr x0, [x24, #24]
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_39:
	ldr x8, [x24, #40]
	cbz x8, LBB568_41
	ldr x0, [x24, #48]
	lsl x1, x8, #5
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_41:
	ldr x8, [x24, #64]
	cbz x8, LBB568_43
	ldr x0, [x24, #72]
	lsl x1, x8, #4
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_43:
	mov x0, x24
	mov w1, #152
	mov w2, #8
	bl __rustc::__rust_dealloc
	ldr x24, [sp, #176]
LBB568_44:
	ldrb w13, [x26, #991]
	strb wzr, [x26, #991]
	ldrb w8, [x26, #989]
	tbz w8, #0, LBB568_85
	ldr x8, [x23, #16]
	ldr x9, [x8, #184]
	mov x10, #9223372036854775807
	cmp x9, x10
	b.hs LBB568_1275
	add x10, x9, #1
	str x10, [x8, #184]
	ldr x11, [x8, #208]
	cbz x11, LBB568_84
	ldr x10, [x8, #200]
	lsl x11, x11, #3
	ldr w12, [sp, #284]
	tbz w13, #0, LBB568_53
	b LBB568_49
LBB568_48:
	subs x11, x11, #8
	b.eq LBB568_84
LBB568_49:
	ldr x27, [x10], #8
	ldr w13, [x27, #232]
	cmp w13, w12
	b.ne LBB568_48
	ldr w13, [x27, #264]
	cbnz w13, LBB568_48
	ldrb w13, [x27, #279]
	tbz w13, #0, LBB568_48
	b LBB568_56
LBB568_52:
	subs x11, x11, #8
	b.eq LBB568_84
LBB568_53:
	ldr x27, [x10], #8
	ldr w13, [x27, #232]
	cmp w13, w12
	b.ne LBB568_52
	ldr w13, [x27, #264]
	cbnz w13, LBB568_56
	ldrb w13, [x27, #279]
	tbz w13, #0, LBB568_52
LBB568_56:
	ldr x9, [x27]
	adds x9, x9, #1
	str x9, [x27]
	b.hs LBB568_1442
	str x24, [sp, #176]
	ldr x9, [x8, #184]
	sub x9, x9, #1
	str x9, [x8, #184]
	str x27, [sp, #840]
	ldr x20, [x27, #216]
	ldr w8, [x27, #232]
	str w8, [sp, #848]
	ldr w21, [x27, #240]
	str w21, [sp, #852]
	ldrb w8, [x27, #278]
	str w8, [sp, #124]
	ldr x8, [x23, #16]
	ldrb w2, [x8, #84]
	str x2, [sp, #856]
	ldr x8, [sp, #192]
	ldr q0, [x8]
	str q0, [sp, #288]
	str xzr, [x26, #872]
	mov w8, #1
	str x8, [x26, #880]
	str xzr, [x26, #888]
	str xzr, [sp, #304]
	ldr x8, [sp, #288]
	cmp x8, x2
	b.lo LBB568_1194
LBB568_58:
	ldr w19, [x27, #264]
	ldr x8, [sp, #184]
	ldr q0, [x8]
	str q0, [sp, #864]
	str xzr, [x26, #824]
	mov w8, #8
	str x8, [x26, #832]
	str xzr, [x26, #840]
	str xzr, [sp, #880]
	adds x24, x19, x21
	stp x28, x21, [sp, #128]
	stp x19, x23, [sp, #160]
	stp x25, x20, [sp, #144]
	b.eq LBB568_63
	ldr x8, [sp, #864]
	cmp x24, x8
	b.hi LBB568_1198
	mov x19, #0
	ldr x20, [sp, #872]
	add x0, x20, x19, lsl #3
	cmp x24, #1
	b.eq LBB568_62
LBB568_61:
	lsl x8, x24, #3
	sub x1, x8, #8
	bl _bzero
	add x8, x19, x24
	add x9, x20, x8, lsl #3
	sub x0, x9, #8
	sub x19, x8, #1
LBB568_62:
	str xzr, [x0]
	add x24, x19, #1
LBB568_63:
	str x24, [sp, #880]
	ldr x28, [sp, #856]
	cbz x28, LBB568_91
	mov x23, #0
	ldr x19, [sp, #872]
	ldr x8, [sp, #224]
	lsl x26, x8, #4
LBB568_65:
	ldp x8, x9, [sp, #216]
	ldr x8, [x8, #312]
	add x9, x9, x23
	cmp x9, x8
	b.hs LBB568_1342
	add x21, x23, #1
	ldr x8, [sp, #232]
	ldr x8, [x8]
	add x8, x8, x26
	ldrb w20, [x8]
	ldr x8, [x8, #8]
	mov x25, x20
Lloh3861:
	adrp x11, LJTI568_0@PAGE
Lloh3862:
	add x11, x11, LJTI568_0@PAGEOFF
	adr x9, LBB568_67
	ldrb w10, [x11, x20]
	add x9, x9, x10, lsl #2
	br x9
LBB568_67:
	mov x20, #0
	tst w8, #0x1
	mov w8, #1
	cinc w25, w8, ne
	ldr x22, [sp, #304]
	ldr x8, [sp, #288]
	cmp x22, x8
	b.ne LBB568_70
LBB568_69:
	add x0, sp, #288
	bl <alloc::raw_vec::RawVec<u8>>::grow_one
LBB568_70:
	ldr x8, [sp, #296]
	strb w25, [x8, x22]
	add x8, x22, #1
	str x8, [sp, #304]
	ldr x8, [x27, #56]
	cmp x23, x8
	b.hs LBB568_72
	ldr x8, [x27, #48]
	add x8, x8, x23
	ldrb w8, [x8, #16]
	cmp w25, w8
	b.ne LBB568_684
LBB568_72:
	sub w8, w25, #3
	cmp w8, #6
	ccmp w25, #0, #4, hs
	b.ne LBB568_684
	cmp x24, x23
	b.eq LBB568_1343
	str x20, [x19, x23, lsl #3]
	add x26, x26, #16
	mov x23, x21
	cmp x28, x21
	b.ne LBB568_65
	b LBB568_91
	mov w25, #5
	mov x20, x8
	ldr x22, [sp, #304]
	ldr x8, [sp, #288]
	cmp x22, x8
	b.eq LBB568_69
	b LBB568_70
	mov w25, #10
	mov x20, x8
	ldr x22, [sp, #304]
	ldr x8, [sp, #288]
	cmp x22, x8
	b.eq LBB568_69
	b LBB568_70
	mov w25, #3
	mov x20, x8
	ldr x22, [sp, #304]
	ldr x8, [sp, #288]
	cmp x22, x8
	b.eq LBB568_69
	b LBB568_70
	mov w25, #4
	mov x20, x8
	ldr x22, [sp, #304]
	ldr x8, [sp, #288]
	cmp x22, x8
	b.eq LBB568_69
	b LBB568_70
	mov w25, #8
	mov x20, x8
	ldr x22, [sp, #304]
	ldr x8, [sp, #288]
	cmp x22, x8
	b.eq LBB568_69
	b LBB568_70
	mov w25, #6
	mov x20, x8
	ldr x22, [sp, #304]
	ldr x8, [sp, #288]
	cmp x22, x8
	b.eq LBB568_69
	b LBB568_70
	mov w25, #7
	mov x20, x8
	ldr x22, [sp, #304]
	ldr x8, [sp, #288]
	cmp x22, x8
	b.eq LBB568_69
	b LBB568_70
	mov w25, #11
	mov x20, x8
	ldr x22, [sp, #304]
	ldr x8, [sp, #288]
	cmp x22, x8
	b.eq LBB568_69
	b LBB568_70
	mov w25, #9
	mov x20, x8
	ldr x22, [sp, #304]
	ldr x8, [sp, #288]
	cmp x22, x8
	b.ne LBB568_70
	b LBB568_69
LBB568_84:
	str x9, [x8, #184]
LBB568_85:
	ldp x9, x8, [x26, #328]
	add x8, x9, x8, lsl #6
	ldur w9, [x8, #-64]
	add x20, sp, #1984
	tbnz w9, #0, LBB568_1276
	ldr w9, [sp, #284]
	add w9, w9, #1
	stur w9, [x8, #-28]
	ldrb w8, [x26, #1836]
	tbnz w8, #0, LBB568_181
	ldrb w8, [x26, #1336]
	cmp w8, #11
	b.ne LBB568_89
	ldr x8, [x26, #1352]
	cbz x8, LBB568_181
LBB568_89:
	ldr x19, [x23, #16]
	ldr x8, [x19, #96]
	cbz x8, LBB568_116
	sub x8, x8, #1
	ldr x9, [sp, #208]
	cmp x8, x9
	csel x8, x8, x9, lo
	ldr x9, [x19, #88]
	ldr w5, [x9, x8, lsl #2]
	mov w4, #1
	ldrb w8, [x26, #1379]
	tbnz w8, #0, LBB568_117
	b LBB568_119
LBB568_91:
	ldr x26, [sp, #216]
	mov w8, #11
	strb w8, [x26, #896]
	ldr x8, [x26, #336]
	str x8, [sp, #896]
	ldp x19, x20, [sp, #160]
	cbz w19, LBB568_150
	cmp x8, #1
	b.ls LBB568_115
	ldr x9, [x26, #328]
	add x8, x9, x8, lsl #6
	ldur w9, [x8, #-128]
	ldr x0, [sp, #136]
	tbz w9, #0, LBB568_148
	mov x8, #0
	cmp x24, x0
	b.hi LBB568_149
	b LBB568_1371
LBB568_95:
	str x8, [x26, #184]
	ldr x8, [x24, #96]
	ldr x10, [sp, #216]
	ldrb w9, [x10, #1840]
	cmp w9, #3
	mov w9, #256
	csel w9, w9, wzr, lo
	cmp x8, #0
	ldr x1, [x10, #944]
	ldr x2, [x10, #952]
	ldr x0, [x10, #928]
	ldr x8, [x10, #936]
	ldr x10, [x8, #24]
	cinc w4, w9, eq
	add x8, sp, #288
	mov x3, x24
	blr x10
	ldr w8, [sp, #536]
	cmp w8, #2
	b.ne LBB568_128
	ldr x20, [sp, #216]
	ldr x8, [x20, #688]
	add x8, x8, #1
	str x8, [x20, #688]
	ldr x0, [x20, #928]
	ldr x8, [x20, #936]
	ldr x8, [x8, #32]
	blr x8
	mov x22, x0
	mov x26, x1
	ldr x19, [x20, #576]
	ldr x8, [x20, #560]
	cmp x19, x8
	b.ne LBB568_100
	add x0, x20, #560
	bl alloc::raw_vec::RawVec<T,A>::grow_one
LBB568_100:
	ldr x8, [x20, #568]
	add x8, x8, x19, lsl #4
	stp x22, x26, [x8]
	add x8, x19, #1
	str x8, [x20, #576]
	mov x26, x20
	ldr x1, [x24, #16]
	cbnz x1, LBB568_38
	b LBB568_39
LBB568_101:
	ldrb w8, [x26, #992]
	cmp w8, w21, uxtb
	b.hs LBB568_103
	strb w21, [x26, #992]
LBB568_103:
	ldr x8, [x10, #56]
	cbz x8, LBB568_110
	ldr x10, [x10, #48]
	add x8, x10, x8, lsl #5
	ldur w10, [x8, #-12]
	lsr w11, w10, #24
	cbnz w11, LBB568_110
	and w11, w10, #0x7f
	cmp w11, #47
	b.ne LBB568_110
	ldur w11, [x8, #-32]
	cbnz w11, LBB568_110
	ldur w11, [x9, #-64]
	tbnz w11, #0, LBB568_110
	ldur w9, [x9, #-32]
	lsr w10, w10, #7
	add w9, w9, w10, uxtb
	ldr w10, [x26, #1792]
	subs w9, w10, w9
	b.lo LBB568_110
	mov w10, #1
	stp w10, w9, [x8, #-32]
LBB568_110:
	mov w12, #0
	and w8, w28, #0x7f
	tst w28, #0xff0000
	b.ne LBB568_222
	cmp w8, #57
	b.ne LBB568_477
	ldr x9, [x26, #336]
	cbz x9, LBB568_114
	ldr x10, [x26, #328]
	add x9, x10, x9, lsl #6
	ldur w10, [x9, #-64]
	tbz w10, #0, LBB568_476
LBB568_114:
	mov w12, #0
	b LBB568_477
LBB568_115:
	mov x8, #0
	ldr x0, [sp, #136]
	cmp x24, x0
	b.hi LBB568_149
	b LBB568_1371
LBB568_116:
	mov x4, #0
	ldrb w8, [x26, #1379]
	tbz w8, #0, LBB568_119
LBB568_117:
	ldr x8, [x26, #1368]
	sub x8, x8, #1
	str x8, [x26, #1368]
	cmp x8, #0
	b.gt LBB568_119
	ldr x8, [x26, #1360]
	str x8, [x26, #1368]
	add x0, sp, #1984
	mov x1, x26
Lloh3863:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.978@PAGE
Lloh3864:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.978@PAGEOFF
	mov w3, #5
	mov w6, #0
	bl luna_core::vm::exec::Vm::run_hook
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.ne LBB568_1295
LBB568_119:
	ldrb w8, [x26, #1378]
	tbz w8, #0, LBB568_181
	ldr x8, [x19, #96]
	cbz x8, LBB568_176
	sub x8, x8, #1
	ldp x11, x9, [sp, #200]
	cmp x8, x9
	csel x10, x8, x9, lo
	ldr x9, [x19, #88]
	ldr w5, [x9, x10, lsl #2]
	cmn w11, #1
	b.eq LBB568_124
	ldr w10, [sp, #284]
	cmp w10, w11
	b.lo LBB568_124
	cmp x8, x11
	csel x8, x8, x11, lo
	ldr w8, [x9, x8, lsl #2]
	cmp w5, w8
	b.eq LBB568_178
LBB568_124:
	add x0, sp, #1984
	mov x1, x26
Lloh3865:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.979@PAGE
Lloh3866:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.979@PAGEOFF
	mov w3, #4
	mov w4, #1
	mov w6, #0
	bl luna_core::vm::exec::Vm::run_hook
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_178
	b LBB568_1295
LBB568_125:
	ldr x15, [x10, #56]
	cbz x15, LBB568_13
	ldr x15, [x10, #88]
	cmp x12, x15
	b.ne LBB568_13
	cmp x21, #16
	ldr w13, [x10, #136]
	ldr x15, [sp, #208]
	ccmp w15, w13, #4, ls
	cset w13, eq
	b LBB568_13
LBB568_128:
	add x0, sp, #560
	add x1, sp, #288
	mov w2, #272
	bl _memcpy
	ldr x11, [sp, #216]
	ldr x8, [x11, #736]
	ldr w9, [sp, #788]
	ldr w10, [sp, #792]
	add x8, x8, x9
	str x8, [x11, #736]
	ldr x8, [x11, #744]
	add x8, x8, x10
	str x8, [x11, #744]
	ldr x8, [x11, #752]
	ldr w9, [sp, #796]
	ldr w10, [sp, #800]
	add x8, x8, x9
	str x8, [x11, #752]
	ldr x8, [x11, #760]
	add x8, x8, x10
	str x8, [x11, #760]
	ldr w8, [sp, #804]
	ldr x9, [x11, #784]
	add x8, x9, x8
	str x8, [x11, #784]
	ldrb w8, [sp, #824]
	tbz w8, #0, LBB568_130
	ldr x9, [sp, #216]
	ldr x8, [x9, #672]
	add x8, x8, #1
	str x8, [x9, #672]
LBB568_130:
	ldr x8, [sp, #632]
	cbz x8, LBB568_133
	ldr x9, [sp, #216]
	ldr x8, [x9, #768]
	add x8, x8, #1
	str x8, [x9, #768]
	ldrb w8, [sp, #823]
	tbz w8, #0, LBB568_133
	ldr x9, [sp, #216]
	ldr x8, [x9, #776]
	add x8, x8, #1
	str x8, [x9, #776]
LBB568_133:
	ldr x22, [sp, #560]
	cbz x22, LBB568_137
	ldr x27, [sp, #568]
	ldr x8, [sp, #216]
	ldr x19, [x8, #552]
	ldr x8, [x8, #536]
	cmp x19, x8
	b.ne LBB568_136
	ldr x8, [sp, #216]
	add x0, x8, #536
	bl alloc::raw_vec::RawVec<T,A>::grow_one
LBB568_136:
	ldr x9, [sp, #216]
	ldr x8, [x9, #544]
	add x8, x8, x19, lsl #4
	stp x22, x27, [x8]
	add x8, x19, #1
	str x8, [x9, #552]
	add x0, x9, #536
	mov x1, x22
	mov x2, x27
	bl luna_core::vm::jit_state::JitCounters::bump_close_cause
LBB568_137:
	ldr w8, [sp, #808]
	cbz w8, LBB568_139
	ldr x9, [sp, #216]
	ldr x8, [x9, #792]
	add x8, x8, #1
	str x8, [x9, #792]
LBB568_139:
	ldrb w8, [sp, #821]
	cmp w8, #2
	b.lo LBB568_141
	ldr x9, [sp, #216]
	ldr x8, [x9, #816]
	add x8, x8, #1
	str x8, [x9, #816]
LBB568_141:
	ldr x22, [x24, #96]
	cbz x22, LBB568_1060
	ldr w10, [x24, #104]
	ldr x0, [x24, #112]
	strb wzr, [sp, #823]
	ldr x8, [x22, #184]
	mov x9, #9223372036854775807
	cmp x8, x9
	b.hs LBB568_1337
	ldr x9, [sp, #760]
	add x11, x8, #1
	mov x8, x22
	ldr x12, [x8, #208]!
	stur x11, [x8, #-24]
	ldur x11, [x8, #-8]
	lsl x12, x12, #3
LBB568_144:
	cbz x12, LBB568_1059
	ldr x27, [x11], #8
	ldr w13, [x27, #232]
	sub x12, x12, #8
	cmp w13, w10
	b.ne LBB568_144
	ldr x10, [x27, #88]
	ldr x1, [x27, #72]
	subs x11, x0, x10
	b.hs LBB568_690
	ldr x11, [x27, #80]
	mov w12, #48
	madd x11, x0, x12, x11
	add x13, x11, #16
	add x11, x11, #24
	ldr x11, [x11]
	ldr x12, [sp, #600]
	cmp x12, x11
	b.lo LBB568_862
	b LBB568_863
LBB568_148:
	ldur w8, [x8, #-92]
	cmp x24, x0
	b.ls LBB568_1371
LBB568_149:
	ldr x9, [sp, #872]
	str x8, [x9, x0, lsl #3]
LBB568_150:
Lloh3867:
	adrp x8, __MergedGlobals@PAGE
Lloh3868:
	add x8, x8, __MergedGlobals@PAGEOFF
	ldapr x8, [x8]
	cbnz x8, LBB568_1197
LBB568_151:
	adrp x8, __MergedGlobals@PAGE+8
	ldrb w8, [x8, __MergedGlobals@PAGEOFF+8]
	tbz w8, #0, LBB568_154
	ldr x8, [x26, #696]
	cbnz x8, LBB568_154
	add x8, sp, #848
	str x8, [sp, #1984]
Lloh3869:
	adrp x8, <u32 as core::fmt::Display>::fmt@GOTPAGE
Lloh3870:
	ldr x8, [x8, <u32 as core::fmt::Display>::fmt@GOTPAGEOFF]
	str x8, [sp, #1992]
	add x1, sp, #1984
Lloh3871:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.949@PAGE
Lloh3872:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.949@PAGEOFF
	bl std::io::stdio::_eprint
LBB568_154:
	ldr x0, [x26, #912]
	ldr x8, [x26, #920]
	ldr x9, [x8, #32]
	add x8, sp, #1984
	mov x1, x26
	mov x2, x20
	blr x9
	ldr x22, [sp, #872]
	mov x0, x22
	ldr x8, [sp, #152]
	blr x8
	mov x24, x0
	ldr x8, [sp, #1984]
	cbz x8, LBB568_157
	ldr x1, [sp, #2000]
	ldr x0, [sp, #1992]
	blr x8
LBB568_157:
	ldr x8, [x26, #696]
	add x8, x8, #1
	str x8, [x26, #696]
	ldrb w8, [x26, #896]
	cmp w8, #11
	b.ne LBB568_680
	cbz w19, LBB568_161
	tbnz x24, #63, LBB568_164
	str x24, [sp, #904]
	strb wzr, [sp, #919]
	b LBB568_162
LBB568_161:
	str x24, [sp, #904]
	lsr x8, x24, #63
	strb w8, [sp, #919]
	tbnz x24, #63, LBB568_189
LBB568_162:
	ldr x1, [x27, #88]
	lsr x8, x24, #32
	cbz x8, LBB568_173
	sub w0, w8, #1
	cmp x0, x1
	b.lo LBB568_198
	b LBB568_1378
LBB568_164:
	ubfx x19, x24, #56, #7
	cmp w19, #16
	b.ne LBB568_219
	ldr w8, [x26, #984]
	cbz w8, LBB568_506
	sub w8, w8, #1
	str w8, [x26, #984]
	ldr x8, [x26, #800]
	add x8, x8, #1
	str x8, [x26, #800]
	mov w8, #1
	strb w8, [x26, #991]
	ldr x8, [sp, #896]
	ldr x9, [x26, #336]
	cmp x9, x8
	b.ls LBB568_168
LBB568_167:
	str x8, [x26, #336]
LBB568_168:
	ldr x8, [sp, #184]
	ldr x8, [x8]
	cbz x8, LBB568_170
	ldr x0, [x26, #832]
	lsl x1, x8, #3
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_170:
	ldr q0, [sp, #864]
	ldr x9, [sp, #184]
	str q0, [x9]
	ldr x8, [sp, #880]
	str x8, [x9, #16]
	ldr q0, [sp, #288]
	str q0, [sp, #1984]
	ldr x8, [sp, #304]
	str x8, [sp, #2000]
	ldr x8, [sp, #192]
	ldr x1, [x8]
	cbz x1, LBB568_172
	ldr x0, [x26, #880]
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_172:
	ldr q0, [sp, #1984]
	ldr x9, [sp, #192]
	str q0, [x9]
	ldr x8, [sp, #2000]
	str x8, [x9, #16]
	ldr x8, [x27]
	subs x8, x8, #1
	str x8, [x27]
	b.ne LBB568_2
	b LBB568_1
LBB568_173:
	ldp x9, x8, [x27, #64]
	add x9, x9, #16
	sub x0, x1, #1
	add x10, x8, x8, lsl #1
	lsl x10, x10, #3
LBB568_174:
	cbz x10, LBB568_197
	ldr w11, [x9], #24
	add x0, x0, #1
	sub x10, x10, #24
	cmp w11, w24
	b.ne LBB568_174
	b LBB568_198
LBB568_176:
	ldr x8, [sp, #200]
	cmn w8, #1
	b.ne LBB568_181
	add x0, sp, #1984
	mov x1, x26
Lloh3873:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.979@PAGE
Lloh3874:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.979@PAGEOFF
	mov w3, #4
	mov x4, #0
	mov w6, #0
	bl luna_core::vm::exec::Vm::run_hook
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.ne LBB568_1295
LBB568_178:
	ldr x8, [x26, #336]
	cbz x8, LBB568_1325
	ldr x9, [x26, #328]
	add x8, x9, x8, lsl #6
	ldur x9, [x8, #-64]
	cmp x9, #1
	b.eq LBB568_1325
	ldr w9, [sp, #284]
	stur w9, [x8, #-12]
LBB568_181:
	and w8, w28, #0x7f
Lloh3875:
	adrp x11, LJTI568_2@PAGE
Lloh3876:
	add x11, x11, LJTI568_2@PAGEOFF
	adr x9, LBB568_182
	ldrh w10, [x11, x8, lsl #1]
	add x9, x9, x10, lsl #2
	br x9
LBB568_182:
	cmp w8, #50
	b.eq LBB568_473
	cmp w8, #51
	b.ne LBB568_474
	lsr w8, w28, #7
	ldr x19, [sp, #224]
	add w8, w19, w8, uxtb
	mov w9, #1
	b LBB568_800
LBB568_185:
	ldurb w15, [x9, #-56]
	ldurb w12, [x9, #-52]
	ldurb w10, [x9, #-51]
	ldurb w13, [x9, #-50]
	ldp w0, w2, [x9, #-48]
	ldur w14, [x9, #-40]
	ldur w23, [x9, #-8]
	sub w16, w15, #3
	cmp w15, #2
	mov w17, #4
	csel w16, w16, w17, hi
	and w16, w16, #0xff
	cmp w16, #2
	add x20, sp, #1984
	b.eq LBB568_658
	ldur w19, [x9, #-4]
	cmp w16, #3
	b.eq LBB568_633
	cmp w16, #4
	b.ne LBB568_661
	ldur w16, [x9, #-36]
	ldurb w17, [x9, #-49]
	ldur w8, [x9, #-16]
	ldur q0, [x9, #-32]
	str q0, [sp, #2272]
	strb w15, [sp, #2296]
	ldurh w15, [x9, #-55]
	ldr x1, [sp, #56]
	strh w15, [x1]
	ldurb w9, [x9, #-53]
	strb w9, [x1, #2]
	strb w12, [sp, #2300]
	strb w10, [sp, #2301]
	strb w13, [sp, #2302]
	strb w17, [sp, #2303]
	str w0, [sp, #2304]
	str w2, [sp, #2308]
	str w14, [sp, #2312]
	str w16, [sp, #2316]
	sub x9, x11, #1
	str x9, [x26, #336]
	str w23, [x26, #1792]
	add x0, sp, #1984
	add x3, sp, #2272
	add x4, sp, #2296
	mov x1, x26
	mov x2, x8
	ldr x5, [sp, #96]
	bl luna_core::vm::exec::Vm::drive_close
	ldr x8, [sp, #1984]
	ldur q0, [x20, #8]
	str q0, [sp, #256]
	mov x9, #-9223372036854775808
	cmp x8, x9
	b.eq LBB568_2
	b LBB568_1272
LBB568_189:
	ubfx x19, x24, #56, #7
	and x8, x24, #0xffffffffffffff
	b LBB568_224
LBB568_190:
	cbz x21, LBB568_15
	ldr w15, [x10, #136]
	ldr x16, [sp, #208]
	cmp w16, w15
	b.ne LBB568_15
	sub x11, x11, #1
	cmp x14, x11
	b.hs LBB568_15
	mov x11, #0
	add x8, x8, x14, lsl #6
	mov x14, x21
	b LBB568_195
LBB568_194:
	add x8, x8, #64
	subs x14, x14, #1
	b.eq LBB568_676
LBB568_195:
	ldr w15, [x8]
	tbnz w15, #0, LBB568_194
	ldr x15, [x8, #24]
	ldr x15, [x15, #16]
	cmp x15, x12
	cinc x11, x11, eq
	b LBB568_194
LBB568_197:
	add x0, x8, x1
LBB568_198:
	ldr x8, [x20, #16]
	ldr x9, [x8, #184]
	mov x10, #9223372036854775807
	cmp x9, x10
	b.hs LBB568_1321
	mov x23, x20
	add x10, x9, #1
	str x10, [x8, #184]
	ldp x10, x11, [x8, #200]
	ldr w12, [sp, #848]
	lsl x11, x11, #3
	mov x13, x11
	mov x14, x10
LBB568_200:
	cbz x13, LBB568_214
	ldr x15, [x14], #8
	ldr w16, [x15, #232]
	sub x13, x13, #8
	cmp w16, w12
	b.ne LBB568_200
	ldr x12, [x15, #120]
	cmp x0, x12
	b.hs LBB568_214
	ldr x12, [x15, #112]
	add x12, x12, x0, lsl #3
	ldr x13, [x12, #16]
	cbz x13, LBB568_214
LBB568_204:
	cbz x11, LBB568_214
	ldr x12, [x10], #8
	ldr x20, [x12, #216]
	sub x11, x11, #8
	cmp x20, x13
	b.ne LBB568_204
	ldr x9, [x12, #80]
	ldr x10, [x9]
	adds x10, x10, #1
	str x10, [x9]
	b.hs LBB568_1442
	ldp x25, x28, [x12, #80]
	ldr x9, [x12, #64]
	ldr x10, [x9]
	adds x10, x10, #1
	str x10, [x9]
	b.hs LBB568_1442
	ldp x9, x21, [x12, #64]
	str x9, [sp, #208]
	ldr x9, [x12, #32]
	ldr x10, [x9]
	adds x10, x10, #1
	str x10, [x9]
	b.hs LBB568_1442
	ldp x11, x19, [x12, #32]
	ldr x9, [x12, #96]
	ldr x10, [x9]
	adds x10, x10, #1
	str x10, [x9]
	b.hs LBB568_1442
	ldp x10, x26, [x12, #96]
	ldr x9, [x8, #184]
	sub x9, x9, #1
	str x9, [x8, #184]
	stp x25, x28, [x29, #-144]
	ldp x8, x1, [sp, #208]
	stp x8, x21, [x29, #-128]
	str x11, [sp, #200]
	stp x11, x19, [x29, #-112]
	mov x24, x10
	str x10, [sp, #560]
	str x26, [sp, #568]
	ldr x0, [x1, #912]
	ldr x8, [x1, #920]
	ldr x9, [x8, #32]
	add x8, sp, #1984
	mov x2, x23
	blr x9
	mov x0, x22
	blr x20
	mov x22, x0
	ldr x8, [sp, #1984]
	cbz x8, LBB568_213
	ldr x1, [sp, #2000]
	ldr x0, [sp, #1992]
	blr x8
LBB568_213:
	mov x12, x24
	mov x1, x28
	b LBB568_525
LBB568_214:
	str x9, [x8, #184]
	ldr x8, [x27, #80]
	ldr x9, [x8]
	adds x9, x9, #1
	str x9, [x8]
	b.hs LBB568_1442
	ldp x25, x1, [x27, #80]
	ldr x8, [x27, #64]
	ldr x9, [x8]
	adds x9, x9, #1
	str x9, [x8]
	b.hs LBB568_1442
	ldp x8, x21, [x27, #64]
	str x8, [sp, #208]
	ldr x8, [x27, #32]
	ldr x9, [x8]
	adds x9, x9, #1
	str x9, [x8]
	b.hs LBB568_1442
	ldp x8, x19, [x27, #32]
	str x8, [sp, #200]
	ldr x8, [x27, #96]
	ldr x9, [x8]
	adds x9, x9, #1
	str x9, [x8]
	b.hs LBB568_1442
	ldp x12, x26, [x27, #96]
	ldr x22, [sp, #904]
	b LBB568_525
LBB568_219:
	and x8, x24, #0xffffffffffffff
	cmp w19, #96
	b.ne LBB568_223
	ldr w9, [sp, #848]
	cmp x8, x9
	b.ne LBB568_223
	ldr x8, [x26, #808]
	add x8, x8, #1
	str x8, [x26, #808]
	mov w8, #1
	strb w8, [x26, #991]
	ldr x8, [sp, #896]
	ldr x9, [x26, #336]
	cmp x9, x8
	b.ls LBB568_168
	b LBB568_167
LBB568_222:
	b LBB568_477
LBB568_223:
	str x24, [sp, #904]
	mov w9, #1
	strb w9, [sp, #919]
LBB568_224:
	str w19, [sp, #560]
	str x8, [sp, #920]
	ldr x23, [x20, #16]
	ldr x8, [x23, #184]
	mov x9, #9223372036854775806
	cmp x8, x9
	b.hi LBB568_1321
	add x8, x8, #1
	str x8, [x23, #184]
	ldp x8, x10, [x23, #200]
	ldr w9, [sp, #848]
	lsl x10, x10, #3
LBB568_226:
	cbz x10, LBB568_516
	ldr x21, [x8], #8
	ldr w11, [x21, #232]
	sub x10, x10, #8
	cmp w11, w9
	b.ne LBB568_226
	ldr x20, [x21, #160]
	mov x8, #9223372036854775807
	cmp x20, x8
	b.hs LBB568_1338
	add x8, x20, #1
	str x8, [x21, #160]
	ldr x8, [x21, #192]
	cbz x8, LBB568_235
	ldp x0, x1, [x21, #200]
	add x2, sp, #560
	bl core::hash::BuildHasher::hash_one
	mov x8, #0
	lsr x11, x0, #57
	ldp x10, x9, [x21, #168]
	dup.8b v0, w11
	and x11, x0, x9
	ldr d1, [x10, x11]
	cmeq.8b v2, v1, v0
	fmov x12, d2
	ands x12, x12, #0x8080808080808080
	b.eq LBB568_233
LBB568_231:
	rbit x13, x12
	clz x13, x13
	add x13, x11, x13, lsr #3
	and x13, x13, x9
	sub x13, x10, x13, lsl #3
	ldur w14, [x13, #-8]
	cmp w19, w14
	b.eq LBB568_508
	sub x13, x12, #1
	ands x12, x13, x12
	b.ne LBB568_231
LBB568_233:
	movi.2d v2, #0xffffffffffffffff
	cmeq.8b v1, v1, v2
	umaxv.8b b1, v1
	fmov w12, s1
	tbnz w12, #0, LBB568_235
	add x8, x8, #8
	add x0, x11, x8
	and x11, x0, x9
	ldr d1, [x10, x11]
	cmeq.8b v2, v1, v0
	fmov x12, d2
	ands x12, x12, #0x8080808080808080
	b.ne LBB568_231
	b LBB568_233
LBB568_235:
	str x20, [x21, #160]
	b LBB568_516
	lsr w9, w28, #7
	ldp x11, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w11, w9, uxtb
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #1632]
	lsr w10, w28, #16
	add w10, w11, w10, uxtb
	ldr q0, [x8, w10, uxtw #4]
	str q0, [sp, #1648]
	ldrb w11, [sp, #1632]
	cmp w11, #2
	b.ne LBB568_693
	ldrb w11, [sp, #1648]
	cmp w11, #2
	b.ne LBB568_693
	ldr x8, [sp, #1640]
	ldr x9, [sp, #1656]
	cmp x8, x9
	cset w8, le
	and w9, w28, #0x8000
	cmp w8, w9, lsr #15
	b.eq LBB568_2
	b LBB568_391
	lsr w8, w28, #7
	lsr w9, w28, #16
	ldr x10, [sp, #224]
	add w2, w10, w8, uxtb
	add w8, w2, w9, uxtb
	str w8, [x26, #1792]
	add x0, sp, #1984
	mov x1, x26
	bl luna_core::vm::exec::Vm::concat_run
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w2, w9, w8, uxtb
	ubfx w8, w28, #16, #8
	subs w4, w8, #1
	cset w3, hs
	lsr w8, w28, #24
	sub w5, w8, #1
	add x0, sp, #1984
	mov x1, x26
	mov w6, #0
	bl luna_core::vm::exec::Vm::begin_call
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1271
	lsr w9, w28, #16
	ldp x10, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w10, w9, uxtb
	add x9, x8, w9, uxtw #4
	ldrb w10, [x9]
	cmp w10, #1
	b.eq LBB568_802
	cmp w10, #0
	cset w9, eq
	b LBB568_803
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x10, [sp, #224]
	add w8, w10, w8, uxtb
	ldr q0, [x9, w8, uxtw #4]
	str q0, [sp, #1392]
	add x0, sp, #1984
	add x2, sp, #1392
	mov x1, x26
	bl luna_core::vm::exec::Vm::len_step
	ldrb w10, [sp, #1984]
	cmp w10, #14
	b.eq LBB568_1277
	ldr x9, [sp, #72]
	ldr w8, [x9]
	str w8, [sp, #1416]
	ldur w8, [x9, #3]
	add x9, sp, #1416
	stur w8, [x9, #3]
	ldr x9, [sp, #1992]
	ldr x8, [sp, #2000]
	sub w11, w10, #11
	cmp w10, #10
	csinc w11, w11, wzr, hi
	cbz w11, LBB568_790
	and w11, w11, #0xff
	cmp w11, #1
	b.ne LBB568_1348
	ldr x11, [sp, #2008]
	strb w10, [sp, #1424]
	ldr w10, [sp, #1416]
	ldr x12, [sp, #32]
	str w10, [x12]
	add x10, sp, #1416
	ldur w10, [x10, #3]
	stur w10, [x12, #3]
	str x9, [sp, #1432]
	lsr w9, w28, #7
	ldr x10, [sp, #224]
	add w9, w10, w9, uxtb
	str x8, [sp, #1984]
	str x11, [sp, #1992]
	str x8, [sp, #2000]
	str x11, [sp, #2008]
	str w9, [sp, #292]
	strb wzr, [sp, #288]
Lloh3877:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.307@PAGE
Lloh3878:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.307@PAGEOFF
	add x0, sp, #560
	add x2, sp, #1424
	add x3, sp, #1984
	add x5, sp, #288
	b LBB568_1028
	mov x20, x28
	lsr w8, w28, #7
	ldp x9, x10, [sp, #224]
	ldr x26, [x10]
	add w25, w9, w8, uxtb
	add x8, x26, w25, uxtw #4
	ldrb w27, [x8]
	str x8, [sp, #200]
	ldr x28, [x8, #8]
	add w8, w25, #1
	add x8, x26, w8, uxtw #4
	ldrb w23, [x8]
	str x8, [sp, #208]
	ldr x22, [x8, #8]
	add w8, w25, #2
	add x8, x26, w8, uxtw #4
	ldrb w19, [x8]
	str x8, [sp, #224]
	ldr x21, [x8, #8]
	cmp w27, #2
	b.eq LBB568_886
	cmp w27, #3
	b.eq LBB568_877
	cmp w27, #4
	b.ne LBB568_889
	ldr w1, [x28, #32]
	sub x8, x29, #144
	add x0, x28, #40
	mov w2, #1
	mov w3, #1
	bl luna_core::numeric::str2num
	subs w24, w23, #2
	b.ne LBB568_878
	b LBB568_887
	ubfx x19, x28, #7, #8
	ldp x9, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w9, w19
	add w9, w9, #4
	add x8, x8, w9, uxtw #4
	ldrb w21, [x8]
	ldur x9, [x8, #1]
	str x9, [sp, #1768]
	ldr x8, [x8, #8]
	add x9, sp, #1744
	stur x8, [x9, #31]
	cbz w21, LBB568_2
	ldrb w8, [x26, #989]
	tbz w8, #0, LBB568_1236
	ldr x8, [x23, #16]
	ldr w9, [x8, #168]
	mov w10, #2147483646
	cmp w9, w10
	b.hi LBB568_1236
	add w10, w9, #1
	str w10, [x8, #168]
	cmp w9, #64
	b.ne LBB568_1236
	ldr x8, [x26, #960]
	cbnz x8, LBB568_1236
	ldr w9, [sp, #284]
	ldr x8, [x23, #16]
	ldrb w22, [x8, #84]
	str w9, [sp, #208]
	mov x25, x23
	cbz x22, LBB568_1139
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov x0, x22
	mov w1, #1
	bl __rustc::__rust_alloc
	cbz x0, LBB568_1434
	mov x27, #0
	str x22, [sp, #1984]
	str x0, [sp, #1992]
	ldr x23, [sp, #224]
	lsl x26, x23, #4
	str xzr, [sp, #2000]
	b LBB568_260
LBB568_259:
	ldr x8, [sp, #1992]
	strb w20, [x8, x27]
	str x24, [sp, #2000]
	add x23, x23, #1
	add x26, x26, #16
	mov x27, x24
	cmp x22, x24
	b.eq LBB568_1140
LBB568_260:
	ldr x8, [sp, #216]
	ldr x24, [x8, #312]
	cmp x23, x24
	b.hs LBB568_1387
	add x24, x27, #1
	ldr x8, [sp, #232]
	ldr x8, [x8]
	ldrb w20, [x8, x26]
Lloh3879:
	adrp x9, LJTI568_3@PAGE
Lloh3880:
	add x9, x9, LJTI568_3@PAGEOFF
	adr x10, LBB568_262
	ldrb w11, [x9, x20]
	add x10, x10, x11, lsl #2
	br x10
LBB568_262:
	add x8, x8, x26
	ldr w8, [x8, #8]
	tst w8, #0x1
	mov w8, #1
	cinc w20, w8, ne
	b LBB568_272
	mov w20, #5
	b LBB568_272
	mov w20, #10
	b LBB568_272
	mov w20, #3
	b LBB568_272
	mov w20, #4
	b LBB568_272
	mov w20, #8
	b LBB568_272
	mov w20, #6
	b LBB568_272
	mov w20, #7
	b LBB568_272
	mov w20, #11
	b LBB568_272
	mov w20, #9
LBB568_272:
	ldr x8, [sp, #1984]
	cmp x27, x8
	b.ne LBB568_259
	add x0, sp, #1984
	bl <alloc::raw_vec::RawVec<u8>>::grow_one
	b LBB568_259
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x12, [sp, #224]
	add w8, w12, w8, uxtb
	add x8, x9, w8, uxtw #4
	ldrb w19, [x8]
	ldur w10, [x8, #1]
	str w10, [sp, #1744]
	ldr w10, [x8, #4]
	add x11, sp, #1744
	stur w10, [x11, #3]
	ldr x21, [x8, #8]
	add w10, w12, w28, lsr #24
	add x9, x9, w10, uxtw #4
	add x24, sp, #1984
	ldrb w22, [x9]
	ldur w10, [x9, #1]
	stur w10, [x29, #-160]
	ldr w10, [x9, #4]
	add x20, sp, #2320
	stur w10, [x20, #195]
	ldr x23, [x9, #8]
	strb w19, [sp, #288]
	ldur w10, [x8, #1]
	ldr x11, [sp, #112]
	str w10, [x11]
	ldr w8, [x8, #4]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x9, #1]
	ldr x10, [sp, #104]
	str w8, [x10]
	ldr w8, [x9, #4]
	stur w8, [x10, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #6
	bl luna_core::vm::exec::Vm::arith_fast
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	ldur x10, [x24, #9]
	str x10, [sp, #2400]
	ldur x10, [x24, #16]
	stur x10, [x20, #87]
	add x10, sp, #2320
	cmp x9, #1
	b.ne LBB568_695
	ldr x9, [sp, #2400]
	stur x9, [x29, #-144]
	ldur x9, [x10, #87]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
	lsr w8, w28, #16
	ldp x10, x9, [sp, #224]
	ldr x9, [x9]
	add w8, w10, w8, uxtb
	ldr q0, [x9, w8, uxtw #4]
	str q0, [sp, #1360]
	ldrb w8, [sp, #1360]
	ldr x9, [sp, #1368]
	cmp w8, #4
	b.eq LBB568_910
	cmp w8, #3
	add x19, sp, #1744
	b.eq LBB568_906
	cmp w8, #2
	b.ne LBB568_912
	stp xzr, x9, [sp, #288]
	b LBB568_997
	lsr w8, w28, #7
	ldr x9, [x26, #304]
	ldr x10, [sp, #224]
	add w8, w10, w8, uxtb
	ldr q0, [x9, w8, uxtw #4]
	lsr w8, w28, #16
	add w8, w10, w8, uxtb
	ldr q1, [x9, w8, uxtw #4]
	str q0, [sp, #1152]
	str q1, [sp, #1168]
	add w8, w10, w28, lsr #24
	ldr q0, [x9, w8, uxtw #4]
	str q0, [sp, #1184]
	add x0, sp, #1984
	add x2, sp, #1152
	add x3, sp, #1168
	add x4, sp, #1184
	b LBB568_395
	lsr w9, w28, #7
	ldp x10, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w10, w9, uxtb
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #1504]
	ldr x10, [x23, #16]
	ubfx x0, x28, #16, #8
	ldr x1, [x10, #40]
	cmp x1, x0
	b.ls LBB568_1393
	ldr x10, [x10, #32]
	ldr q0, [x10, x0, lsl #4]
	str q0, [sp, #1520]
	ldrb w11, [sp, #1504]
	cmp w11, #2
	b.ne LBB568_760
	ldrb w11, [sp, #1520]
	cmp w11, #2
	b.ne LBB568_760
	ldr x8, [sp, #1512]
	ldr x9, [sp, #1528]
	cmp x8, x9
	cset w8, eq
	and w9, w28, #0x8000
	cmp w8, w9, lsr #15
	b.eq LBB568_2
	b LBB568_391
	lsr w8, w28, #16
	ldp x10, x9, [sp, #224]
	ldr x9, [x9]
	add w8, w10, w8, uxtb
	ldr q0, [x9, w8, uxtw #4]
	str q0, [sp, #1056]
	ldr x8, [x23, #16]
	lsr x0, x28, #24
	ldr x1, [x8, #40]
	cmp x1, x0
	b.ls LBB568_1407
	ldr x8, [x8, #32]
	ldr q0, [x8, x0, lsl #4]
	str q0, [sp, #1072]
	ldrb w8, [sp, #1056]
	cmp w8, #5
	b.ne LBB568_507
	ldr x8, [sp, #1064]
	ldr x9, [x8, #152]
	cbnz x9, LBB568_507
	ldrb w9, [sp, #1072]
	cmp w9, #4
	b.ne LBB568_507
	ldr x9, [sp, #1080]
	ldp x19, x22, [x8, #72]
	str x9, [sp, #1992]
	mov w8, #4
	strb w8, [sp, #1984]
	cbz x22, LBB568_947
	add x1, sp, #1984
	mov x0, x22
	bl luna_core::runtime::table::Table::main_position
	mov x23, x0
	cmp x0, x22
	b.hs LBB568_1384
LBB568_291:
	mov w8, #40
	madd x24, x23, x8, x19
	ldrb w8, [x24, #36]
	tbnz w8, #0, LBB568_293
	add x1, sp, #1984
	mov x0, x24
	bl luna_core::runtime::value::Value::raw_eq
	tbnz w0, #0, LBB568_1119
LBB568_293:
	ldrsw x23, [x24, #32]
	cmn w23, #1
	b.eq LBB568_947
	cmp x22, x23
	b.hi LBB568_291
	b LBB568_1384
	lsr w9, w28, #16
	ldp x10, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w10, w9, uxtb
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #1328]
	ldrb w10, [sp, #1328]
	ldr x9, [sp, #1336]
	cmp w10, #4
	b.eq LBB568_916
	cmp w10, #3
	b.eq LBB568_915
	cmp w10, #2
	b.ne LBB568_921
	stp xzr, x9, [sp, #288]
	b LBB568_919
	ldr x8, [x26, #336]
	cbz x8, LBB568_1333
	ldr x9, [x26, #328]
	add x8, x9, x8, lsl #6
	ldur x9, [x8, #-64]
	cmp x9, #1
	b.eq LBB568_1333
	mov w9, #255
	ands w10, w9, w28, lsr #16
	sub w10, w10, #1
	tst w9, w28, lsr #16
	ubfx w19, w28, #7, #8
	ldr w9, [x26, #1792]
	mov x12, x26
	ldr x11, [sp, #224]
	add w26, w19, w11
	mvn w11, w26
	add w9, w9, w11
	csel w24, w9, w10, eq
	ldr x1, [x12, #312]
	cmp x1, x26
	b.ls LBB568_1396
	ldur w22, [x8, #-32]
	ldur w23, [x8, #-24]
	ldur w20, [x8, #-16]
	ldur w21, [x8, #-8]
	ldr x8, [sp, #232]
	ldr x8, [x8]
	ldr q0, [x8, x26, lsl #4]
	str q0, [sp, #1696]
	ldrb w27, [sp, #1696]
	and w8, w27, #0xe
	cmp w8, #6
	str w21, [sp, #200]
	str w22, [sp, #208]
	b.ne LBB568_786
LBB568_303:
	cmp w27, #6
	b.ne LBB568_858
	mov w8, #11
	strb w8, [sp, #1984]
	add x0, sp, #560
	add x3, sp, #1984
	ldr x1, [sp, #216]
	ldr w2, [sp, #208]
	bl luna_core::vm::exec::Vm::close_slots
	ldrb w8, [sp, #560]
	cmp w8, #11
	add x19, sp, #1744
	b.ne LBB568_1267
	mov w9, #0
	ldr w12, [sp, #200]
LBB568_306:
	cmp w9, w24
	cinc w10, w9, lo
	ldr x8, [sp, #216]
	ldr x1, [x8, #312]
	add w0, w9, w26
	cmp x1, x0
	b.ls LBB568_1375
	add w8, w9, w23
	cmp x1, x8
	b.ls LBB568_1376
	ldr x11, [sp, #232]
	ldr x11, [x11]
	ldr q0, [x11, x0, lsl #4]
	str q0, [x11, x8, lsl #4]
	cmp w9, w24
	b.hs LBB568_310
	mov x9, x10
	cmp w10, w24
	b.ls LBB568_306
LBB568_310:
	adds w8, w12, #1
	csinv w8, w8, wzr, lo
	ldr x26, [sp, #216]
	str w8, [x26, #1820]
	ldr x8, [x26, #336]
	cbz x8, LBB568_312
	sub x8, x8, #1
	str x8, [x26, #336]
LBB568_312:
	add x0, sp, #1984
	mov x1, x26
	mov x2, x23
	mov w3, #1
	mov x4, x24
	mov x5, x20
	mov w6, #0
	bl luna_core::vm::exec::Vm::begin_call
	ldrb w9, [sp, #1984]
	ldrb w8, [sp, #1985]
	cmp w9, #11
	b.ne LBB568_1320
	tbnz w8, #0, LBB568_2
	ldr x8, [x26, #336]
	ldr x9, [sp, #96]
	cmp x8, x9
	b.hs LBB568_2
	b LBB568_1329
	ubfx w2, w28, #16, #8
	add x8, sp, #976
	mov x0, x26
	mov x1, x23
	bl luna_core::vm::exec::Vm::upval_get
	ldr x8, [x23, #16]
	lsr x0, x28, #24
	ldr x1, [x8, #40]
	cmp x1, x0
	b.ls LBB568_1397
	ldr x8, [x8, #32]
	ldr q0, [x8, x0, lsl #4]
	str q0, [sp, #992]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w4, w9, w8, uxtb
	add x0, sp, #1984
	add x2, sp, #976
	add x3, sp, #992
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_index
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
	ldr x8, [x23, #16]
	lsr x0, x28, #15
	ldr x1, [x8, #56]
	cmp x1, x0
	b.ls LBB568_1398
	mov x27, x23
	ldr x8, [x8, #48]
	ldr x23, [x8, x0, lsl #3]
	ldr x24, [x23, #72]
	mov w8, #8
	str xzr, [sp, #1984]
	str x8, [sp, #1992]
	str xzr, [sp, #2000]
	cmp x24, #3
	b.hs LBB568_1071
	mov x20, #0
	mov x21, x24
	cbz x24, LBB568_971
LBB568_320:
	ldr x19, [x23, #64]
	ldrb w8, [x19, #17]
	tbz w8, #0, LBB568_841
	ldrb w8, [x19, #16]
	ldr x9, [sp, #224]
	add w1, w9, w8
	mov x0, x26
	bl luna_core::vm::exec::Vm::find_or_create_upval
	mov x22, x0
	cmp x24, #3
	b.lo LBB568_843
LBB568_323:
	ldr x25, [sp, #2000]
	cmp x25, x20
	b.ne LBB568_325
	add x0, sp, #1984
	bl alloc::raw_vec::RawVec<T,A>::grow_one
LBB568_325:
	ldr x8, [sp, #1992]
	str x22, [x8, x25, lsl #3]
	add x8, x25, #1
	str x8, [sp, #2000]
	cmp x21, #1
	b.ne LBB568_844
	b LBB568_971
	ldr x8, [x26, #336]
	cbz x8, LBB568_1333
	ldr x9, [x26, #328]
	add x8, x9, x8, lsl #6
	ldur x9, [x8, #-64]
	cmp x9, #1
	b.eq LBB568_1333
	ldr x9, [x23, #16]
	ldur w0, [x8, #-28]
	ldr x1, [x9, #24]
	cmp x1, x0
	b.ls LBB568_1408
	ldr x9, [x9, #16]
	ldr w9, [x9, x0, lsl #2]
	add w10, w0, #1
	stur w10, [x8, #-28]
	ldr x8, [x23, #16]
	lsr x0, x9, #7
	ldr x1, [x8, #40]
	cmp x1, x0
	b.hi LBB568_373
	b LBB568_1391
	lsr w19, w28, #7
	mov w8, #2147483520
	cmp w28, w8
	b.hs LBB568_1069
	ldrb w8, [x26, #989]
	tbz w8, #0, LBB568_1069
	ldr x8, [x23, #16]
	ldr w9, [x8, #168]
	mov w10, #2147483647
	cmp w9, w10
	b.hs LBB568_334
	add w10, w9, #1
	str w10, [x8, #168]
LBB568_334:
	ldrb w10, [x8, #180]
	tbnz w10, #0, LBB568_1069
	ldr x10, [x8, #184]
	mov x11, #9223372036854775807
	cmp x10, x11
	b.hs LBB568_1275
	ldr w11, [sp, #284]
	mov w12, #2
	movk w12, #65280, lsl #16
	add w12, w19, w12
	add w11, w12, w11
	bic w22, w11, w11, asr #31
	add x11, x10, #1
	str x11, [x8, #184]
	ldp x11, x12, [x8, #200]
	lsl x13, x12, #3
LBB568_337:
	mov x12, x13
	cbz x13, LBB568_339
	ldr x13, [x11], #8
	ldr w14, [x13, #232]
	sub x13, x12, #8
	cmp w14, w22
	b.ne LBB568_337
LBB568_339:
	str x10, [x8, #184]
	cmp w9, #64
	b.lo LBB568_1069
	ldr x8, [x26, #960]
	orr x8, x12, x8
	cbnz x8, LBB568_1069
	mov x28, x23
	ldr x8, [x23, #16]
	ldrb w23, [x8, #84]
	add x0, sp, #1984
	mov x1, x23
	mov w2, #0
	mov w3, #1
	mov w4, #1
	bl alloc::raw_vec::RawVecInner<A>::try_allocate_in
	ldr x8, [sp, #1984]
	ldr x0, [sp, #1992]
	cmp x8, #1
	b.eq LBB568_1364
	ldr x8, [sp, #2000]
	stp x0, x8, [sp, #288]
	str xzr, [sp, #304]
	ldr x8, [sp, #224]
	cbz w23, LBB568_1067
	mov x24, #0
	lsl x21, x8, #4
	b LBB568_345
LBB568_344:
	ldr x8, [sp, #296]
	strb w20, [x8, x24]
	str x25, [sp, #304]
	add x8, x27, #1
	add x21, x21, #16
	mov x24, x25
	cmp x23, x25
	b.eq LBB568_1067
LBB568_345:
	ldr x1, [x26, #312]
	mov x27, x8
	cmp x8, x1
	b.hs LBB568_1389
	add x25, x24, #1
	ldr x8, [sp, #232]
	ldr x8, [x8]
	ldrb w20, [x8, x21]
Lloh3881:
	adrp x9, LJTI568_6@PAGE
Lloh3882:
	add x9, x9, LJTI568_6@PAGEOFF
	adr x10, LBB568_347
	ldrb w11, [x9, x20]
	add x10, x10, x11, lsl #2
	br x10
LBB568_347:
	add x8, x8, x21
	ldr w8, [x8, #8]
	tst w8, #0x1
	mov w8, #1
	cinc w20, w8, ne
	b LBB568_357
	mov w20, #5
	b LBB568_357
	mov w20, #10
	b LBB568_357
	mov w20, #3
	b LBB568_357
	mov w20, #4
	b LBB568_357
	mov w20, #8
	b LBB568_357
	mov w20, #6
	b LBB568_357
	mov w20, #7
	b LBB568_357
	mov w20, #11
	b LBB568_357
	mov w20, #9
LBB568_357:
	ldr x8, [sp, #288]
	cmp x24, x8
	b.ne LBB568_344
	add x0, sp, #288
	bl <alloc::raw_vec::RawVec<u8>>::grow_one
	b LBB568_344
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x10, [sp, #224]
	add w8, w10, w8, uxtb
	ldr q1, [x9, w8, uxtw #4]
	add w8, w10, w28, lsr #24
	ldr q0, [x9, w8, uxtw #4]
	stp q1, q0, [sp, #1008]
	lsr w8, w28, #7
	add w4, w10, w8, uxtb
	add x0, sp, #1984
	add x2, sp, #1008
	add x3, sp, #1024
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_index
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
	ubfx w2, w28, #16, #8
	add x8, sp, #944
	mov x0, x26
	mov x1, x23
	bl luna_core::vm::exec::Vm::upval_get
	lsr w8, w28, #7
	ldr x9, [x26, #304]
	ldr x10, [sp, #224]
	add w8, w10, w8, uxtb
	ldr q0, [sp, #944]
	str q0, [x9, w8, uxtw #4]
	b LBB568_2
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x12, [sp, #224]
	add w8, w12, w8, uxtb
	add x8, x9, w8, uxtw #4
	ldrb w19, [x8]
	ldur w10, [x8, #1]
	str w10, [sp, #1744]
	ldr w10, [x8, #4]
	add x11, sp, #1744
	stur w10, [x11, #3]
	ldr x21, [x8, #8]
	add w10, w12, w28, lsr #24
	add x9, x9, w10, uxtw #4
	add x24, sp, #1984
	ldrb w22, [x9]
	ldur w10, [x9, #1]
	stur w10, [x29, #-160]
	ldr w10, [x9, #4]
	add x20, sp, #2320
	stur w10, [x20, #195]
	ldr x23, [x9, #8]
	strb w19, [sp, #288]
	ldur w10, [x8, #1]
	ldr x11, [sp, #112]
	str w10, [x11]
	ldr w8, [x8, #4]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x9, #1]
	ldr x10, [sp, #104]
	str w8, [x10]
	ldr w8, [x9, #4]
	stur w8, [x10, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #4
	bl luna_core::vm::exec::Vm::arith_fast
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	ldur x10, [x24, #9]
	stur x10, [x29, #-240]
	ldur x10, [x24, #16]
	stur x10, [x20, #119]
	add x10, sp, #2320
	cmp x9, #1
	b.ne LBB568_698
	ldur x9, [x29, #-240]
	stur x9, [x29, #-144]
	ldur x9, [x10, #119]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x12, [sp, #224]
	add w8, w12, w8, uxtb
	add x8, x9, w8, uxtw #4
	ldrb w19, [x8]
	ldur w10, [x8, #1]
	str w10, [sp, #1744]
	ldr w10, [x8, #4]
	add x11, sp, #1744
	stur w10, [x11, #3]
	ldr x21, [x8, #8]
	add w10, w12, w28, lsr #24
	add x9, x9, w10, uxtw #4
	add x24, sp, #1984
	ldrb w22, [x9]
	ldur w10, [x9, #1]
	stur w10, [x29, #-160]
	ldr w10, [x9, #4]
	add x20, sp, #2320
	stur w10, [x20, #195]
	ldr x23, [x9, #8]
	strb w19, [sp, #288]
	ldur w10, [x8, #1]
	ldr x11, [sp, #112]
	str w10, [x11]
	ldr w8, [x8, #4]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x9, #1]
	ldr x10, [sp, #104]
	str w8, [x10]
	ldr w8, [x9, #4]
	stur w8, [x10, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #3
	bl luna_core::vm::exec::Vm::arith_fast
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	ldur x10, [x24, #9]
	stur x10, [x29, #-224]
	ldur x10, [x24, #16]
	stur x10, [x20, #135]
	add x10, sp, #2320
	cmp x9, #1
	b.ne LBB568_701
	ldur x9, [x29, #-224]
	stur x9, [x29, #-144]
	ldur x9, [x10, #135]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
	lsr w8, w28, #7
	mov w9, #-65535
	add w9, w9, w28, lsr #15
	scvtf d0, w9
	ldp x10, x9, [sp, #224]
	ldr x9, [x9]
	add w8, w10, w8, uxtb
	add x8, x9, w8, uxtw #4
	b LBB568_1033
	mov x0, x26
	bl luna_core::runtime::heap::Heap::new_table
	mov x22, x0
	cbz w24, LBB568_791
	add w8, w25, #1
	mov w19, #1
	mov x21, x24
	mov x20, x24
LBB568_368:
	mov w24, w8
	ldr x1, [x26, #312]
	cmp x1, x24
	b.ls LBB568_1366
	ldr x8, [x26, #304]
	ldr q0, [x8, x24, lsl #4]
	str q0, [sp, #560]
	str x19, [sp, #1992]
	mov w8, #2
	strb w8, [sp, #1984]
	add x2, sp, #1984
	add x3, sp, #560
	mov x0, x22
	mov x1, x26
	bl luna_core::runtime::table::Table::set_norm
	add w8, w24, #1
	add x19, x19, #1
	subs x20, x20, #1
	b.ne LBB568_368
	b LBB568_792
	ubfx w2, w28, #7, #8
	add x8, sp, #1104
	mov x0, x26
	mov x1, x23
	bl luna_core::vm::exec::Vm::upval_get
	ldr x8, [x23, #16]
	ubfx x0, x28, #16, #8
	ldr x1, [x8, #40]
	cmp x1, x0
	b.ls LBB568_1399
	ldr x8, [x8, #32]
	ldr q0, [x8, x0, lsl #4]
	str q0, [sp, #1120]
	ldr x8, [x26, #304]
	ldr x9, [sp, #224]
	add w9, w9, w28, lsr #24
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #1136]
	add x0, sp, #1984
	add x2, sp, #1104
	add x3, sp, #1120
	add x4, sp, #1136
	b LBB568_395
	ldr x8, [x23, #16]
	lsr x0, x28, #15
	ldr x1, [x8, #40]
	cmp x1, x0
	b.ls LBB568_1390
LBB568_373:
	ldr x8, [x8, #32]
	lsr w9, w28, #7
	ldp x11, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w11, w9, uxtb
	ldr q0, [x8, x0, lsl #4]
	str q0, [x10, w9, uxtw #4]
	b LBB568_2
	lsr w8, w28, #7
	ldp x10, x9, [sp, #224]
	ldr x9, [x9]
	add w8, w10, w8, uxtb
	add x8, x9, w8, uxtw #4
	mov w9, #1
	strb w9, [x8]
	strb w9, [x8, #8]
	b LBB568_2
	lsr w8, w28, #16
	ldp x11, x9, [sp, #224]
	ldr x9, [x9]
	add w8, w11, w8, uxtb
	lsr w10, w28, #7
	add w10, w11, w10, uxtb
	ldr q0, [x9, w8, uxtw #4]
	str q0, [x9, w10, uxtw #4]
	b LBB568_2
	lsr w8, w28, #7
	mov w9, #-65535
	add w9, w9, w28, lsr #15
	sxtw x9, w9
	b LBB568_998
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x12, [sp, #224]
	add w8, w12, w8, uxtb
	add x8, x9, w8, uxtw #4
	ldrb w19, [x8]
	ldur w10, [x8, #1]
	str w10, [sp, #1744]
	ldr w10, [x8, #4]
	add x11, sp, #1744
	stur w10, [x11, #3]
	ldr x21, [x8, #8]
	add w10, w12, w28, lsr #24
	add x9, x9, w10, uxtw #4
	add x24, sp, #1984
	ldrb w22, [x9]
	ldur w10, [x9, #1]
	stur w10, [x29, #-160]
	ldr w10, [x9, #4]
	add x20, sp, #2320
	stur w10, [x20, #195]
	ldr x23, [x9, #8]
	strb w19, [sp, #288]
	ldur w10, [x8, #1]
	ldr x11, [sp, #112]
	str w10, [x11]
	ldr w8, [x8, #4]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x9, #1]
	ldr x10, [sp, #104]
	str w8, [x10]
	ldr w8, [x9, #4]
	stur w8, [x10, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #10
	bl luna_core::vm::exec::Vm::arith_fast
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	ldur x10, [x24, #9]
	str x10, [sp, #2336]
	ldur x10, [x24, #16]
	stur x10, [x20, #23]
	add x10, sp, #2320
	cmp x9, #1
	b.ne LBB568_704
	ldr x9, [sp, #2336]
	stur x9, [x29, #-144]
	ldur x9, [x10, #23]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
	lsr w8, w28, #7
	ldr x9, [x26, #304]
	ldr x10, [sp, #224]
	add w8, w10, w8, uxtb
	ldr q0, [x9, w8, uxtw #4]
	add w8, w10, w28, lsr #24
	ldr q1, [x9, w8, uxtw #4]
	str q0, [sp, #1200]
	str q1, [sp, #1216]
	ubfx x8, x28, #16, #8
	str x8, [sp, #1992]
	mov w8, #2
	strb w8, [sp, #1984]
	add x0, sp, #560
	add x2, sp, #1200
	add x3, sp, #1984
	add x4, sp, #1216
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_newindex
	ldrb w8, [sp, #560]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1267
	lsr w9, w28, #7
	ldp x11, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w11, w9, uxtb
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #1440]
	lsr w10, w28, #16
	add w10, w11, w10, uxtb
	ldr q0, [x8, w10, uxtw #4]
	str q0, [sp, #1456]
	ldrb w11, [sp, #1440]
	cmp w11, #2
	b.ne LBB568_707
	ldrb w11, [sp, #1456]
	cmp w11, #2
	b.ne LBB568_707
	ldr x8, [sp, #1448]
	ldr x9, [sp, #1464]
	cmp x8, x9
	cset w8, eq
	and w9, w28, #0x8000
	cmp w8, w9, lsr #15
	b.eq LBB568_2
	b LBB568_391
	lsr w8, w28, #16
	ldp x12, x11, [sp, #224]
	ldr x9, [x11]
	add w8, w12, w8, uxtb
	ldr q0, [x9, w8, uxtw #4]
	str q0, [sp, #1296]
	ubfx w8, w28, #7, #8
	add w10, w12, w8
	add w10, w10, #1
	str q0, [x9, w10, uxtw #4]
	tbnz w28, #15, LBB568_708
	ldr x9, [x11]
	add w10, w12, w28, lsr #24
	add x9, x9, w10, uxtw #4
	b LBB568_710
	lsr w8, w28, #7
	ldp x10, x9, [sp, #224]
	ldr x9, [x9]
	add w8, w10, w8, uxtb
	add x8, x9, w8, uxtw #4
	mov w9, #1
	strb w9, [x8]
	strb wzr, [x8, #8]
	b LBB568_2
	lsr w9, w28, #16
	ldp x13, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w13, w9, uxtb
	add x10, x8, w9, uxtw #4
	ldrb w21, [x10]
	mov x9, x10
	ldr w11, [x9, #1]!
	str w11, [sp, #1744]
	ldr w11, [x10, #4]
	add x12, sp, #1744
	stur w11, [x12, #3]
	ldr x19, [x10, #8]
	add w10, w13, w28, lsr #24
	add x11, x8, w10, uxtw #4
	mov x10, x11
	ldr w12, [x10, #1]!
	ldrb w23, [x11]
	stur w12, [x29, #-160]
	ldr w12, [x11, #4]
	add x13, sp, #2320
	stur w12, [x13, #195]
	ldr x22, [x11, #8]
	cmp w21, #3
	b.eq LBB568_804
	cmp w21, #2
	ccmp w23, #2, #0, eq
	b.ne LBB568_806
	add x9, x22, x19
	lsr w10, w28, #7
	b LBB568_920
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x10, [sp, #224]
	add w8, w10, w8, uxtb
	ldr q0, [x9, w8, uxtw #4]
	str q0, [sp, #1040]
	lsr x8, x28, #24
	str x8, [sp, #1992]
	mov w8, #2
	strb w8, [sp, #1984]
	lsr w8, w28, #7
	add w4, w10, w8, uxtb
	add x0, sp, #560
	add x2, sp, #1040
	add x3, sp, #1984
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_index
	ldrb w8, [sp, #560]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1267
	lsr w8, w28, #7
	ldr x9, [x26, #304]
	ldr x10, [sp, #224]
	add w8, w10, w8, uxtb
	add x8, x9, w8, uxtw #4
	mov w9, #1
	strb w9, [x8]
	strb wzr, [x8, #8]
LBB568_391:
	ldp x9, x8, [x26, #328]
	add x9, x9, x8, lsl #6
	ldr x10, [x9, #-64]!
	cmp x8, #0
	csel x8, xzr, x9, eq
	cmp x10, #1
	b.eq LBB568_1336
LBB568_392:
	ldr w9, [x8, #36]
	add w9, w9, #1
	str w9, [x8, #36]
	b LBB568_2
	lsr w9, w28, #7
	ldp x10, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w10, w9, uxtb
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #1232]
	ldr x9, [x23, #16]
	ubfx x0, x28, #16, #8
	ldr x1, [x9, #40]
	cmp x1, x0
	b.ls LBB568_1395
	ldr x9, [x9, #32]
	ldr q0, [x9, x0, lsl #4]
	str q0, [sp, #1248]
	add w9, w10, w28, lsr #24
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #1264]
	add x0, sp, #1984
	add x2, sp, #1232
	add x3, sp, #1248
	add x4, sp, #1264
LBB568_395:
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_newindex
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
	lsr w9, w28, #7
	ldp x10, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w10, w9, uxtb
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #960]
	ubfx x0, x28, #16, #8
	ldr w1, [x23, #32]
	cmp w0, w1
	b.hs LBB568_1394
	ldr x10, [x23, #24]
	ldr x10, [x10, x0, lsl #3]
	ldr w11, [x10, #16]
	tbz w11, #0, LBB568_761
	ldr q0, [x8, w9, uxtw #4]
	stur q0, [x20, #4]
	mov w8, #1
	str w8, [x10, #16]
	ldr q0, [sp, #1984]
	stur q0, [x10, #20]
	ldr w8, [sp, #2000]
	stur w8, [x10, #36]
	ldrb w8, [x10, #9]
	tbz w8, #2, LBB568_2
	ldrb w8, [sp, #960]
	sub w8, w8, #4
	cmp w8, #5
	b.hi LBB568_2
	ldr x19, [sp, #968]
	ldrb w8, [x19, #9]
	tst w8, #0x3
	b.eq LBB568_2
	ldrb w9, [x19, #8]
	cbz w9, LBB568_1147
	and w8, w8, #0xfc
	strb w8, [x19, #9]
	ldr x20, [x26, #64]
	ldr x8, [x26, #48]
	cmp x20, x8
	b.eq LBB568_1239
LBB568_403:
	ldr x8, [x26, #56]
	str x19, [x8, x20, lsl #3]
	add x8, x20, #1
	str x8, [x26, #64]
	b LBB568_2
	mov w11, #0
	lsr w10, w28, #7
	ubfx w8, w28, #16, #8
	ldp x12, x9, [sp, #224]
	ldr x9, [x9]
	add w10, w12, w10, uxtb
LBB568_405:
	add w12, w10, w11
	ubfiz x12, x12, #4, #32
	strb wzr, [x9, x12]
	cmp w11, w8
	cinc w12, w11, lo
	b.hs LBB568_2
	mov x11, x12
	cmp w12, w8
	b.ls LBB568_405
	b LBB568_2
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x12, [sp, #224]
	add w8, w12, w8, uxtb
	add x8, x9, w8, uxtw #4
	ldrb w19, [x8]
	ldur w10, [x8, #1]
	str w10, [sp, #1744]
	ldr w10, [x8, #4]
	add x11, sp, #1744
	stur w10, [x11, #3]
	ldr x21, [x8, #8]
	add w10, w12, w28, lsr #24
	add x9, x9, w10, uxtw #4
	add x24, sp, #1984
	ldrb w22, [x9]
	ldur w10, [x9, #1]
	stur w10, [x29, #-160]
	ldr w10, [x9, #4]
	add x20, sp, #2320
	stur w10, [x20, #195]
	ldr x23, [x9, #8]
	strb w19, [sp, #288]
	ldur w10, [x8, #1]
	ldr x11, [sp, #112]
	str w10, [x11]
	ldr w8, [x8, #4]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x9, #1]
	ldr x10, [sp, #104]
	str w8, [x10]
	ldr w8, [x9, #4]
	stur w8, [x10, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #9
	bl luna_core::vm::exec::Vm::arith_fast
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	ldur x10, [x24, #9]
	str x10, [sp, #2352]
	ldur x10, [x24, #16]
	stur x10, [x20, #39]
	add x10, sp, #2320
	cmp x9, #1
	b.ne LBB568_711
	ldr x9, [sp, #2352]
	stur x9, [x29, #-144]
	ldur x9, [x10, #39]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x12, [sp, #224]
	add w8, w12, w8, uxtb
	add x8, x9, w8, uxtw #4
	ldrb w19, [x8]
	ldur w10, [x8, #1]
	str w10, [sp, #1744]
	ldr w10, [x8, #4]
	add x11, sp, #1744
	stur w10, [x11, #3]
	ldr x21, [x8, #8]
	add w10, w12, w28, lsr #24
	add x9, x9, w10, uxtw #4
	add x24, sp, #1984
	ldrb w22, [x9]
	ldur w10, [x9, #1]
	stur w10, [x29, #-160]
	ldr w10, [x9, #4]
	add x20, sp, #2320
	stur w10, [x20, #195]
	ldr x23, [x9, #8]
	strb w19, [sp, #288]
	ldur w10, [x8, #1]
	ldr x11, [sp, #112]
	str w10, [x11]
	ldr w8, [x8, #4]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x9, #1]
	ldr x10, [sp, #104]
	str w8, [x10]
	ldr w8, [x9, #4]
	stur w8, [x10, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #11
	bl luna_core::vm::exec::Vm::arith_fast
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	ldur x10, [x24, #9]
	str x10, [sp, #2320]
	ldur x10, [x24, #16]
	stur x10, [x20, #7]
	add x10, sp, #2320
	cmp x9, #1
	b.ne LBB568_714
	ldr x9, [sp, #2320]
	stur x9, [x29, #-144]
	ldur x9, [x10, #7]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
	ldp x8, x1, [x26, #304]
	ldr x9, [sp, #224]
	add w9, w9, w28, lsr #24
	add x10, x8, w9, uxtw #4
	ldrb w9, [x10]
	ldr x10, [x10, #8]
	cmp w9, #2
	b.eq LBB568_932
	cmp w9, #3
	add x12, sp, #1744
	b.eq LBB568_927
	cmp w9, #4
	ldr x11, [sp, #16]
	b.ne LBB568_933
	ldr w9, [x10, #32]
	cmp w9, #1
	b.ne LBB568_933
	ldrb w9, [x10, #40]
	mov w10, #2
	cmp w9, #110
	csel w9, w10, wzr, eq
	csel x11, x24, x11, eq
	b LBB568_1002
	lsr w8, w28, #7
	ldp x10, x9, [sp, #224]
	ldr x9, [x9]
	add w8, w10, w8, uxtb
	ubfiz x8, x8, #4, #32
	ldrb w8, [x9, x8]
	cbz w8, LBB568_2
	b LBB568_1280
	lsr w8, w28, #7
	ldp x10, x9, [sp, #224]
	ldr x9, [x9]
	add w8, w10, w8, uxtb
	add x8, x9, w8, uxtw #4
	ldrb w9, [x8]
	cbz w9, LBB568_811
	cmp w9, #1
	b.ne LBB568_823
	ldrb w8, [x8, #8]
	and w8, w8, #0x1
	and w9, w28, #0x8000
	cmp w8, w9, lsr #15
	b.eq LBB568_2
	b LBB568_391
	lsr w9, w28, #16
	ldp x13, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w13, w9, uxtb
	add x10, x8, w9, uxtw #4
	ldrb w21, [x10]
	mov x9, x10
	ldr w11, [x9, #1]!
	str w11, [sp, #1744]
	ldr w11, [x10, #4]
	add x12, sp, #1744
	stur w11, [x12, #3]
	ldr x19, [x10, #8]
	add w10, w13, w28, lsr #24
	add x11, x8, w10, uxtw #4
	mov x10, x11
	ldr w12, [x10, #1]!
	ldrb w23, [x11]
	stur w12, [x29, #-160]
	ldr w12, [x11, #4]
	add x13, sp, #2320
	stur w12, [x13, #195]
	ldr x22, [x11, #8]
	cmp w21, #3
	b.eq LBB568_812
	cmp w21, #2
	ccmp w23, #2, #0, eq
	b.ne LBB568_814
	sub x9, x19, x22
	lsr w10, w28, #7
	b LBB568_920
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w2, w9, w8, uxtb
	add x0, sp, #1984
	mov x1, x26
	bl luna_core::vm::exec::Vm::register_tbc
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
	mov x0, x26
	bl luna_core::runtime::heap::Heap::new_table
	lsr w8, w28, #7
	ldr x9, [x26, #304]
	ldr x10, [sp, #224]
	add w8, w10, w8, uxtb
	add x9, x9, w8, uxtw #4
	mov w10, #5
	strb w10, [x9]
	str x0, [x9, #8]
	ldrb w9, [x26, #1832]
	tbnz w9, #0, LBB568_2
	ldrb w9, [x26, #268]
	tbnz w9, #0, LBB568_2
	ldp x9, x10, [x26, #240]
	cmp x9, x10
	b.lo LBB568_2
	add w8, w8, #1
	str w8, [x26, #1816]
	ldr x8, [x26, #1640]
	cmp x8, #1
	csinc x8, x8, xzr, gt
	mov w9, #400
	udiv x8, x9, x8
	cmp x8, #1
	csinc x8, x8, xzr, hi
	ldr x9, [x26, #232]
	udiv x8, x9, x8
	mov w9, #64000
	cmp x8, x9
	csel x1, x8, x9, hi
	mov x0, x26
	bl luna_core::vm::exec::Vm::gc_step
	cbz w0, LBB568_2
	ldr x8, [x26, #1632]
	bic x8, x8, x8, asr #63
	ldr x9, [x26, #240]
	umulh x10, x9, x8
	cbnz x10, LBB568_743
LBB568_429:
	mul x8, x9, x8
LBB568_430:
	lsr x8, x8, #2
	mov x9, #62915
	movk x9, #23592, lsl #16
	movk x9, #49807, lsl #32
	movk x9, #10485, lsl #48
	umulh x8, x8, x9
	lsr x8, x8, #2
	mov w9, #1048576
	cmp x8, #256, lsl #12
	csel x8, x8, x9, hi
	str x8, [x26, #248]
	b LBB568_2
	lsr w9, w28, #7
	ldp x11, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w11, w9, uxtb
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #1568]
	lsr w10, w28, #16
	add w10, w11, w10, uxtb
	ldr q0, [x8, w10, uxtw #4]
	str q0, [sp, #1584]
	ldrb w11, [sp, #1568]
	cmp w11, #2
	b.ne LBB568_717
	ldrb w11, [sp, #1584]
	cmp w11, #2
	b.ne LBB568_717
	ldr x8, [sp, #1576]
	ldr x9, [sp, #1592]
	cmp x8, x9
	cset w8, lt
	and w9, w28, #0x8000
	cmp w8, w9, lsr #15
	b.eq LBB568_2
	b LBB568_391
	lsr w9, w28, #16
	ldp x13, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w13, w9, uxtb
	add x10, x8, w9, uxtw #4
	ldrb w21, [x10]
	mov x9, x10
	ldr w11, [x9, #1]!
	str w11, [sp, #1744]
	ldr w11, [x10, #4]
	add x12, sp, #1744
	stur w11, [x12, #3]
	ldr x19, [x10, #8]
	add w10, w13, w28, lsr #24
	add x11, x8, w10, uxtw #4
	mov x10, x11
	ldr w12, [x10, #1]!
	ldrb w23, [x11]
	stur w12, [x29, #-160]
	ldr w12, [x11, #4]
	add x13, sp, #2320
	stur w12, [x13, #195]
	ldr x22, [x11, #8]
	cmp w21, #3
	b.eq LBB568_819
	cmp w21, #2
	ccmp w23, #2, #0, eq
	b.ne LBB568_821
	mul x9, x22, x19
	lsr w10, w28, #7
	b LBB568_920
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x12, [sp, #224]
	add w8, w12, w8, uxtb
	add x8, x9, w8, uxtw #4
	ldrb w19, [x8]
	ldur w10, [x8, #1]
	str w10, [sp, #1744]
	ldr w10, [x8, #4]
	add x11, sp, #1744
	stur w10, [x11, #3]
	ldr x21, [x8, #8]
	add w10, w12, w28, lsr #24
	add x9, x9, w10, uxtw #4
	add x24, sp, #1984
	ldrb w22, [x9]
	ldur w10, [x9, #1]
	stur w10, [x29, #-160]
	ldr w10, [x9, #4]
	add x20, sp, #2320
	stur w10, [x20, #195]
	ldr x23, [x9, #8]
	strb w19, [sp, #288]
	ldur w10, [x8, #1]
	ldr x11, [sp, #112]
	str w10, [x11]
	ldr w8, [x8, #4]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x9, #1]
	ldr x10, [sp, #104]
	str w8, [x10]
	ldr w8, [x9, #4]
	stur w8, [x10, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #7
	bl luna_core::vm::exec::Vm::arith_fast
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	ldur x10, [x24, #9]
	str x10, [sp, #2384]
	ldur x10, [x24, #16]
	stur x10, [x20, #71]
	add x10, sp, #2320
	cmp x9, #1
	b.ne LBB568_719
	ldr x9, [sp, #2384]
	stur x9, [x29, #-144]
	ldur x9, [x10, #71]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
	lsr w8, w28, #16
	ldr x9, [x26, #304]
	ldr x12, [sp, #224]
	add w8, w12, w8, uxtb
	add x8, x9, w8, uxtw #4
	ldrb w19, [x8]
	ldur w10, [x8, #1]
	str w10, [sp, #1744]
	ldr w10, [x8, #4]
	add x11, sp, #1744
	stur w10, [x11, #3]
	ldr x21, [x8, #8]
	add w10, w12, w28, lsr #24
	add x9, x9, w10, uxtw #4
	add x24, sp, #1984
	ldrb w22, [x9]
	ldur w10, [x9, #1]
	stur w10, [x29, #-160]
	ldr w10, [x9, #4]
	add x20, sp, #2320
	stur w10, [x20, #195]
	ldr x23, [x9, #8]
	strb w19, [sp, #288]
	ldur w10, [x8, #1]
	ldr x11, [sp, #112]
	str w10, [x11]
	ldr w8, [x8, #4]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x9, #1]
	ldr x10, [sp, #104]
	str w8, [x10]
	ldr w8, [x9, #4]
	stur w8, [x10, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #8
	bl luna_core::vm::exec::Vm::arith_fast
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	ldur x10, [x24, #9]
	str x10, [sp, #2368]
	ldur x10, [x24, #16]
	stur x10, [x20, #55]
	add x10, sp, #2320
	cmp x9, #1
	b.ne LBB568_723
	ldr x9, [sp, #2368]
	stur x9, [x29, #-144]
	ldur x9, [x10, #55]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w19, w9, w8, uxtb
	ldr w8, [x26, #1792]
	mvn w9, w19
	add w8, w8, w9
	mov w9, #255
	ands w9, w9, w28, lsr #16
	csel w21, w8, w9, eq
	tbnz w28, #15, LBB568_726
	lsr w8, w28, #24
	b LBB568_730
	mov x10, x24
	ldr x24, [x26, #312]
	cmp x24, x25
	b.ls LBB568_1392
	ldr x8, [sp, #232]
	ldr x8, [x8]
	add x8, x8, x25, lsl #4
	ldrb w9, [x8]
	cmp w9, #5
	b.ne LBB568_762
	ldr x22, [x8, #8]
Lloh3883:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.789@PAGE
Lloh3884:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.789@PAGEOFF
	mov x0, x26
	mov w2, #1
	bl luna_core::runtime::heap::Heap::intern
	str x0, [sp, #1944]
	mov w8, #4
	strb w8, [sp, #1936]
	add x8, sp, #560
	add x1, sp, #1936
	mov x0, x22
	bl luna_core::runtime::table::Table::get
	ldrb w8, [sp, #560]
	cmp w8, #2
	b.ne LBB568_1291
	ldr x8, [sp, #568]
	lsr x9, x8, #30
	cbnz x9, LBB568_1291
	mov x23, x25
	ldr x24, [x26, #312]
	mov x10, x8
	b LBB568_763
	ldr w8, [x26, #1792]
	ldr x9, [x23, #16]
	ldrb w9, [x9, #84]
	ldr x10, [sp, #224]
	add w9, w10, w9
	cmp w9, w8
	csel w8, w9, w8, hi
	str w8, [x26, #1792]
	lsr w8, w28, #7
	add w22, w10, w8, uxtb
	mov w8, #11
	strb w8, [sp, #288]
	strb wzr, [sp, #1984]
	mov x0, x26
	mov x1, x22
	bl luna_core::vm::exec::Vm::close_from
	add x0, sp, #560
	add x3, sp, #288
	add x4, sp, #1984
	mov x1, x26
	mov x2, x22
	ldr x5, [sp, #96]
	bl luna_core::vm::exec::Vm::drive_close
	ldr x8, [sp, #560]
	ldr x0, [sp, #568]
	mov x9, #-9223372036854775807
	cmp x8, x9
	b.eq LBB568_1279
	tst x8, #0x7fffffffffffffff
	b.eq LBB568_2
	lsl x1, x8, #4
	b LBB568_1157
	lsr w9, w28, #16
	ldp x13, x8, [sp, #224]
	ldr x8, [x8]
	add w9, w13, w9, uxtb
	add x10, x8, w9, uxtw #4
	ldrb w21, [x10]
	mov x9, x10
	ldr w11, [x9, #1]!
	str w11, [sp, #1744]
	ldr w11, [x10, #4]
	add x12, sp, #1744
	stur w11, [x12, #3]
	ldr x19, [x10, #8]
	add w10, w13, w28, lsr #24
	add x11, x8, w10, uxtw #4
	ldrb w23, [x11]
	mov x10, x11
	ldr w12, [x10, #1]!
	stur w12, [x29, #-160]
	ldr w12, [x11, #4]
	add x20, sp, #2320
	stur w12, [x20, #195]
	ldr x22, [x11, #8]
	cmp w21, #3
	ccmp w23, #3, #0, eq
	b.eq LBB568_744
	strb w21, [sp, #288]
	ldr w8, [x9]
	ldr x11, [sp, #112]
	str w8, [x11]
	ldur w8, [x9, #3]
	stur w8, [x11, #3]
	str x19, [sp, #296]
	strb w23, [sp, #560]
	ldr w8, [x10]
	ldr x9, [sp, #104]
	str w8, [x9]
	ldur w8, [x10, #3]
	stur w8, [x9, #3]
	str x22, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #5
	bl luna_core::vm::exec::Vm::arith_fast
	ldr w9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-256]
	ldur x10, [x11, #16]
	stur x10, [x20, #103]
	add x10, sp, #2320
	tbz w9, #0, LBB568_936
	ldur x9, [x29, #-256]
	stur x9, [x29, #-144]
	ldur x9, [x10, #103]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	add x0, sp, #1984
	add w2, w8, #3
	mov x1, x26
	bl luna_core::vm::exec::Vm::register_tbc
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.ne LBB568_1295
	ldp x9, x8, [x26, #328]
	add x9, x9, x8, lsl #6
	ldr x10, [x9, #-64]!
	cmp x8, #0
	csel x8, xzr, x9, eq
	cmp x10, #1
	b.eq LBB568_1335
	ldr w9, [x8, #36]
	add w9, w9, w28, lsr #15
	str w9, [x8, #36]
	b LBB568_2
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w22, w9, w8, uxtb
	add w19, w22, #7
	ldr x23, [x26, #312]
	subs x24, x19, x23
	b.ls LBB568_745
	ldr x8, [x26, #296]
	sub x8, x8, x23
	cmp x24, x8
	b.hi LBB568_1244
	mov x8, x23
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x23, lsl #4
	cmp x24, #2
	b.lo LBB568_463
LBB568_460:
	sub x10, x23, x19
	add x10, x10, #1
LBB568_461:
	strb wzr, [x9], #16
	adds x10, x10, #1
	b.lo LBB568_461
	add x8, x24, x8
	sub x8, x8, #1
LBB568_463:
	add x19, sp, #1744
	strb wzr, [x9]
	add x23, x8, #1
	str x23, [x26, #312]
	b LBB568_746
	lsr w8, w28, #16
	ldp x9, x11, [sp, #224]
	ldr x10, [x11]
	add w8, w9, w8, uxtb
	add x8, x10, w8, uxtw #4
	ldrb w11, [x8]
	ldrb w9, [x8, #8]
	cbz w11, LBB568_827
	cmp w11, #1
	b.ne LBB568_828
	and w12, w28, #0x8000
	eor w12, w9, w12, lsr #15
	tbz w12, #0, LBB568_829
	b LBB568_391
	ldrb w8, [x26, #989]
	ubfx w19, w28, #7, #8
	tbz w8, #0, LBB568_753
	ldr x10, [x26, #304]
	ldr x8, [sp, #224]
	add w21, w19, w8
	add x8, x10, w21, uxtw #4
	ldrb w11, [x8]
	ldr x9, [x8, #8]
	add w8, w21, #1
	add x8, x10, w8, uxtw #4
	ldrb w12, [x8]
	ldr x8, [x8, #8]
	add w13, w21, #2
	add x10, x10, w13, uxtw #4
	ldrb w13, [x10]
	ldr x10, [x10, #8]
	cmp w11, #3
	b.eq LBB568_952
	cmp w11, #2
	ccmp w12, #2, #0, eq
	ccmp w13, #2, #0, eq
	b.ne LBB568_754
	ldrb w11, [x26, #1840]
	cmp w11, #3
	b.hs LBB568_1128
	add x9, x10, x9
	cmp x10, #1
	b.lt LBB568_1169
	cmp x9, x8
	b.gt LBB568_754
	b LBB568_1170
LBB568_473:
	mov w9, #0
	ldr x19, [sp, #224]
	mov x8, x19
	b LBB568_800
LBB568_474:
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	ubfx w9, w28, #16, #8
	cbz w9, LBB568_798
	sub w9, w9, #1
	b LBB568_799
LBB568_476:
	ldur w9, [x9, #-32]
	lsr w10, w28, #7
	add w9, w9, w10, uxtb
	ldr w10, [x26, #1792]
	cmp w10, w9
	cset w12, hi
	mvn w9, w9
	add w9, w10, w9
	str w9, [sp, #160]
LBB568_477:
	mov x27, x28
	and w9, w28, #0xff
	ldr x13, [x23, #16]
	ldr w28, [sp, #284]
	and w9, w9, #0x7e
	ldr x22, [x26, #960]
	cmp w9, #50
	b.ne LBB568_503
	tst w21, #0xff
	b.eq LBB568_503
	ldrb w9, [x26, #990]
	tbz w9, #0, LBB568_503
	str x24, [sp, #176]
	mov x24, x23
	cmp w8, #51
	cset w19, eq
	sub w23, w21, #1
	ldr x8, [x22, #56]
	str x25, [sp, #144]
	cbz x8, LBB568_487
	ldr x9, [x22, #48]
	lsl x10, x8, #5
	add x8, x9, x10
	sub x8, x8, #24
	neg x9, x10
	b LBB568_483
LBB568_482:
	sub x8, x8, #32
	adds x9, x9, #32
	b.eq LBB568_487
LBB568_483:
	ldrb w10, [x8, #16]
	cmp w10, w23, uxtb
	b.ne LBB568_482
	ldr w10, [x8, #12]
	and w10, w10, #0x7f
	cmp w10, #47
	b.ne LBB568_482
	ldr w9, [x8, #8]
	add w20, w9, #1
	ldr x25, [x8]
	ldr x26, [x22, #80]
	ldr x8, [x22, #64]
	cmp x26, x8
	ldr w9, [sp, #160]
	b.ne LBB568_488
LBB568_486:
	add x0, x22, #64
	str w12, [sp, #136]
	str x13, [sp, #152]
	bl alloc::raw_vec::RawVec<T,A>::grow_one
	ldr x13, [sp, #152]
	ldr w9, [sp, #160]
	ldr w12, [sp, #136]
	b LBB568_488
LBB568_487:
	add x8, x22, #88
	mov x20, x28
	ldr x25, [x8]
	ldr x26, [x22, #80]
	ldr x8, [x22, #64]
	cmp x26, x8
	ldr w9, [sp, #160]
	b.eq LBB568_486
LBB568_488:
	ldr x8, [x22, #72]
	add x8, x8, x26, lsl #4
	str x25, [x8]
	str w20, [x8, #8]
	strb w21, [x8, #12]
	strb w23, [x8, #13]
	strb w19, [x8, #14]
	add x8, x26, #1
	str x8, [x22, #80]
	ldr x26, [sp, #216]
	ldr x22, [x26, #960]
	ldr x8, [x22, #120]
	cbz x8, LBB568_498
	mov x23, x24
LBB568_490:
	ldr x25, [sp, #144]
	ldr x24, [sp, #176]
	ldr x19, [x22, #56]
	cmp x19, #255
	b.ls LBB568_504
LBB568_491:
	ldr x1, [x22, #16]
	cbz x1, LBB568_493
	ldr x0, [x22, #24]
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_493:
	ldr x8, [x22, #40]
	mov x28, x27
	cbz x8, LBB568_495
	ldr x0, [x22, #48]
	lsl x1, x8, #5
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_495:
	ldr x8, [x22, #64]
	cbz x8, LBB568_497
	ldr x0, [x22, #72]
	lsl x1, x8, #4
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_497:
	mov x0, x22
	mov w1, #152
	mov w2, #8
	bl __rustc::__rust_dealloc
	str xzr, [x26, #960]
	ldr x8, [x26, #664]
	add x8, x8, #1
	str x8, [x26, #664]
	add x0, x26, #536
Lloh3885:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.961@PAGE
Lloh3886:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.961@PAGEOFF
	mov w2, #14
	bl luna_core::vm::jit_state::JitCounters::bump_close_cause
	b LBB568_44
LBB568_498:
	ldr x8, [x22, #80]
	mov x23, x24
	cbz x8, LBB568_490
	mov x9, #0
	ldr x10, [x22, #72]
	ldr x24, [sp, #176]
LBB568_500:
	ldr x11, [x10], #16
	cmp x11, x25
	cinc x9, x9, eq
	subs x8, x8, #1
	b.ne LBB568_500
	cmp x9, #3
	b.lo LBB568_801
	str x25, [x22, #120]
	str w20, [x22, #128]
	mov w8, #1
	strb w8, [x22, #132]
	add x0, x26, #536
Lloh3887:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.960@PAGE
Lloh3888:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.960@PAGEOFF
	mov w2, #15
	mov x19, x12
	mov x20, x13
	bl luna_core::vm::jit_state::JitCounters::bump_close_cause
	mov x13, x20
	mov x12, x19
	ldr x22, [x26, #960]
	ldr x25, [sp, #144]
LBB568_503:
	ldr w9, [sp, #160]
	ldr x19, [x22, #56]
	cmp x19, #255
	b.hi LBB568_491
LBB568_504:
	ldr x8, [x22, #40]
	cmp x19, x8
	b.eq LBB568_948
LBB568_505:
	ldr x8, [x22, #48]
	add x8, x8, x19, lsl #5
	stp w12, w9, [x8]
	str x13, [x8, #8]
	str w28, [x8, #16]
	mov x28, x27
	str w28, [x8, #20]
	strb w21, [x8, #24]
	add x8, x19, #1
	str x8, [x22, #56]
	b LBB568_44
LBB568_506:
	ldr x8, [x26, #808]
	add x8, x8, #1
	str x8, [x26, #808]
	mov w8, #32
	str w8, [x26, #984]
	mov w8, #1
	strb w8, [x26, #991]
	ldr x8, [sp, #896]
	ldr x9, [x26, #336]
	cmp x9, x8
	b.hi LBB568_167
	b LBB568_168
LBB568_507:
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w4, w9, w8, uxtb
	add x0, sp, #1984
	add x2, sp, #1056
	add x3, sp, #1072
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_index
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
LBB568_508:
	ldur w19, [x13, #-4]
	str x20, [x21, #160]
	str w19, [sp, #932]
	ldr x8, [x23, #208]
	cmp x8, x19
	b.ls LBB568_516
	ldr x20, [x23, #200]
Lloh3889:
	adrp x8, __MergedGlobals@PAGE+16
Lloh3890:
	add x8, x8, __MergedGlobals@PAGEOFF+16
	ldapr x8, [x8]
	cbnz x8, LBB568_1246
LBB568_510:
	adrp x8, __MergedGlobals@PAGE+24
	ldrb w8, [x8, __MergedGlobals@PAGEOFF+24]
	tbz w8, #0, LBB568_512
	ldr x8, [x20, x19, lsl #3]
	add x9, x8, #236
	add x10, x8, #232
	add x11, x8, #240
	add x12, x8, #279
	add x13, sp, #560
	str x13, [sp, #1984]
Lloh3891:
	adrp x13, <u32 as core::fmt::LowerHex>::fmt@GOTPAGE
Lloh3892:
	ldr x13, [x13, <u32 as core::fmt::LowerHex>::fmt@GOTPAGEOFF]
	str x13, [sp, #1992]
	add x8, x8, #280
	add x13, sp, #920
	str x13, [sp, #2000]
Lloh3893:
	adrp x13, <u64 as core::fmt::LowerHex>::fmt@GOTPAGE
Lloh3894:
	ldr x13, [x13, <u64 as core::fmt::LowerHex>::fmt@GOTPAGEOFF]
	str x13, [sp, #2008]
	add x13, sp, #932
	str x13, [sp, #2016]
Lloh3895:
	adrp x13, <u32 as core::fmt::Display>::fmt@GOTPAGE
Lloh3896:
	ldr x13, [x13, <u32 as core::fmt::Display>::fmt@GOTPAGEOFF]
	str x13, [sp, #2024]
	str x9, [sp, #2032]
	str x13, [sp, #2040]
	str x10, [sp, #2048]
	str x13, [sp, #2056]
	str x11, [sp, #2064]
	str x13, [sp, #2072]
	add x9, sp, #284
	str x9, [sp, #2080]
	str x13, [sp, #2088]
	add x9, sp, #852
	str x9, [sp, #2096]
	str x13, [sp, #2104]
	str x12, [sp, #2112]
Lloh3897:
	adrp x9, <bool as core::fmt::Display>::fmt@GOTPAGE
Lloh3898:
	ldr x9, [x9, <bool as core::fmt::Display>::fmt@GOTPAGEOFF]
	str x9, [sp, #2120]
	str x8, [sp, #2128]
	str x9, [sp, #2136]
	add x1, sp, #1984
Lloh3899:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.950@PAGE
Lloh3900:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.950@PAGEOFF
	bl std::io::stdio::_eprint
LBB568_512:
	ldr x8, [x20, x19, lsl #3]
	ldr x9, [x8, #80]
	ldr x10, [x9]
	adds x10, x10, #1
	str x10, [x9]
	b.hs LBB568_1442
	ldp x25, x1, [x8, #80]
	ldr x9, [x8, #64]
	ldr x10, [x9]
	adds x10, x10, #1
	str x10, [x9]
	b.hs LBB568_1442
	ldp x9, x21, [x8, #64]
	str x9, [sp, #208]
	ldr x9, [x8, #32]
	ldr x10, [x9]
	adds x10, x10, #1
	str x10, [x9]
	b.hs LBB568_1442
	ldp x9, x19, [x8, #32]
	str x9, [sp, #200]
	ldr x9, [x8, #96]
	ldr x10, [x9]
	adds x10, x10, #1
	str x10, [x9]
	b.lo LBB568_524
	b LBB568_1442
LBB568_516:
Lloh3901:
	adrp x8, __MergedGlobals@PAGE+16
Lloh3902:
	add x8, x8, __MergedGlobals@PAGEOFF+16
	ldapr x8, [x8]
	cbnz x8, LBB568_1243
LBB568_517:
	adrp x8, __MergedGlobals@PAGE+24
	ldrb w8, [x8, __MergedGlobals@PAGEOFF+24]
	tbz w8, #0, LBB568_519
	add x8, sp, #560
	str x8, [sp, #1984]
Lloh3903:
	adrp x8, <u32 as core::fmt::LowerHex>::fmt@GOTPAGE
Lloh3904:
	ldr x8, [x8, <u32 as core::fmt::LowerHex>::fmt@GOTPAGEOFF]
	str x8, [sp, #1992]
	add x8, sp, #920
	str x8, [sp, #2000]
Lloh3905:
	adrp x8, <u64 as core::fmt::LowerHex>::fmt@GOTPAGE
Lloh3906:
	ldr x8, [x8, <u64 as core::fmt::LowerHex>::fmt@GOTPAGEOFF]
	str x8, [sp, #2008]
	add x1, sp, #1984
Lloh3907:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.951@PAGE
Lloh3908:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.951@PAGEOFF
	bl std::io::stdio::_eprint
LBB568_519:
	ldr x8, [x27, #80]
	ldr x9, [x8]
	adds x9, x9, #1
	str x9, [x8]
	b.hs LBB568_1442
	ldp x25, x1, [x27, #80]
	ldr x8, [x27, #64]
	ldr x9, [x8]
	adds x9, x9, #1
	str x9, [x8]
	b.hs LBB568_1442
	ldp x8, x21, [x27, #64]
	str x8, [sp, #208]
	ldr x8, [x27, #32]
	ldr x9, [x8]
	adds x9, x9, #1
	str x9, [x8]
	b.hs LBB568_1442
	ldp x8, x19, [x27, #32]
	str x8, [sp, #200]
	ldr x8, [x27, #96]
	ldr x9, [x8]
	adds x9, x9, #1
	str x9, [x8]
	b.hs LBB568_1442
	mov x8, x27
LBB568_524:
	ldr x22, [sp, #920]
	ldp x12, x26, [x8, #96]
	ldr x8, [x23, #184]
	sub x8, x8, #1
	str x8, [x23, #184]
LBB568_525:
	stp x25, x1, [x29, #-160]
	ldp x10, x9, [sp, #200]
	stp x9, x21, [x29, #-144]
	stp x10, x19, [x29, #-128]
	lsr x8, x22, #32
	stp x12, x26, [x29, #-112]
	str x12, [sp, #160]
	cbz x8, LBB568_531
	sub w23, w8, #1
	cmp x23, x1
	b.hs LBB568_1373
	add x9, x25, #16
	mov w10, #48
	umaddl x9, w23, w10, x9
	ldp x10, x19, [x9]
	add x28, x10, #16
	mov w24, #1
	str w8, [sp, #936]
	str w22, [sp, #940]
	cmp x23, x26
	b.hs LBB568_538
LBB568_528:
	add x8, x12, #16
	ldr w9, [x8, x23, lsl #2]
	cmn w9, #1
	ldr x11, [sp, #216]
	b.eq LBB568_535
	add w10, w9, #1
	str w10, [x8, x23, lsl #2]
	cmp w9, #1
	b.ne LBB568_535
	mov x21, x1
	ldr x8, [x11, #960]
	cmp x8, #0
	cset w8, eq
	ldrb w9, [x11, #989]
	and w8, w8, w9
	str w8, [sp, #152]
	ldrb w20, [sp, #919]
	ldr x26, [x11, #312]
	ldr x8, [sp, #224]
	add x8, x19, x8
	subs x22, x8, x26
	b.ls LBB568_536
	b LBB568_539
LBB568_531:
	add x28, x10, #16
	add x9, x9, #16
	sub x23, x1, #1
	add x10, x21, x21, lsl #1
	lsl x10, x10, #3
LBB568_532:
	cbz x10, LBB568_537
	ldr w11, [x9], #24
	add x23, x23, #1
	sub x10, x10, #24
	cmp w11, w22
	b.ne LBB568_532
	ldp x10, x19, [x9, #-16]
	add x28, x10, #16
	mov w24, #1
	str w8, [sp, #936]
	str w22, [sp, #940]
	cmp x23, x26
	b.hs LBB568_538
	b LBB568_528
LBB568_535:
	mov x21, x1
	str wzr, [sp, #152]
	ldrb w20, [sp, #919]
	ldr x26, [x11, #312]
	ldr x8, [sp, #224]
	add x8, x19, x8
	subs x22, x8, x26
	b.hi LBB568_539
LBB568_536:
	str x25, [sp, #176]
	orr w8, w24, w20
	ldr x26, [sp, #216]
	tbz w8, #0, LBB568_545
	b LBB568_547
LBB568_537:
	mov w24, #0
	add x23, x1, x21
	str w8, [sp, #936]
	str w22, [sp, #940]
	cmp x23, x26
	b.lo LBB568_528
LBB568_538:
	mov x21, x1
	str wzr, [sp, #152]
	ldp x11, x8, [sp, #216]
	ldrb w20, [sp, #919]
	ldr x26, [x11, #312]
	add x8, x19, x8
	subs x22, x8, x26
	b.ls LBB568_536
LBB568_539:
	ldr x8, [sp, #216]
	ldr x8, [x8, #296]
	sub x8, x8, x26
	cmp x22, x8
	b.hi LBB568_1211
	mov x8, x26
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x26, lsl #4
	cmp x22, #2
	b.lo LBB568_544
LBB568_541:
	mvn x10, x26
	ldr x11, [sp, #224]
	add x11, x19, x11
	add x10, x10, x11
LBB568_542:
	strb wzr, [x9], #16
	subs x10, x10, #1
	b.ne LBB568_542
	add x8, x22, x8
	sub x8, x8, #1
LBB568_544:
	ldr x10, [sp, #216]
	strb wzr, [x9]
	add x8, x8, #1
	str x8, [x10, #312]
	str x25, [sp, #176]
	orr w8, w24, w20
	ldr x26, [sp, #216]
	tbnz w8, #0, LBB568_547
LBB568_545:
	ldr w8, [sp, #124]
	cbz w8, LBB568_566
	cmp w8, #2
	b.ne LBB568_561
LBB568_547:
	cbz x19, LBB568_566
	mov x22, #0
	ldr x24, [sp, #880]
	ldr x20, [sp, #872]
	ldr x8, [sp, #224]
	lsl x25, x8, #4
LBB568_549:
	ldrb w8, [x28, x22]
	cmp w8, #2
	b.le LBB568_551
	cmp w8, #5
	mov w9, #5
	csel w9, wzr, w9, eq
	cmp w8, #3
	mov w10, #6
	mov w11, #7
	csel w10, w10, w11, eq
	cmp w8, #4
	csel w0, w9, w10, gt
	b LBB568_553
LBB568_551:
	cbz w8, LBB568_557
	cmp w8, #1
	mov w8, #4
	mov w9, #3
	csel w0, w9, w8, eq
LBB568_553:
	cmp x24, x22
	b.eq LBB568_1350
	ldr x1, [x20, x22, lsl #3]
	add x8, sp, #1984
	bl luna_core::runtime::value::Value::pack
	ldr x8, [x26, #312]
	ldr x9, [sp, #224]
	add x9, x9, x22
	cmp x9, x8
	b.hs LBB568_1349
	add x22, x22, #1
	ldr x8, [sp, #232]
	ldr x8, [x8]
	ldr q0, [sp, #1984]
	str q0, [x8, x25]
	add x25, x25, #16
	cmp x19, x22
	b.ne LBB568_549
	b LBB568_566
LBB568_557:
	ldr x8, [sp, #856]
	cmp x22, x8
	b.hs LBB568_560
	ldr x8, [sp, #304]
	cmp x22, x8
	b.hs LBB568_1368
	ldr x8, [sp, #296]
	ldrb w0, [x8, x22]
	b LBB568_553
LBB568_560:
	mov w0, #0
	b LBB568_553
LBB568_561:
	cbz x19, LBB568_566
	mov x11, #0
	ldr x22, [sp, #880]
	ldr x8, [sp, #872]
	ldr x10, [sp, #224]
	lsl x9, x10, #4
LBB568_563:
	cmp x22, x11
	b.eq LBB568_1362
	ldr x12, [x26, #312]
	cmp x10, x12
	b.hs LBB568_1363
	ldr x12, [x8, x11, lsl #3]
	add x11, x11, #1
	ldr x13, [sp, #232]
	ldr x13, [x13]
	add x13, x13, x9
	mov w14, #2
	strb w14, [x13]
	str x12, [x13, #8]
	add x10, x10, #1
	add x9, x9, #16
	cmp x19, x11
	b.ne LBB568_563
LBB568_566:
	ldr w8, [sp, #936]
	ldr x25, [sp, #176]
	cbz w8, LBB568_572
	sub w22, w8, #1
	cmp x21, x22
	b.ls LBB568_1374
	ldr x8, [sp, #896]
	cbz x8, LBB568_572
	sub x8, x8, #1
	ldr x1, [x26, #336]
	cmp x8, x1
	b.hs LBB568_1379
	ldr x9, [x26, #328]
	add x8, x9, x8, lsl #6
	ldr w9, [x8]
	tbnz w9, #0, LBB568_572
	mov w9, #48
	umaddl x9, w22, w9, x25
	ldr w9, [x9, #60]
	str w9, [x8, #36]
LBB568_572:
	ldp x9, x8, [x26, #328]
	str x8, [sp, #1744]
	add x19, x9, x8, lsl #6
	mov x9, x19
	ldr w10, [x9, #-64]!
	cmp x8, #0
	csel x20, xzr, x9, eq
	tbnz w10, #0, LBB568_1322
Lloh3909:
	adrp x8, __MergedGlobals@PAGE+16
Lloh3910:
	add x8, x8, __MergedGlobals@PAGEOFF+16
	ldapr x8, [x8]
	cbnz x8, LBB568_1200
LBB568_574:
	adrp x8, __MergedGlobals@PAGE+24
	ldrb w8, [x8, __MergedGlobals@PAGEOFF+24]
	tbz w8, #0, LBB568_576
	add x8, x20, #36
	add x9, sp, #919
	str x9, [sp, #1984]
Lloh3911:
	adrp x9, <bool as core::fmt::Display>::fmt@GOTPAGE
Lloh3912:
	ldr x9, [x9, <bool as core::fmt::Display>::fmt@GOTPAGEOFF]
	str x9, [sp, #1992]
	add x9, sp, #904
	str x9, [sp, #2000]
Lloh3913:
	adrp x9, <u64 as core::fmt::LowerHex>::fmt@GOTPAGE
Lloh3914:
	ldr x9, [x9, <u64 as core::fmt::LowerHex>::fmt@GOTPAGEOFF]
	str x9, [sp, #2008]
	str x8, [sp, #2016]
Lloh3915:
	adrp x8, <u32 as core::fmt::Display>::fmt@GOTPAGE
Lloh3916:
	ldr x8, [x8, <u32 as core::fmt::Display>::fmt@GOTPAGEOFF]
	str x8, [sp, #2024]
	add x9, sp, #940
	str x9, [sp, #2032]
	str x8, [sp, #2040]
	add x9, sp, #936
	str x9, [sp, #2048]
	str x8, [sp, #2056]
	add x8, sp, #1744
	str x8, [sp, #2064]
Lloh3917:
	adrp x8, <usize as core::fmt::Display>::fmt@GOTPAGE
Lloh3918:
	ldr x8, [x8, <usize as core::fmt::Display>::fmt@GOTPAGEOFF]
	str x8, [sp, #2072]
	add x9, sp, #896
	str x9, [sp, #2080]
	str x8, [sp, #2088]
	add x9, sp, #856
	str x9, [sp, #2096]
	str x8, [sp, #2104]
	add x1, sp, #1984
Lloh3919:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.952@PAGE
Lloh3920:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.952@PAGEOFF
	bl std::io::stdio::_eprint
LBB568_576:
	ldr w8, [sp, #940]
	stur w8, [x19, #-28]
	ldr x26, [sp, #216]
	ldr w8, [sp, #152]
	tbz w8, #0, LBB568_620
	ldr x8, [x26, #336]
	cbz x8, LBB568_582
	ldr x9, [x26, #328]
	add x9, x9, x8, lsl #6
	ldur w10, [x9, #-64]
	ldr x8, [sp, #168]
	tbnz w10, #0, LBB568_580
	ldur w8, [x9, #-32]
	str x8, [sp, #224]
	ldur x8, [x9, #-40]
LBB568_580:
	ldr x19, [x8, #16]
	ldrb w22, [x19, #84]
	cbz x22, LBB568_583
LBB568_581:
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov x0, x22
	mov w1, #1
	bl __rustc::__rust_alloc
	cbnz x0, LBB568_584
	b LBB568_1381
LBB568_582:
	ldr x8, [sp, #168]
	ldr x19, [x8, #16]
	ldrb w22, [x19, #84]
	cbnz x22, LBB568_581
LBB568_583:
	mov w0, #1
LBB568_584:
	ldr x8, [sp, #224]
	str x22, [sp, #560]
	str x0, [sp, #568]
	str xzr, [sp, #576]
	ldr x26, [x26, #312]
	add x8, x8, x22
	subs x24, x8, x26
	b.ls LBB568_591
	ldr x8, [sp, #216]
	ldr x8, [x8, #296]
	sub x8, x8, x26
	cmp x24, x8
	b.hi LBB568_1240
	mov x8, x26
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x26, lsl #4
	cmp x24, #2
	b.lo LBB568_590
LBB568_587:
	mvn x10, x26
	ldr x11, [sp, #224]
	add x11, x11, x22
	add x10, x10, x11
LBB568_588:
	strb wzr, [x9], #16
	subs x10, x10, #1
	b.ne LBB568_588
	add x8, x24, x8
	sub x8, x8, #1
LBB568_590:
	ldr x10, [sp, #216]
	strb wzr, [x9]
	add x8, x8, #1
	str x8, [x10, #312]
LBB568_591:
	cbz w22, LBB568_608
	ldp x25, x8, [sp, #216]
	lsl x21, x8, #4
	ldr x26, [sp, #232]
	b LBB568_594
LBB568_593:
	ldr x8, [sp, #568]
	strb w20, [x8, x24]
	add x20, x24, #1
	str x20, [sp, #576]
	add x8, x28, #1
	add x21, x21, #16
	subs x22, x22, #1
	b.eq LBB568_609
LBB568_594:
	ldr x1, [x25, #312]
	mov x28, x8
	cmp x8, x1
	b.hs LBB568_1352
	ldr x8, [x26]
	ldrb w20, [x8, x21]
Lloh3921:
	adrp x11, LJTI568_1@PAGE
Lloh3922:
	add x11, x11, LJTI568_1@PAGEOFF
	adr x9, LBB568_596
	ldrb w10, [x11, x20]
	add x9, x9, x10, lsl #2
	br x9
LBB568_596:
	add x8, x8, x21
	ldr w8, [x8, #8]
	tst w8, #0x1
	mov w8, #1
	cinc w20, w8, ne
	ldr x24, [sp, #576]
	ldr x8, [sp, #560]
	cmp x24, x8
	b.eq LBB568_606
	b LBB568_593
	mov w20, #5
	ldr x24, [sp, #576]
	ldr x8, [sp, #560]
	cmp x24, x8
	b.ne LBB568_593
	b LBB568_606
	mov w20, #10
	ldr x24, [sp, #576]
	ldr x8, [sp, #560]
	cmp x24, x8
	b.ne LBB568_593
	b LBB568_606
	mov w20, #3
	ldr x24, [sp, #576]
	ldr x8, [sp, #560]
	cmp x24, x8
	b.ne LBB568_593
	b LBB568_606
	mov w20, #4
	ldr x24, [sp, #576]
	ldr x8, [sp, #560]
	cmp x24, x8
	b.ne LBB568_593
	b LBB568_606
	mov w20, #8
	ldr x24, [sp, #576]
	ldr x8, [sp, #560]
	cmp x24, x8
	b.ne LBB568_593
	b LBB568_606
	mov w20, #6
	ldr x24, [sp, #576]
	ldr x8, [sp, #560]
	cmp x24, x8
	b.ne LBB568_593
	b LBB568_606
	mov w20, #7
	ldr x24, [sp, #576]
	ldr x8, [sp, #560]
	cmp x24, x8
	b.ne LBB568_593
	b LBB568_606
	mov w20, #11
	ldr x24, [sp, #576]
	ldr x8, [sp, #560]
	cmp x24, x8
	b.ne LBB568_593
LBB568_606:
	add x0, sp, #560
	bl <alloc::raw_vec::RawVec<u8>>::grow_one
	b LBB568_593
	mov w20, #9
	ldr x24, [sp, #576]
	ldr x8, [sp, #560]
	cmp x24, x8
	b.ne LBB568_593
	b LBB568_606
LBB568_608:
	ldr x20, [sp, #576]
LBB568_609:
	ldr x22, [sp, #560]
	ldr x24, [sp, #568]
	ldr x8, [sp, #168]
	ldr x21, [x8, #16]
	ldr w26, [sp, #940]
	ldr w25, [sp, #848]
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov w0, #8192
	mov w1, #8
	bl __rustc::__rust_alloc
	cbz x0, LBB568_1377
	str w26, [sp, #2120]
	str x24, [sp, #2008]
	str x20, [sp, #2016]
	mov w8, #256
	str x8, [sp, #2024]
	str x0, [sp, #2032]
	str xzr, [sp, #2048]
	str xzr, [sp, #2040]
	strh wzr, [sp, #2126]
	str x22, [sp, #2000]
	str xzr, [sp, #1984]
	strb wzr, [sp, #2124]
	str x19, [sp, #2072]
	str x21, [sp, #2080]
	str w25, [sp, #2088]
	mov w8, #2
	strb w8, [sp, #2128]
	mov w8, #8
	str x8, [sp, #2056]
	str xzr, [sp, #2064]
	str x23, [sp, #2096]
	str xzr, [sp, #2104]
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov w0, #152
	mov w1, #8
	bl __rustc::__rust_alloc
	cbz x0, LBB568_1324
	mov x22, x0
	ldr q0, [sp, #2080]
	ldr q1, [sp, #2096]
	stp q0, q1, [x0, #96]
	ldr q0, [sp, #2112]
	str q0, [x0, #128]
	ldr x8, [sp, #2128]
	str x8, [x0, #144]
	ldr q0, [sp, #2016]
	ldr q1, [sp, #2032]
	stp q0, q1, [x0, #32]
	ldr q0, [sp, #2048]
	ldr q1, [sp, #2064]
	stp q0, q1, [x0, #64]
	ldr q0, [sp, #1984]
	ldr q1, [sp, #2000]
	stp q0, q1, [x0]
	ldr x26, [sp, #216]
	ldr x23, [x26, #960]
	ldr x25, [sp, #176]
	cbz x23, LBB568_619
	ldr x1, [x23, #16]
	cbz x1, LBB568_614
	ldr x0, [x23, #24]
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_614:
	ldr x8, [x23, #40]
	cbz x8, LBB568_616
	ldr x0, [x23, #48]
	lsl x1, x8, #5
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_616:
	ldr x8, [x23, #64]
	cbz x8, LBB568_618
	ldr x0, [x23, #72]
	lsl x1, x8, #4
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_618:
	mov x0, x23
	mov w1, #152
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_619:
	str x22, [x26, #960]
	ldr x8, [x26, #336]
	sub x8, x8, #1
	str x8, [x26, #968]
	ldr x8, [x26, #712]
	add x8, x8, #1
	str x8, [x26, #712]
LBB568_620:
	ldr x8, [sp, #184]
	ldr x8, [x8]
	cbz x8, LBB568_622
	ldr x0, [x26, #832]
	lsl x1, x8, #3
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_622:
	ldr q0, [sp, #864]
	ldr x9, [sp, #184]
	str q0, [x9]
	ldr x8, [sp, #880]
	str x8, [x9, #16]
	ldr q0, [sp, #288]
	str q0, [sp, #1984]
	ldr x8, [sp, #304]
	str x8, [sp, #2000]
	ldp x8, x20, [sp, #192]
	ldr x1, [x8]
	ldr x19, [sp, #208]
	cbz x1, LBB568_624
	ldr x0, [x26, #880]
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_624:
	ldr q0, [sp, #1984]
	ldr x9, [sp, #192]
	str q0, [x9]
	ldr x8, [sp, #2000]
	str x8, [x9, #16]
	ldr x9, [sp, #160]
	ldr x8, [x9]
	subs x8, x8, #1
	str x8, [x9]
	b.eq LBB568_629
	ldr x8, [x20]
	subs x8, x8, #1
	str x8, [x20]
	b.eq LBB568_630
LBB568_626:
	ldr x8, [x19]
	subs x8, x8, #1
	str x8, [x19]
	b.eq LBB568_631
LBB568_627:
	ldr x8, [x25]
	subs x8, x8, #1
	str x8, [x25]
	b.eq LBB568_632
LBB568_628:
	ldr x8, [x27]
	subs x8, x8, #1
	str x8, [x27]
	b.ne LBB568_2
	b LBB568_1
LBB568_629:
	sub x0, x29, #112
	bl alloc::rc::Rc<T,A>::drop_slow
	ldr x8, [x20]
	subs x8, x8, #1
	str x8, [x20]
	b.ne LBB568_626
LBB568_630:
	sub x0, x29, #128
	bl alloc::rc::Rc<T,A>::drop_slow
	ldr x8, [x19]
	subs x8, x8, #1
	str x8, [x19]
	b.ne LBB568_627
LBB568_631:
	sub x0, x29, #144
	bl alloc::rc::Rc<T,A>::drop_slow
	ldr x8, [x25]
	subs x8, x8, #1
	str x8, [x25]
	b.ne LBB568_628
LBB568_632:
	sub x0, x29, #160
	bl alloc::rc::Rc<T,A>::drop_slow
	ldr x8, [x27]
	subs x8, x8, #1
	str x8, [x27]
	b.ne LBB568_2
	b LBB568_1
LBB568_633:
	sub x8, x11, #1
	str x8, [x26, #336]
	add w22, w23, #4
	ldr x25, [x26, #312]
	subs x24, x22, x25
	b.ls LBB568_640
	ldr x8, [x26, #296]
	sub x8, x8, x25
	cmp x24, x8
	b.hi LBB568_1247
	mov x8, x25
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x25, lsl #4
	cmp x24, #2
	b.lo LBB568_639
LBB568_636:
	mvn x10, x25
	add x10, x10, x22
LBB568_637:
	strb wzr, [x9], #16
	subs x10, x10, #1
	b.ne LBB568_637
	add x8, x24, x8
	sub x8, x8, #1
LBB568_639:
	strb wzr, [x9]
	add x8, x8, #1
	str x8, [x26, #312]
LBB568_640:
	ldr w0, [x26, #1792]
	cmp w0, w22
	b.hs LBB568_644
	lsl x8, x0, #4
LBB568_642:
	ldr x1, [x26, #312]
	cmp x1, x0
	b.ls LBB568_1372
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x0, x0, #1
	strb wzr, [x9, x8]
	add x8, x8, #16
	cmp x22, x0
	b.ne LBB568_642
LBB568_644:
	str w22, [x26, #1792]
	ldr x8, [x26, #336]
	ldr x9, [sp, #96]
	cmp x8, x9
	b.lo LBB568_1313
	tbnz w19, #31, LBB568_2
	ldr x24, [x26, #312]
	add w21, w19, w23
	subs x23, x21, x24
	b.ls LBB568_653
	ldr x8, [x26, #296]
	sub x8, x8, x24
	cmp x23, x8
	b.hi LBB568_1250
	mov x8, x24
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x24, lsl #4
	cmp x23, #2
	b.lo LBB568_652
LBB568_649:
	mvn x10, x24
	add x10, x10, x21
LBB568_650:
	strb wzr, [x9], #16
	subs x10, x10, #1
	b.ne LBB568_650
	add x8, x23, x8
	sub x8, x8, #1
LBB568_652:
	strb wzr, [x9]
	add x24, x8, #1
	str x24, [x26, #312]
LBB568_653:
	cmp w19, #5
	b.lo LBB568_657
	ldr x8, [sp, #232]
	ldr x8, [x8]
	sub x9, x19, #4
LBB568_655:
	mov w0, w22
	cmp x24, x0
	b.ls LBB568_1370
	lsl x10, x0, #4
	strb wzr, [x8, x10]
	add w22, w0, #1
	subs x9, x9, #1
	b.ne LBB568_655
LBB568_657:
	str w21, [x26, #1792]
	b LBB568_2
LBB568_658:
	sub x9, x11, #1
	str x9, [x26, #336]
	ldr w11, [x26, #1792]
	cmp w11, w23
	b.ls LBB568_830
	ldr x1, [x26, #312]
	cmp x1, x23
	b.ls LBB568_1414
	ldr x11, [sp, #232]
	ldr x11, [x11]
	add x15, x11, x23, lsl #4
	ldrb w11, [x15]
	ldur w16, [x15, #1]
	str w16, [sp, #248]
	ldr w16, [x15, #4]
	stur w16, [sp, #251]
	ldrb w16, [x15, #8]
	str w16, [sp, #12]
	ldur w16, [x15, #9]
	str w16, [sp, #240]
	ldr w15, [x15, #12]
	stur w15, [sp, #243]
	ldr x1, [x26, #312]
	cmp x1, x23
	b.hs LBB568_831
	b LBB568_832
LBB568_661:
	sub x8, x11, #1
	str x8, [x26, #336]
	ldr w8, [x26, #1800]
	sub w8, w8, #1
	str w8, [x26, #1800]
	ldr x1, [x26, #312]
	cmp x1, x23
	b.ls LBB568_1409
	ldr x8, [x26, #304]
	ldr w22, [x26, #1792]
	add x8, x8, x23, lsl #4
	mov w9, #1
	strb w9, [x8]
	strb w9, [x8, #8]
	str w22, [x26, #1792]
	ldr x8, [x26, #336]
	ldr x9, [sp, #96]
	cmp x8, x9
	b.lo LBB568_1309
	tbnz w19, #31, LBB568_2
	ldr x24, [x26, #312]
	add w21, w19, w23
	subs x25, x21, x24
	b.ls LBB568_671
	ldr x8, [x26, #296]
	sub x8, x8, x24
	cmp x25, x8
	b.hi LBB568_1249
	mov x8, x24
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x24, lsl #4
	cmp x25, #2
	b.lo LBB568_670
LBB568_667:
	mvn x10, x24
	add x10, x10, x21
LBB568_668:
	strb wzr, [x9], #16
	subs x10, x10, #1
	b.ne LBB568_668
	add x8, x25, x8
	sub x8, x8, #1
LBB568_670:
	strb wzr, [x9]
	add x24, x8, #1
	str x24, [x26, #312]
LBB568_671:
	sub w9, w22, w23
	cmp w9, w19
	b.hs LBB568_675
	ldr x8, [sp, #232]
	ldr x8, [x8]
	sub x9, x19, w9, uxtw
LBB568_673:
	mov w0, w22
	cmp x24, x0
	b.ls LBB568_1370
	lsl x10, x0, #4
	strb wzr, [x8, x10]
	add w22, w0, #1
	subs x9, x9, #1
	b.ne LBB568_673
LBB568_675:
	str w21, [x26, #1792]
	b LBB568_2
LBB568_676:
	cmp x11, #2
	b.ls LBB568_15
	cmp x21, #1
	b.ls LBB568_679
	ldr x8, [x10, #120]
	cbz x8, LBB568_1075
LBB568_679:
	mov w8, #1
	strb w8, [x10, #144]
	b LBB568_16
LBB568_680:
	mov w8, #11
	strb w8, [x26, #896]
	ldr x8, [x26, #704]
	add x8, x8, #1
	str x8, [x26, #704]
	ldr x8, [sp, #896]
	ldr x9, [x26, #336]
	cmp x9, x8
	b.ls LBB568_682
	str x8, [x26, #336]
LBB568_682:
	cbz w19, LBB568_684
	ldr x9, [sp, #216]
	ldr x8, [x9, #808]
	add x8, x8, #1
	str x8, [x9, #808]
	mov w8, #1
	strb w8, [x9, #991]
LBB568_684:
	ldr x8, [sp, #184]
	ldr x8, [x8]
	ldr x26, [sp, #216]
	cbz x8, LBB568_686
	ldr x0, [x26, #832]
	lsl x1, x8, #3
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_686:
	ldr q0, [sp, #864]
	ldp x24, x9, [sp, #176]
	str q0, [x9]
	ldr x8, [sp, #880]
	str x8, [x9, #16]
	ldr q0, [sp, #288]
	str q0, [sp, #1984]
	ldr x8, [sp, #304]
	str x8, [sp, #2000]
	ldr x8, [sp, #192]
	ldr x1, [x8]
	ldr x28, [sp, #128]
	ldr x23, [sp, #168]
	ldr x25, [sp, #144]
	cbz x1, LBB568_688
	ldr x0, [x26, #880]
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_688:
	ldr q0, [sp, #1984]
	ldr x9, [sp, #192]
	str q0, [x9]
	ldr x8, [sp, #2000]
	str x8, [x9, #16]
	ldr x8, [x27]
	subs x8, x8, #1
	str x8, [x27]
	b.ne LBB568_85
	add x0, sp, #840
	bl alloc::rc::Rc<T,A>::drop_slow
	b LBB568_85
LBB568_690:
	add x12, x1, x10
	cmp x0, x12
	b.hs LBB568_861
	cmp x11, x1
	b.hs LBB568_1416
	ldr x12, [x27, #64]
	mov w13, #24
	madd x11, x11, x13, x12
	add x13, x11, #24
	add x11, x11, #32
	ldr x11, [x11]
	ldr x12, [sp, #600]
	cmp x12, x11
	b.hs LBB568_863
	b LBB568_862
LBB568_693:
	ldr q0, [x8, w9, uxtw #4]
	stur q0, [x29, #-112]
	ldr q0, [x8, w10, uxtw #4]
	str q0, [sp, #288]
	add x0, sp, #1984
	sub x2, x29, #112
	add x3, sp, #288
	mov x1, x26
	mov w4, #1
	bl luna_core::vm::exec::Vm::less_step
	ldrb w8, [sp, #1984]
	cmp w8, #14
	b.eq LBB568_1278
	ldr x11, [sp, #72]
	ldur x9, [x11, #15]
	add x10, sp, #560
	stur x9, [x10, #15]
	ldr q0, [x11]
	str q0, [sp, #560]
	ldr x9, [sp, #2008]
	strb w8, [sp, #1664]
	ldr q0, [sp, #560]
	ldr x11, [sp, #48]
	str q0, [x11]
	ldur x8, [x10, #15]
	stur x8, [x11, #15]
	str x9, [sp, #1688]
	ubfx w5, w28, #15, #1
Lloh3923:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.747@PAGE
Lloh3924:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.747@PAGEOFF
	add x0, sp, #1984
	add x2, sp, #1664
	add x3, sp, #1632
	add x4, sp, #1648
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_compare
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
LBB568_695:
	cmp w8, #11
	b.ne LBB568_909
	strb w19, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #6
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w19, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x21, [sp, #1992]
	strb w22, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x23, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3925:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.737@PAGE
Lloh3926:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.737@PAGEOFF
	b LBB568_722
LBB568_698:
	cmp w8, #11
	b.ne LBB568_924
	strb w19, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #4
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w19, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x21, [sp, #1992]
	strb w22, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x23, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3927:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.735@PAGE
Lloh3928:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.735@PAGEOFF
	b LBB568_1066
LBB568_701:
	cmp w8, #11
	b.ne LBB568_925
	strb w19, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #3
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w19, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x21, [sp, #1992]
	strb w22, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x23, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3929:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.734@PAGE
Lloh3930:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.734@PAGEOFF
	b LBB568_1066
LBB568_704:
	cmp w8, #11
	b.ne LBB568_926
	strb w19, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #10
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w19, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x21, [sp, #1992]
	strb w22, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x23, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3931:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.741@PAGE
Lloh3932:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.741@PAGEOFF
	b LBB568_1066
LBB568_707:
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #560]
	ldr q0, [x8, w10, uxtw #4]
	str q0, [sp, #1984]
	add x0, sp, #1472
	add x2, sp, #560
	add x3, sp, #1984
	mov x1, x26
	bl luna_core::vm::exec::Vm::eq_step
	ubfx w5, w28, #15, #1
Lloh3933:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.748@PAGE
Lloh3934:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.748@PAGEOFF
	add x0, sp, #1984
	add x2, sp, #1472
	add x3, sp, #1440
	add x4, sp, #1456
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_compare
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
LBB568_708:
	ldr x9, [x23, #16]
	lsr x0, x28, #24
	ldr x1, [x9, #40]
	cmp x1, x0
	b.ls LBB568_1410
	ldr x9, [x9, #32]
	add x9, x9, x0, lsl #4
	ldr x12, [sp, #224]
LBB568_710:
	ldr q0, [x9]
	str q0, [sp, #1312]
	add x0, sp, #1984
	add x2, sp, #1296
	add x3, sp, #1312
	add w4, w8, w12
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_index
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
LBB568_711:
	cmp w8, #11
	b.ne LBB568_930
	strb w19, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #9
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w19, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x21, [sp, #1992]
	strb w22, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x23, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3935:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.740@PAGE
Lloh3936:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.740@PAGEOFF
	b LBB568_722
LBB568_714:
	cmp w8, #11
	b.ne LBB568_931
	strb w19, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #11
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w19, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x21, [sp, #1992]
	strb w22, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x23, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3937:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.742@PAGE
Lloh3938:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.742@PAGEOFF
	b LBB568_1066
LBB568_717:
	ldr q0, [x8, w9, uxtw #4]
	stur q0, [x29, #-112]
	ldr q0, [x8, w10, uxtw #4]
	str q0, [sp, #288]
	add x0, sp, #1984
	sub x2, x29, #112
	add x3, sp, #288
	mov x1, x26
	mov w4, #0
	bl luna_core::vm::exec::Vm::less_step
	ldrb w8, [sp, #1984]
	cmp w8, #14
	b.eq LBB568_1278
	ldr x11, [sp, #72]
	ldur x9, [x11, #15]
	add x10, sp, #560
	stur x9, [x10, #15]
	ldr q0, [x11]
	str q0, [sp, #560]
	ldr x9, [sp, #2008]
	strb w8, [sp, #1600]
	ldr q0, [sp, #560]
	ldr x11, [sp, #40]
	str q0, [x11]
	ldur x8, [x10, #15]
	stur x8, [x11, #15]
	str x9, [sp, #1624]
	ubfx w5, w28, #15, #1
Lloh3939:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.746@PAGE
Lloh3940:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.746@PAGEOFF
	add x0, sp, #1984
	add x2, sp, #1600
	add x3, sp, #1568
	add x4, sp, #1584
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_compare
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
LBB568_719:
	cmp w8, #11
	b.ne LBB568_934
	strb w19, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #7
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w19, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x21, [sp, #1992]
	strb w22, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x23, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3941:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.738@PAGE
Lloh3942:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.738@PAGEOFF
LBB568_722:
	add x0, sp, #560
	sub x2, x29, #112
	add x3, sp, #1984
	add x5, sp, #288
	mov x1, x26
	mov w4, #2
	mov w7, #4
	bl luna_core::vm::exec::Vm::begin_meta_call
	ldrb w8, [sp, #560]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1268
LBB568_723:
	cmp w8, #11
	b.ne LBB568_935
	strb w19, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x21, [sp, #296]
	strb w22, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x23, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #8
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w19, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x21, [sp, #1992]
	strb w22, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x23, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3943:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.739@PAGE
Lloh3944:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.739@PAGEOFF
	b LBB568_1066
LBB568_726:
	ldr x8, [x26, #336]
	cbz x8, LBB568_1333
	ldr x9, [x26, #328]
	add x8, x9, x8, lsl #6
	ldur x9, [x8, #-64]
	cmp x9, #1
	b.eq LBB568_1333
	ldr x9, [x23, #16]
	ldur w0, [x8, #-28]
	ldr x1, [x9, #24]
	cmp x1, x0
	b.ls LBB568_1411
	ldr x9, [x9, #16]
	ldr w9, [x9, x0, lsl #2]
	add w10, w0, #1
	stur w10, [x8, #-28]
	lsr w8, w9, #7
LBB568_730:
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, w19, uxtw #4
	ldrb w10, [x9]
	cmp w10, #5
	b.ne LBB568_1341
	mov w23, #0
	mov w20, w8
	ldr x22, [x9, #8]
	mov w8, #1
LBB568_732:
	tbnz w23, #0, LBB568_735
	cmp w8, w21
	b.hi LBB568_735
	cset w23, hs
	ldr x9, [x26, #304]
	add w10, w8, w19
	ldr q0, [x9, w10, uxtw #4]
	cinc w24, w8, lo
	str q0, [sp, #1280]
	add x8, x20, w8, uxtw
	str x8, [sp, #1992]
	mov w8, #2
	strb w8, [sp, #1984]
	add x2, sp, #1984
	add x3, sp, #1280
	mov x0, x22
	mov x1, x26
	bl luna_core::runtime::table::Table::set_norm
	and w9, w0, #0xff
	mov x8, x24
	cmp w9, #3
	b.ne LBB568_732
	b LBB568_1256
LBB568_735:
	ldrb w8, [x22, #9]
	tbz w8, #2, LBB568_738
	and w8, w8, #0xf8
	strb w8, [x22, #9]
	ldr x20, [x26, #64]
	ldr x8, [x26, #48]
	cmp x20, x8
	b.eq LBB568_1193
LBB568_737:
	ldr x8, [x26, #56]
	str x22, [x8, x20, lsl #3]
	add x8, x20, #1
	str x8, [x26, #64]
LBB568_738:
	ldrb w8, [x26, #1832]
	tbnz w8, #0, LBB568_2
	ldrb w8, [x26, #268]
	tbnz w8, #0, LBB568_2
	ldp x8, x9, [x26, #240]
	cmp x8, x9
	b.lo LBB568_2
	add w8, w19, #1
	str w8, [x26, #1816]
	ldr x8, [x26, #1640]
	cmp x8, #1
	csinc x8, x8, xzr, gt
	mov w9, #400
	udiv x8, x9, x8
	cmp x8, #1
	csinc x8, x8, xzr, hi
	ldr x9, [x26, #232]
	udiv x8, x9, x8
	mov w9, #64000
	cmp x8, x9
	csel x1, x8, x9, hi
	mov x0, x26
	bl luna_core::vm::exec::Vm::gc_step
	cbz w0, LBB568_2
	ldr x8, [x26, #1632]
	bic x8, x8, x8, asr #63
	ldr x9, [x26, #240]
	umulh x10, x9, x8
	cbz x10, LBB568_429
LBB568_743:
	mov x8, #-1
	b LBB568_430
LBB568_744:
	fmov d0, x19
	fmov d1, x22
	fdiv d0, d0, d1
	lsr w9, w28, #7
	add w9, w13, w9, uxtb
	b LBB568_1032
LBB568_745:
	add x19, sp, #1744
LBB568_746:
	cmp x23, x22
	b.ls LBB568_1405
	add w2, w22, #4
	cmp x23, x2
	b.ls LBB568_1400
	ldr x8, [x26, #304]
	ldr q0, [x8, x22, lsl #4]
	str q0, [x8, x2, lsl #4]
	add w0, w22, #1
	ldr x1, [x26, #312]
	cmp x1, x0
	b.ls LBB568_1401
	add w8, w22, #5
	cmp x1, x8
	b.ls LBB568_1403
	ldr x9, [x26, #304]
	ldr q0, [x9, x0, lsl #4]
	str q0, [x9, x8, lsl #4]
	add w0, w22, #2
	ldr x1, [x26, #312]
	cmp x1, x0
	b.ls LBB568_1404
	add w8, w22, #6
	cmp x1, x8
	b.ls LBB568_1402
	ldr x9, [x26, #304]
	ldr q0, [x9, x0, lsl #4]
	str q0, [x9, x8, lsl #4]
	lsr w5, w28, #24
	add x0, sp, #1984
	mov x1, x26
	mov w3, #1
	mov w4, #2
	mov w6, #0
	bl luna_core::vm::exec::Vm::begin_call
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1288
LBB568_753:
	ldr x8, [sp, #224]
	add w21, w19, w8
LBB568_754:
	ldr x8, [x26, #304]
	add x9, x8, x21, lsl #4
	ldrb w11, [x9]
	ldr x10, [x9, #8]
	cmp w11, #2
	b.eq LBB568_781
	cmp w11, #3
	b.ne LBB568_1345
	ldr x11, [sp, #224]
	add w12, w11, w19
	add w11, w12, #1
	add x11, x8, w11, uxtw #4
	ldrb w13, [x11]
	cmp w13, #3
	b.ne LBB568_1346
	add w12, w12, #2
	add x12, x8, w12, uxtw #4
	ldrb w13, [x12]
	cmp w13, #3
	b.ne LBB568_1347
	fmov d0, x10
	ldr d1, [x11, #8]
	ldr d2, [x12, #8]
	fadd d0, d2, d0
	fcmp d0, d1
	cset w10, ls
	cset w11, ge
	fcmp d2, #0.0
	csel w10, w10, w11, gt
	tbz w10, #0, LBB568_2
	mov w10, #3
	strb w10, [x9]
	str d0, [x9, #8]
	ldr x9, [sp, #224]
	add w9, w9, w19
	add w9, w9, #3
	add x8, x8, w9, uxtw #4
	strb w10, [x8]
	str d0, [x8, #8]
	b LBB568_1237
LBB568_760:
	ldr q0, [x8, w9, uxtw #4]
	str q0, [sp, #560]
	ldr q0, [x10, x0, lsl #4]
	str q0, [sp, #1984]
	add x0, sp, #1536
	add x2, sp, #560
	add x3, sp, #1984
	mov x1, x26
	bl luna_core::vm::exec::Vm::eq_step
	ubfx w5, w28, #15, #1
Lloh3945:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.748@PAGE
Lloh3946:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.748@PAGEOFF
	add x0, sp, #1984
	add x2, sp, #1536
	add x3, sp, #1504
	add x4, sp, #1520
	mov x1, x26
	bl luna_core::vm::exec::Vm::op_compare
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1295
LBB568_761:
	ldr x2, [x10, #24]
	ldr w1, [x10, #20]
	add x3, sp, #960
	mov x0, x26
	bl luna_core::vm::exec::Vm::write_slot
	b LBB568_2
LBB568_762:
	mov x23, x25
	mov x22, #0
LBB568_763:
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w26, w9, w8, uxtb
	lsr w21, w28, #24
	sub w8, w21, #1
	cmp w21, #0
	mov x28, x10
	csel w27, w10, w8, eq
	add w19, w27, w26
	subs x25, x19, x24
	b.ls LBB568_770
	ldr x8, [sp, #216]
	ldr x8, [x8, #296]
	sub x8, x8, x24
	cmp x25, x8
	mov x8, x24
	b.hi LBB568_1245
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x8, lsl #4
	cmp x25, #2
	b.lo LBB568_769
LBB568_766:
	sub x10, x24, x19
	add x10, x10, #1
LBB568_767:
	strb wzr, [x9], #16
	adds x10, x10, #1
	b.lo LBB568_767
	add x8, x25, x8
	sub x8, x8, #1
LBB568_769:
	strb wzr, [x9]
	add x8, x8, #1
	ldr x9, [sp, #216]
	str x8, [x9, #312]
LBB568_770:
	cbz w27, LBB568_779
	mov x8, #0
	add w20, w23, #1
	mov w24, w28
	b LBB568_774
LBB568_772:
	strb wzr, [sp, #1952]
	mov w0, w26
	ldr x8, [sp, #216]
	ldr x1, [x8, #312]
	cmp x1, x0
	b.ls LBB568_1367
LBB568_773:
	ldr x8, [sp, #232]
	ldr x8, [x8]
	ldr q0, [sp, #1952]
	str q0, [x8, x0, lsl #4]
	add w20, w20, #1
	add w26, w26, #1
	mov x8, x23
	cmp x27, x23
	b.eq LBB568_779
LBB568_774:
	add x23, x8, #1
	cmp x8, x24
	b.hs LBB568_772
	cbz x22, LBB568_777
	add x8, sp, #1952
	mov x0, x22
	mov x1, x23
	bl luna_core::runtime::table::Table::get_int
	mov w0, w26
	ldr x8, [sp, #216]
	ldr x1, [x8, #312]
	cmp x1, x0
	b.hi LBB568_773
	b LBB568_1367
LBB568_777:
	mov w0, w20
	ldr x8, [sp, #216]
	ldr x1, [x8, #312]
	cmp x1, x0
	b.ls LBB568_1383
	ldr x8, [sp, #232]
	ldr x8, [x8]
	ldr q0, [x8, x0, lsl #4]
	str q0, [sp, #1952]
	mov w0, w26
	ldr x8, [sp, #216]
	ldr x1, [x8, #312]
	cmp x1, x0
	b.hi LBB568_773
	b LBB568_1367
LBB568_779:
	ldr x26, [sp, #216]
	cbnz w21, LBB568_2
	str w19, [x26, #1792]
	b LBB568_2
LBB568_781:
	ldrb w14, [x26, #1840]
	ldr x11, [sp, #224]
	add w11, w11, w19
	add w11, w11, #1
	add x12, x8, w11, uxtw #4
	ldrb w13, [x12]
	ldr x11, [x12, #8]
	cmp w14, #3
	b.hs LBB568_939
	cmp w13, #2
	b.ne LBB568_1355
	ldr x12, [sp, #224]
	add w12, w12, w19
	add w12, w12, #2
	add x12, x8, w12, uxtw #4
	ldrb w13, [x12]
	cmp w13, #2
	b.ne LBB568_1356
	ldr x12, [x12, #8]
	add x10, x12, x10
	cmp x10, x11
	cset w11, le
	cset w13, ge
	cmp x12, #0
	csel w11, w11, w13, gt
	tbz w11, #0, LBB568_2
	mov w11, #2
	strb w11, [x9]
	str x10, [x9, #8]
	ldr x9, [sp, #224]
	add w9, w9, w19
	add w9, w9, #3
	add x8, x8, w9, uxtw #4
	strb w11, [x8]
	b LBB568_943
LBB568_786:
	cmp w27, #4
	b.gt LBB568_944
	cmp w27, #1
	b.le LBB568_1020
	sub w8, w27, #2
	cmp w8, #2
	b.hs LBB568_1081
	mov w8, #2
	b LBB568_1084
LBB568_790:
	lsr w10, w28, #7
	ldp x12, x11, [sp, #224]
	ldr x11, [x11]
	add w10, w12, w10, uxtb
	add x10, x11, w10, uxtw #4
	stp x9, x8, [x10]
	b LBB568_2
LBB568_791:
	mov x21, #0
LBB568_792:
Lloh3947:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.789@PAGE
Lloh3948:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.789@PAGEOFF
	mov x0, x26
	mov w2, #1
	bl luna_core::runtime::heap::Heap::intern
	str x21, [sp, #568]
	mov w8, #2
	strb w8, [sp, #560]
	mov w8, #4
	strb w8, [sp, #1984]
	str x0, [sp, #1992]
	add x2, sp, #1984
	add x3, sp, #560
	mov x0, x22
	mov x1, x26
	bl luna_core::runtime::table::Table::set_norm
	and w8, w0, #0xff
	cmp w8, #4
	b.ne LBB568_1340
	ldrb w8, [x22, #9]
	tbz w8, #2, LBB568_796
	and w8, w8, #0xf8
	strb w8, [x22, #9]
	ldr x19, [x26, #64]
	ldr x8, [x26, #48]
	cmp x19, x8
	b.eq LBB568_1138
LBB568_795:
	ldr x8, [x26, #56]
	str x22, [x8, x19, lsl #3]
	add x8, x19, #1
	str x8, [x26, #64]
LBB568_796:
	ldr x1, [x26, #312]
	cmp x1, x25
	b.ls LBB568_1406
	ldp x11, x10, [sp, #224]
	ldr x8, [x10]
	add x8, x8, x25, lsl #4
	mov w9, #5
	strb w9, [x8]
	str x22, [x8, #8]
	lsr w8, w28, #7
	ldr x10, [x10]
	add w8, w11, w8, uxtb
	add x8, x10, w8, uxtw #4
	strb w9, [x8]
	str x22, [x8, #8]
	b LBB568_2
LBB568_798:
	ldr w9, [x26, #1792]
	sub w9, w9, w8
LBB568_799:
	ldr x19, [sp, #224]
LBB568_800:
	ldr w10, [x26, #1792]
	add w11, w9, w8
	cmp w11, w10
	csel w10, w11, w10, hi
	str w10, [x26, #1792]
	mov w10, #11
	strb w10, [sp, #288]
	str w8, [sp, #1988]
	str w9, [sp, #1992]
	mov w8, #1
	strh w8, [sp, #1984]
	mov x0, x26
	mov x1, x19
	bl luna_core::vm::exec::Vm::close_from
	add x0, sp, #560
	add x3, sp, #288
	add x4, sp, #1984
	mov x1, x26
	mov x2, x19
	ldr x5, [sp, #96]
	bl luna_core::vm::exec::Vm::drive_close
	ldr x8, [sp, #560]
	add x9, sp, #560
	ldur q0, [x9, #8]
	str q0, [sp, #1728]
	mov x9, #-9223372036854775808
	cmp x8, x9
	b.eq LBB568_2
	b LBB568_1262
LBB568_801:
	ldr x25, [sp, #144]
	ldr w9, [sp, #160]
	ldr x19, [x22, #56]
	cmp x19, #255
	b.hi LBB568_491
	b LBB568_504
LBB568_802:
	ldrb w9, [x9, #8]
	eor w9, w9, #0x1
LBB568_803:
	ubfx w10, w28, #7, #8
	and w9, w9, #0x1
	ldr x11, [sp, #224]
	add w10, w10, w11
	add x8, x8, w10, uxtw #4
	mov w10, #1
	strb w10, [x8]
	strb w9, [x8, #8]
	b LBB568_2
LBB568_804:
	cmp w23, #3
	b.ne LBB568_806
	fmov d0, x19
	fmov d1, x22
	fadd d0, d0, d1
	lsr w9, w28, #7
	b LBB568_1031
LBB568_806:
	strb w21, [sp, #288]
	ldr w8, [x9]
	ldr x11, [sp, #112]
	str w8, [x11]
	ldur w8, [x9, #3]
	stur w8, [x11, #3]
	str x19, [sp, #296]
	strb w23, [sp, #560]
	ldr w8, [x10]
	ldr x9, [sp, #104]
	str w8, [x9]
	ldur w8, [x10, #3]
	stur w8, [x9, #3]
	str x22, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #0
	bl luna_core::vm::exec::Vm::arith_fast
	ldr w9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-176]
	ldur x10, [x11, #16]
	add x11, sp, #2320
	stur x10, [x11, #183]
	add x10, sp, #2320
	tbz w9, #0, LBB568_808
	ldur x9, [x29, #-176]
	stur x9, [x29, #-144]
	ldur x9, [x10, #183]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
LBB568_808:
	cmp w8, #11
	b.ne LBB568_949
	strb w21, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x19, [sp, #296]
	strb w23, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x22, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #0
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w21, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x19, [sp, #1992]
	strb w23, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x22, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3949:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.732@PAGE
Lloh3950:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.732@PAGEOFF
	b LBB568_1066
LBB568_811:
	tbz w28, #15, LBB568_2
	b LBB568_391
LBB568_812:
	cmp w23, #3
	b.ne LBB568_814
	fmov d0, x19
	fmov d1, x22
	fsub d0, d0, d1
	lsr w9, w28, #7
	b LBB568_1031
LBB568_814:
	strb w21, [sp, #288]
	ldr w8, [x9]
	ldr x11, [sp, #112]
	str w8, [x11]
	ldur w8, [x9, #3]
	stur w8, [x11, #3]
	str x19, [sp, #296]
	strb w23, [sp, #560]
	ldr w8, [x10]
	ldr x9, [sp, #104]
	str w8, [x9]
	ldur w8, [x10, #3]
	stur w8, [x9, #3]
	str x22, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #1
	bl luna_core::vm::exec::Vm::arith_fast
	ldr w9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-192]
	ldur x10, [x11, #16]
	add x11, sp, #2320
	stur x10, [x11, #167]
	add x10, sp, #2320
	tbz w9, #0, LBB568_816
	ldur x9, [x29, #-192]
	stur x9, [x29, #-144]
	ldur x9, [x10, #167]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
LBB568_816:
	cmp w8, #11
	b.ne LBB568_950
	strb w21, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x19, [sp, #296]
	strb w23, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x22, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #1
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w21, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x19, [sp, #1992]
	strb w23, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x22, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3951:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.308@PAGE
Lloh3952:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.308@PAGEOFF
	b LBB568_1066
LBB568_819:
	cmp w23, #3
	b.ne LBB568_821
	fmov d0, x19
	fmov d1, x22
	fmul d0, d0, d1
	lsr w9, w28, #7
	b LBB568_1031
LBB568_821:
	strb w21, [sp, #288]
	ldr w8, [x9]
	ldr x11, [sp, #112]
	str w8, [x11]
	ldur w8, [x9, #3]
	stur w8, [x11, #3]
	str x19, [sp, #296]
	strb w23, [sp, #560]
	ldr w8, [x10]
	ldr x9, [sp, #104]
	str w8, [x9]
	ldur w8, [x10, #3]
	stur w8, [x9, #3]
	str x22, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #2
	bl luna_core::vm::exec::Vm::arith_fast
	ldr w9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-208]
	ldur x10, [x11, #16]
	add x11, sp, #2320
	stur x10, [x11, #151]
	add x10, sp, #2320
	tbz w9, #0, LBB568_824
	ldur x9, [x29, #-208]
	stur x9, [x29, #-144]
	ldur x9, [x10, #151]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
LBB568_823:
	tbnz w28, #15, LBB568_2
	b LBB568_391
LBB568_824:
	cmp w8, #11
	b.ne LBB568_951
	strb w21, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x19, [sp, #296]
	strb w23, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x22, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #2
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.eq LBB568_938
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w21, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x19, [sp, #1992]
	strb w23, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x22, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3953:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.733@PAGE
Lloh3954:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.733@PAGEOFF
	b LBB568_1066
LBB568_827:
	tbnz w28, #15, LBB568_391
	b LBB568_829
LBB568_828:
	tbz w28, #15, LBB568_391
LBB568_829:
	lsr w12, w28, #7
	ldr x13, [sp, #224]
	add w12, w13, w12, uxtb
	add x10, x10, w12, uxtw #4
	strb w11, [x10]
	ldurb w11, [x8, #7]
	ldurh w12, [x8, #5]
	ldur w13, [x8, #1]
	stur w13, [x10, #1]
	sturh w12, [x10, #5]
	strb w11, [x10, #7]
	strb w9, [x10, #8]
	ldurb w9, [x8, #15]
	ldurh w11, [x8, #13]
	ldur w8, [x8, #9]
	stur w8, [x10, #9]
	sturh w11, [x10, #13]
	strb w9, [x10, #15]
	b LBB568_2
LBB568_830:
	mov w11, #0
	ldr x1, [x26, #312]
	cmp x1, x23
	b.lo LBB568_832
LBB568_831:
	str x23, [x26, #312]
	mov x1, x23
LBB568_832:
	str w14, [x26, #1792]
	cmp w12, #1
	b.gt LBB568_836
	cbnz w12, LBB568_2
	cmp x1, x0
	b.ls LBB568_1420
	ldr x8, [sp, #232]
	ldr x8, [x8]
	add x8, x8, x0, lsl #4
	strb w11, [x8]
	ldr w9, [sp, #248]
	stur w9, [x8, #1]
	ldur w9, [sp, #251]
	str w9, [x8, #4]
	ldr w9, [sp, #12]
	strb w9, [x8, #8]
	ldr w9, [sp, #240]
	stur w9, [x8, #9]
	ldur w9, [sp, #243]
	str w9, [x8, #12]
	b LBB568_2
LBB568_836:
	cmp w12, #2
	b.ne LBB568_955
	ldr w12, [sp, #12]
	tbz w13, #0, LBB568_1034
	cbz w11, LBB568_1117
	cmp w11, #1
	b.ne LBB568_1116
	eor w11, w12, #0x1
	b LBB568_1037
LBB568_841:
	ldr w1, [x27, #32]
	ldrb w26, [x19, #16]
	cmp x1, x26
	b.ls LBB568_1436
	ldr x8, [x27, #24]
	ldr x22, [x8, x26, lsl #3]
	ldr x26, [sp, #216]
	cmp x24, #3
	b.hs LBB568_323
LBB568_843:
	str x22, [sp, #560]
	cmp x21, #1
	b.eq LBB568_971
LBB568_844:
	ldrb w8, [x19, #41]
	tbz w8, #0, LBB568_968
	ldrb w8, [x19, #40]
	ldr x9, [sp, #224]
	add w1, w9, w8
	mov x0, x26
	bl luna_core::vm::exec::Vm::find_or_create_upval
	mov x22, x0
	cmp x24, #3
	b.lo LBB568_970
LBB568_847:
	ldr x20, [sp, #2000]
	ldr x8, [sp, #1984]
	cmp x20, x8
	b.ne LBB568_849
	add x0, sp, #1984
	bl alloc::raw_vec::RawVec<T,A>::grow_one
LBB568_849:
	ldr x8, [sp, #1992]
	str x22, [x8, x20, lsl #3]
	add x8, x20, #1
	str x8, [sp, #2000]
	cmp x21, #2
	b.eq LBB568_971
	add x8, x21, x21, lsl #1
	lsl x8, x8, #3
	sub x20, x8, #48
	add x19, x19, #64
	b LBB568_853
LBB568_851:
	add x0, sp, #1984
	bl alloc::raw_vec::RawVec<T,A>::grow_one
LBB568_852:
	ldr x8, [sp, #1992]
	str x22, [x8, x21, lsl #3]
	add x8, x21, #1
	str x8, [sp, #2000]
	add x19, x19, #24
	subs x20, x20, #24
	b.eq LBB568_971
LBB568_853:
	ldrb w8, [x19, #1]
	tbz w8, #0, LBB568_856
	ldrb w8, [x19]
	ldr x9, [sp, #224]
	add w1, w9, w8
	mov x0, x26
	bl luna_core::vm::exec::Vm::find_or_create_upval
	mov x22, x0
	ldr x21, [sp, #2000]
	ldr x8, [sp, #1984]
	cmp x21, x8
	b.ne LBB568_852
	b LBB568_851
LBB568_856:
	ldr w1, [x27, #32]
	ldrb w26, [x19]
	cmp x1, x26
	b.ls LBB568_1436
	ldr x8, [x27, #24]
	ldr x22, [x8, x26, lsl #3]
	ldr x26, [sp, #216]
	ldr x21, [sp, #2000]
	ldr x8, [sp, #1984]
	cmp x21, x8
	b.ne LBB568_852
	b LBB568_851
LBB568_858:
	add x0, sp, #1984
	ldr x1, [sp, #216]
	mov x2, x26
	mov w3, #1
	mov x4, x24
	mov w5, #-1
	mov w6, #0
	bl luna_core::vm::exec::Vm::begin_call
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.ne LBB568_1271
	ldr x26, [sp, #216]
	b LBB568_2
LBB568_860:
	add x0, x26, #584
	bl alloc::raw_vec::RawVec<T,A>::grow_one
	b LBB568_22
LBB568_861:
	add x13, x27, #32
	add x11, x27, #40
	ldr x11, [x11]
	ldr x12, [sp, #600]
	cmp x12, x11
	b.hs LBB568_863
LBB568_862:
	ldr x9, [sp, #216]
	ldr x8, [x9, #728]
	add x8, x8, #1
	str x8, [x9, #728]
	b LBB568_1059
LBB568_863:
	mov x12, #0
	ldr x13, [x13]
	add x13, x13, #16
	ldr x14, [sp, #592]
	add x14, x14, #16
	ldp x16, x15, [x27, #48]
	add x16, x16, #16
	b LBB568_866
LBB568_864:
	cmp w17, #5
	mov w2, #5
	csel w2, wzr, w2, eq
	cmp w17, #3
	mov w3, #6
	mov w4, #7
	csel w3, w3, w4, eq
	cmp w17, #4
	csel w17, w2, w3, gt
LBB568_865:
	ldrb w2, [x14, x12]
	add x12, x12, #1
	cmp w2, w17
	b.ne LBB568_862
LBB568_866:
	cmp x11, x12
	b.eq LBB568_872
	ldrb w17, [x13, x12]
	cmp w17, #2
	b.gt LBB568_864
	cbz w17, LBB568_870
	cmp w17, #1
	mov w17, #4
	mov w2, #3
	csel w17, w2, w17, eq
	b LBB568_865
LBB568_870:
	cmp x12, x15
	b.hs LBB568_862
	ldrb w17, [x16, x12]
	b LBB568_865
LBB568_872:
	ldr x11, [x27, #120]
	cmp x0, x11
	b.hs LBB568_874
	ldr x11, [x27, #112]
	add x11, x11, x0, lsl #3
	str x9, [x11, #16]
LBB568_874:
	subs x11, x0, x10
	b.hs LBB568_983
	ldr x1, [x27, #88]
	cmp x0, x1
	b.hs LBB568_1423
	ldr x10, [x27, #80]
	mov w11, #48
	madd x10, x0, x11, x10
	ldr x10, [x10, #48]
	str x9, [x10]
	mov w9, #32
	mov x11, x0
	b LBB568_1041
LBB568_877:
	mov w8, #1
	stp x8, x28, [x29, #-144]
	subs w24, w23, #2
	b.eq LBB568_887
LBB568_878:
	cmp w23, #3
	b.eq LBB568_881
	cmp w23, #4
	b.ne LBB568_890
	ldr w1, [x22, #32]
	sub x8, x29, #128
	add x0, x22, #40
	mov w2, #1
	mov w3, #1
	bl luna_core::numeric::str2num
	cmp w19, #4
	b.ne LBB568_882
	b LBB568_888
LBB568_881:
	mov w8, #1
	stp x8, x22, [x29, #-128]
	cmp w19, #4
	b.eq LBB568_888
LBB568_882:
	cmp w19, #3
	b.eq LBB568_885
	cmp w19, #2
	b.ne LBB568_891
	mov x11, #0
	stur x21, [x29, #-104]
	cmp x11, #2
	b.ne LBB568_892
	b LBB568_896
LBB568_885:
	stur x21, [x29, #-104]
	mov w11, #1
	cmp x11, #2
	b.ne LBB568_892
	b LBB568_896
LBB568_886:
	stp xzr, x28, [x29, #-144]
	subs w24, w23, #2
	b.ne LBB568_878
LBB568_887:
	stp xzr, x22, [x29, #-128]
	cmp w19, #4
	b.ne LBB568_882
LBB568_888:
	ldr w1, [x21, #32]
	sub x8, x29, #112
	add x0, x21, #40
	mov w2, #1
	mov w3, #1
	bl luna_core::numeric::str2num
	ldur x11, [x29, #-112]
	cmp x11, #2
	b.ne LBB568_892
	b LBB568_896
LBB568_889:
	mov w8, #2
	stur x8, [x29, #-144]
	subs w24, w23, #2
	b.ne LBB568_878
	b LBB568_887
LBB568_890:
	mov w8, #2
	stur x8, [x29, #-128]
	cmp w19, #4
	b.ne LBB568_882
	b LBB568_888
LBB568_891:
	mov w11, #2
	cmp x11, #2
	b.eq LBB568_896
LBB568_892:
	ldur x14, [x29, #-144]
	cmp x14, #2
	b.eq LBB568_896
	ldur x13, [x29, #-128]
	cmp x13, #2
	b.eq LBB568_896
	ldur x9, [x29, #-136]
	ldur x10, [x29, #-120]
	ldur x8, [x29, #-104]
	ldr x12, [sp, #216]
	ldrb w12, [x12, #1840]
	tbz w14, #0, LBB568_899
	fmov d0, x9
	fmov d1, x10
	scvtf d2, x10
	tst w13, #0x1
	fcsel d1, d1, d2, ne
	fmov d2, x8
	scvtf d3, x8
	tst w11, #0x1
	fcsel d2, d2, d3, ne
	mov x14, x20
	fcmp d2, #0.0
	b.eq LBB568_995
	b LBB568_901
LBB568_896:
	cmp w24, #2
	b.hs LBB568_903
LBB568_897:
	sub w8, w19, #2
	cmp w8, #2
	ldr x26, [sp, #216]
	b.hs LBB568_957
LBB568_898:
	mov w8, #13
	mov x22, x28
	mov x23, x27
Lloh3955:
	adrp x9, l_anon.89dbc2968085ea1691689a13183de4a7.1061@PAGE
Lloh3956:
	add x9, x9, l_anon.89dbc2968085ea1691689a13183de4a7.1061@PAGEOFF
	b LBB568_960
LBB568_899:
	cmp x11, #1
	mov x14, x20
	b.ne LBB568_987
	scvtf d0, x9
	fmov d1, x10
	scvtf d2, x10
	tst w13, #0x1
	fcsel d1, d1, d2, ne
	fmov d2, x8
	fcmp d2, #0.0
	b.eq LBB568_995
LBB568_901:
	cmp w12, #3
	b.hs LBB568_1038
	fsub d0, d0, d2
	mov w8, #3
	ldp x10, x9, [sp, #200]
	strb w8, [x10]
	str d0, [x10, #8]
	strb w8, [x9]
	str d1, [x9, #8]
	ldp x26, x9, [sp, #216]
	strb w8, [x9]
	str d2, [x9, #8]
	b LBB568_1209
LBB568_903:
	cmp w23, #4
	b.ne LBB568_905
	ldr w1, [x22, #32]
	add x8, sp, #1984
	add x0, x22, #40
	mov w2, #1
	mov w3, #1
	bl luna_core::numeric::str2num
	ldr x8, [sp, #1984]
	cmp x8, #2
	b.ne LBB568_897
LBB568_905:
Lloh3957:
	adrp x9, l_anon.89dbc2968085ea1691689a13183de4a7.1063@PAGE
Lloh3958:
	add x9, x9, l_anon.89dbc2968085ea1691689a13183de4a7.1063@PAGEOFF
	mov w8, #5
	ldr x26, [sp, #216]
	b LBB568_960
LBB568_906:
	mov w8, #1
	stp x8, x9, [sp, #288]
LBB568_907:
	fmov d0, x9
	frintz d1, d0
	fcmp d1, d0
	mov x8, #-4332462841530417152
	fmov d1, x8
	fccmp d0, d1, #8, eq
	mov x8, #4890909195324358656
	fmov d1, x8
	fccmp d0, d1, #0, ge
	b.mi LBB568_967
Lloh3959:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.379@PAGE
Lloh3960:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.379@PAGEOFF
	add x8, sp, #1984
	mov x0, x26
	mov w2, #36
	bl luna_core::vm::exec::Vm::rt_err
	ldrb w8, [sp, #1984]
	ldr x9, [sp, #1992]
	cmp w8, #11
	b.eq LBB568_997
	b LBB568_1327
LBB568_909:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldr x8, [sp, #2400]
	stur x8, [x9, #1]
	ldur x8, [x11, #87]
	str x8, [x9, #8]
	b LBB568_2
LBB568_910:
	ldr w1, [x9, #32]
	add x8, sp, #288
	add x0, x9, #40
	mov w2, #1
	mov w3, #1
	bl luna_core::numeric::str2num
	ldr x8, [sp, #288]
	cmp x8, #2
	add x19, sp, #1744
	b.ne LBB568_996
	mov w8, #3
	b LBB568_1007
LBB568_912:
	mov w10, #2
	str x10, [sp, #288]
	cmp w8, #5
	b.le LBB568_1004
	cmp w8, #8
	b.ge LBB568_1124
	mov w8, #4
	b LBB568_1007
LBB568_915:
	mov w10, #1
	stp x10, x9, [sp, #288]
	fmov d0, x9
	b LBB568_1030
LBB568_916:
	ldr w1, [x9, #32]
	add x8, sp, #288
	add x0, x9, #40
	mov w2, #1
	mov w3, #1
	bl luna_core::numeric::str2num
	ldr x8, [sp, #288]
	cmp x8, #2
	b.eq LBB568_1022
	cbnz x8, LBB568_1029
	ldr x9, [sp, #296]
	ldr x8, [sp, #232]
	ldr x8, [x8]
LBB568_919:
	lsr w10, w28, #7
	neg x9, x9
LBB568_920:
	ldr x11, [sp, #224]
	add w10, w11, w10, uxtb
	add x8, x8, w10, uxtw #4
	b LBB568_999
LBB568_921:
	mov w8, #2
	str x8, [sp, #288]
	cmp w10, #5
	b.le LBB568_1012
	cmp w10, #8
	b.ge LBB568_1126
	mov w10, #4
	b LBB568_1023
LBB568_924:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldur x8, [x29, #-240]
	stur x8, [x9, #1]
	ldur x8, [x11, #119]
	str x8, [x9, #8]
	b LBB568_2
LBB568_925:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldur x8, [x29, #-224]
	stur x8, [x9, #1]
	ldur x8, [x11, #135]
	str x8, [x9, #8]
	b LBB568_2
LBB568_926:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldr x8, [sp, #2336]
	stur x8, [x9, #1]
	ldur x8, [x11, #23]
	str x8, [x9, #8]
	b LBB568_2
LBB568_927:
	mov w9, #0
	fmov d0, x10
	ucvtf d1, w24
	fcmp d0, d1
	fmov d1, #1.00000000
	fccmp d0, d1, #8, ls
	ldr x11, [sp, #16]
	b.lt LBB568_1002
	frintz d1, d0
	fsub d1, d0, d1
	fcmp d1, #0.0
	b.ne LBB568_1002
	fcvtzu w9, d0
	add w0, w9, w25
	cmp x1, x0
	b.hi LBB568_1001
	b LBB568_1425
LBB568_930:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldr x8, [sp, #2352]
	stur x8, [x9, #1]
	ldur x8, [x11, #39]
	str x8, [x9, #8]
	b LBB568_2
LBB568_931:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldr x8, [sp, #2320]
	stur x8, [x9, #1]
	ldur x8, [x11, #7]
	str x8, [x9, #8]
	b LBB568_2
LBB568_932:
	cmp x10, #1
	ccmp x10, x24, #2, ge
	add x12, sp, #1744
	ldr x11, [sp, #16]
	b.ls LBB568_1000
LBB568_933:
	mov w9, #0
	b LBB568_1002
LBB568_934:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldr x8, [sp, #2384]
	stur x8, [x9, #1]
	ldur x8, [x11, #71]
	str x8, [x9, #8]
	b LBB568_2
LBB568_935:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldr x8, [sp, #2368]
	stur x8, [x9, #1]
	ldur x8, [x11, #55]
	str x8, [x9, #8]
	b LBB568_2
LBB568_936:
	cmp w8, #11
	b.ne LBB568_1003
	strb w21, [sp, #288]
	ldr w8, [sp, #1744]
	ldp x9, x11, [sp, #104]
	str w8, [x11]
	add x24, sp, #1744
	ldur w8, [x24, #3]
	stur w8, [x11, #3]
	str x19, [sp, #296]
	strb w23, [sp, #560]
	ldur w8, [x29, #-160]
	str w8, [x9]
	ldur w8, [x10, #195]
	stur w8, [x9, #3]
	str x22, [sp, #568]
	add x0, sp, #1984
	add x3, sp, #288
	add x4, sp, #560
	mov x1, x26
	mov w2, #5
	add x20, sp, #2320
	bl luna_core::vm::exec::Vm::arith_mm_func
	ldr x9, [sp, #1984]
	ldrb w8, [sp, #1992]
	add x11, sp, #1984
	ldur x10, [x11, #9]
	stur x10, [x29, #-128]
	ldur x10, [x11, #16]
	stur x10, [x20, #231]
	add x10, sp, #2320
	cmp x9, #1
	b.ne LBB568_1065
LBB568_938:
	ldur x9, [x29, #-128]
	stur x9, [x29, #-144]
	ldur x9, [x10, #231]
	stur x9, [x10, #215]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1269
LBB568_939:
	cmp w13, #2
	b.ne LBB568_1354
	cmp x11, #0
	b.le LBB568_2
	ldr x13, [sp, #224]
	add w13, w13, w19
	add w14, w13, #2
	add x14, x8, w14, uxtw #4
	ldrb w15, [x14]
	cmp w15, #2
	b.ne LBB568_1360
	ldr x14, [x14, #8]
	add x10, x14, x10
	mov w14, #2
	strb w14, [x9]
	str x10, [x9, #8]
	sub x9, x11, #1
	strb w14, [x12]
	str x9, [x12, #8]
	add w9, w13, #3
	add x8, x8, w9, uxtw #4
	strb w14, [x8]
LBB568_943:
	str x10, [x8, #8]
	b LBB568_1237
LBB568_944:
	ldr x8, [sp, #1704]
	cmp w27, #7
	b.gt LBB568_1073
	sub w9, w27, #6
	cmp w9, #2
	b.hs LBB568_1082
	mov w8, #4
	b LBB568_1084
LBB568_947:
	mov w8, #0
	lsr w9, w28, #7
	ldr x10, [sp, #224]
	add w0, w10, w9, uxtb
	ldr x1, [x26, #312]
	cmp x1, x0
	b.hi LBB568_1121
	b LBB568_1417
LBB568_948:
	add x0, x22, #40
	mov x20, x12
	str x13, [sp, #152]
	bl alloc::raw_vec::RawVec<T,A>::grow_one
	ldr x13, [sp, #152]
	ldr w9, [sp, #160]
	mov x12, x20
	b LBB568_505
LBB568_949:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldur x8, [x29, #-176]
	stur x8, [x9, #1]
	ldur x8, [x11, #183]
	str x8, [x9, #8]
	b LBB568_2
LBB568_950:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldur x8, [x29, #-192]
	stur x8, [x9, #1]
	ldur x8, [x11, #167]
	str x8, [x9, #8]
	b LBB568_2
LBB568_951:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldur x8, [x29, #-208]
	stur x8, [x9, #1]
	ldur x8, [x11, #151]
	str x8, [x9, #8]
	b LBB568_2
LBB568_952:
	cmp w12, #3
	ccmp w13, #3, #0, eq
	b.ne LBB568_754
	fmov d1, x9
	fmov d0, x8
	fmov d2, x10
	fadd d1, d1, d2
	fcmp d2, #0.0
	b.le LBB568_1129
	fcmp d1, d0
	b.hi LBB568_754
	b LBB568_1170
LBB568_955:
	cmp x1, x0
	ldr w10, [sp, #12]
	b.ls LBB568_1419
	ldr x8, [x26, #304]
	add x8, x8, x0, lsl #4
	strb w11, [x8]
	ldr w9, [sp, #248]
	stur w9, [x8, #1]
	ldur w9, [sp, #251]
	str w9, [x8, #4]
	strb w10, [x8, #8]
	ldr w9, [sp, #240]
	stur w9, [x8, #9]
	ldur w9, [sp, #243]
	str w9, [x8, #12]
	add w8, w0, #1
	str w8, [x26, #1792]
	add x0, sp, #1984
	mov x1, x26
	bl luna_core::vm::exec::Vm::concat_run
	ldrb w8, [sp, #1984]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1326
LBB568_957:
	cmp w19, #4
	b.ne LBB568_959
	ldr w1, [x21, #32]
	add x8, sp, #1984
	add x0, x21, #40
	mov w2, #1
	mov w3, #1
	bl luna_core::numeric::str2num
	ldr x8, [sp, #1984]
	cmp x8, #2
	b.ne LBB568_898
LBB568_959:
	mov w8, #4
	mov x22, x21
	mov x23, x19
Lloh3961:
	adrp x9, l_anon.89dbc2968085ea1691689a13183de4a7.1062@PAGE
Lloh3962:
	add x9, x9, l_anon.89dbc2968085ea1691689a13183de4a7.1062@PAGEOFF
LBB568_960:
	stp x9, x8, [x29, #-160]
	add x19, sp, #288
	add x0, sp, #288
	mov x1, x26
	mov x2, x23
	mov x3, x22
	bl luna_core::vm::exec::Vm::obj_typename
	sub x8, x29, #160
Lloh3963:
	adrp x9, <&T as core::fmt::Display>::fmt@PAGE
Lloh3964:
	add x9, x9, <&T as core::fmt::Display>::fmt@PAGEOFF
	str x8, [sp, #1984]
	str x9, [sp, #1992]
Lloh3965:
	adrp x8, <alloc::string::String as core::fmt::Display>::fmt@PAGE
Lloh3966:
	add x8, x8, <alloc::string::String as core::fmt::Display>::fmt@PAGEOFF
	str x19, [sp, #2000]
	str x8, [sp, #2008]
Lloh3967:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.1059@PAGE
Lloh3968:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.1059@PAGEOFF
	add x8, sp, #560
	add x1, sp, #1984
	bl alloc::fmt::format::format_inner
	ldr x22, [sp, #560]
	ldr x23, [sp, #568]
	ldr x2, [sp, #576]
	add x8, sp, #1984
	mov x0, x26
	mov x1, x23
	bl luna_core::vm::exec::Vm::rt_err
	ldr q0, [sp, #1984]
	str q0, [sp, #1744]
	cbz x22, LBB568_964
	mov x0, x23
	mov x1, x22
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_964:
	ldr x1, [sp, #288]
	cbz x1, LBB568_966
	ldr x0, [sp, #296]
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_966:
	ldrb w8, [sp, #1744]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1293
LBB568_967:
	fcvtzs x9, d0
	b LBB568_997
LBB568_968:
	ldr w1, [x27, #32]
	ldrb w26, [x19, #40]
	cmp x1, x26
	b.ls LBB568_1436
	ldr x8, [x27, #24]
	ldr x22, [x8, x26, lsl #3]
	ldr x26, [sp, #216]
	cmp x24, #3
	b.hs LBB568_847
LBB568_970:
	str x22, [sp, #568]
	cmp x21, #2
	b.ne LBB568_1421
LBB568_971:
	ldr x22, [sp, #1992]
	ldr x8, [sp, #2000]
	add x9, sp, #560
	cmp x24, #3
	csel x2, x9, x22, lo
	csel x3, x24, x8, lo
	ldrb w8, [x26, #1840]
	cbz w8, LBB568_979
	ldr x0, [x23, #136]
	cbz x0, LBB568_977
	ldr w8, [x0, #32]
	cmp x3, x8
	b.ne LBB568_977
	ldr x8, [x0, #24]
	mov x9, x3
	mov x10, x2
LBB568_975:
	cbz x9, LBB568_1154
	ldr x11, [x8], #8
	ldr x12, [x10], #8
	sub x9, x9, #1
	cmp x11, x12
	b.eq LBB568_975
LBB568_977:
	mov x0, x26
	mov x1, x23
	bl luna_core::runtime::heap::Heap::new_closure_inline
	str x0, [x23, #136]
	b LBB568_1154
LBB568_979:
	ldrb w26, [x23, #144]
	cmp x26, #255
	b.eq LBB568_1153
	cmp x3, x26
	b.ls LBB568_1418
	ldr x8, [x2, x26, lsl #3]
	ldr w9, [x8, #16]
	ldr x21, [x8, #24]
	tbz w9, #0, LBB568_1015
	mov x20, x2
	mov x19, x3
	add x8, x8, #32
	b LBB568_1151
LBB568_983:
	add x10, x1, x10
	cmp x0, x10
	b.hs LBB568_1040
	ldr x10, [x27, #136]
	cmp x11, x10
	b.hs LBB568_986
	ldr x10, [x27, #128]
	add x10, x10, x11, lsl #3
	ldr x10, [x10, #16]
	str x9, [x10]
LBB568_986:
	mov w9, #64
	b LBB568_1041
LBB568_987:
	cbz x8, LBB568_995
	cmp w12, #3
	b.hs LBB568_1132
	tbz w13, #0, LBB568_1165
	fmov d0, x10
	fcmp d0, d0
	ldp x26, x12, [sp, #216]
	ldp x15, x13, [sp, #200]
	b.vs LBB568_1253
	mov x10, #4890909195324358656
	fmov d1, x10
	fcmp d0, d1
	b.ge LBB568_1196
	mov x10, #-4332462841530417152
	fmov d1, x10
	fcmp d0, d1
	b.ls LBB568_1206
	cmp x8, #0
	b.le LBB568_1242
	fcvtms x10, d0
	b LBB568_1208
LBB568_995:
Lloh3969:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.1060@PAGE
Lloh3970:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.1060@PAGEOFF
	add x8, sp, #1744
	ldr x26, [sp, #216]
	mov x0, x26
	mov w2, #18
	bl luna_core::vm::exec::Vm::rt_err
	ldrb w8, [sp, #1744]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1293
LBB568_996:
	ldr x9, [sp, #296]
	cmp x8, #1
	b.eq LBB568_907
LBB568_997:
	lsr w8, w28, #7
	mvn x9, x9
LBB568_998:
	ldp x11, x10, [sp, #224]
	ldr x10, [x10]
	add w8, w11, w8, uxtb
	add x8, x10, w8, uxtw #4
LBB568_999:
	mov w10, #2
	strb w10, [x8]
	str x9, [x8, #8]
	b LBB568_2
LBB568_1000:
	add w0, w25, w10
	cmp x1, x0
	b.ls LBB568_1426
LBB568_1001:
	add x10, x8, x0, lsl #4
	ldrb w9, [x10]
	ldur w11, [x10, #1]
	str w11, [sp, #1976]
	ldr w11, [x10, #4]
	stur w11, [x12, #235]
	ldr x11, [x10, #8]
LBB568_1002:
	lsr w10, w28, #7
	ldr x13, [sp, #224]
	add w10, w13, w10, uxtb
	add x8, x8, w10, uxtw #4
	strb w9, [x8]
	ldr w9, [sp, #1976]
	stur w9, [x8, #1]
	ldur w9, [x12, #235]
	str w9, [x8, #4]
	str x11, [sp, #16]
	str x11, [x8, #8]
	b LBB568_2
LBB568_1003:
	lsr w9, w28, #7
	add x11, sp, #2320
	ldp x12, x10, [sp, #224]
	ldr x10, [x10]
	add w9, w12, w9, uxtb
	add x9, x10, w9, uxtw #4
	strb w8, [x9]
	ldur x8, [x29, #-256]
	stur x8, [x9, #1]
	ldur x8, [x11, #103]
	str x8, [x9, #8]
	b LBB568_2
LBB568_1004:
	cbz w8, LBB568_1007
	cmp w8, #1
	b.ne LBB568_1130
	mov w8, #1
LBB568_1007:
	ldr x9, [sp, #88]
	add x8, x9, x8, lsl #3
LBB568_1008:
	ldr x0, [x8]
	cbz x0, LBB568_1292
	ldr x1, [x26, #408]
	cmp x1, #24
	b.ls LBB568_1413
	ldr x8, [x26, #400]
	ldr x8, [x8, #192]
	str x8, [sp, #1992]
	mov w8, #4
	strb w8, [sp, #1984]
	add x8, sp, #1376
	add x1, sp, #1984
	bl luna_core::runtime::table::Table::get
	ldrb w8, [sp, #1376]
	cbz w8, LBB568_1292
	lsr w8, w28, #7
	ldr q0, [sp, #1360]
	str q0, [sp, #1984]
	stur q0, [x20, #16]
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	stur w8, [x29, #-108]
	sturb wzr, [x29, #-112]
Lloh3971:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.744@PAGE
Lloh3972:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.744@PAGEOFF
	add x0, sp, #560
	add x2, sp, #1376
	add x3, sp, #1984
	sub x5, x29, #112
	mov x1, x26
	mov w4, #2
	mov w7, #4
	bl luna_core::vm::exec::Vm::begin_meta_call
	ldrb w8, [sp, #560]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1267
LBB568_1012:
	cbz w10, LBB568_1023
	cmp w10, #1
	b.ne LBB568_1131
	mov w10, #1
	b LBB568_1023
LBB568_1015:
	ldr w0, [x8, #20]
	ldr x8, [sp, #216]
	ldr x8, [x8, #1616]
	cbz x8, LBB568_1122
	cbz x21, LBB568_1148
	cmp x8, x21
	b.eq LBB568_1123
LBB568_1018:
	ldr x1, [x21, #128]
	cmp x1, x0
	b.ls LBB568_1432
	add x8, x21, #120
	b LBB568_1150
LBB568_1020:
	cbnz w27, LBB568_1083
	mov x8, x27
	b LBB568_1084
LBB568_1022:
	mov w10, #3
LBB568_1023:
	ldr x8, [sp, #88]
	add x8, x8, x10, lsl #3
LBB568_1024:
	ldr x0, [x8]
	cbz x0, LBB568_1294
	ldr x1, [x26, #408]
	cmp x1, #23
	b.ls LBB568_1415
	ldr x8, [x26, #400]
	ldr x8, [x8, #184]
	str x8, [sp, #1992]
	mov w8, #4
	strb w8, [sp, #1984]
	add x8, sp, #1344
	add x1, sp, #1984
	bl luna_core::runtime::table::Table::get
	ldrb w8, [sp, #1344]
	cbz w8, LBB568_1294
	lsr w8, w28, #7
	ldr q0, [sp, #1328]
	str q0, [sp, #1984]
	stur q0, [x20, #16]
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	stur w8, [x29, #-108]
	sturb wzr, [x29, #-112]
Lloh3973:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.743@PAGE
Lloh3974:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.743@PAGEOFF
	add x0, sp, #560
	add x2, sp, #1344
	add x3, sp, #1984
	sub x5, x29, #112
LBB568_1028:
	mov x1, x26
	mov w4, #2
	mov w7, #3
	bl luna_core::vm::exec::Vm::begin_meta_call
	ldrb w8, [sp, #560]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1267
LBB568_1029:
	ldr d0, [sp, #296]
	ldr x8, [sp, #232]
	ldr x8, [x8]
LBB568_1030:
	lsr w9, w28, #7
	fneg d0, d0
LBB568_1031:
	ldr x10, [sp, #224]
	add w9, w10, w9, uxtb
LBB568_1032:
	add x8, x8, w9, uxtw #4
LBB568_1033:
	mov w9, #3
	strb w9, [x8]
	str d0, [x8, #8]
	b LBB568_2
LBB568_1034:
	cbz w11, LBB568_1116
	cmp w11, #1
	b.ne LBB568_1117
	mov x11, x12
LBB568_1037:
	and w10, w10, #0x1
	and w11, w11, #0x1
	cmp w11, w10
	b.eq LBB568_2
	b LBB568_1118
LBB568_1038:
	fcmp d0, d1
	cset w8, ls
	cset w9, ge
	fcmp d2, #0.0
	csel w8, w8, w9, gt
	tbz w8, #0, LBB568_1167
	mov w8, #3
	ldp x10, x9, [sp, #200]
	strb w8, [x10]
	str d0, [x10, #8]
	strb w8, [x9]
	str d1, [x9, #8]
	ldr x9, [sp, #224]
	strb w8, [x9]
	str d2, [x9, #8]
	add w9, w25, #3
	add x9, x26, w9, uxtw #4
	strb w8, [x9]
	str d0, [x9, #8]
	ldr x26, [sp, #216]
	b LBB568_2
LBB568_1040:
	mov w11, #0
	ldr x10, [x27, #224]
	str x9, [x10]
	mov w9, #96
LBB568_1041:
	ldr x12, [sp, #216]
	ldr x10, [x12, #720]
	add x10, x10, #1
	str x10, [x12, #720]
	mov w10, #1
	strb w10, [x27, #276]
	cmp x22, x26
	b.eq LBB568_1043
	ldr x10, [x26, #184]
	mov x8, x21
	mov x12, #9223372036854775807
	cmp x10, x12
	b.hs LBB568_1359
LBB568_1043:
	str x28, [sp, #128]
	ldr x10, [x27, #160]
	cbnz x10, LBB568_1353
	and w10, w11, #0x1f
	orr w9, w10, w9
	ldr x8, [x8]
	str x8, [sp, #160]
	mov x8, #-1
	str x8, [x27, #160]
	mov x28, x27
	ldp x0, x1, [x28, #200]!
	mov x19, x9
	str w9, [sp, #1984]
	add x2, sp, #1984
	bl core::hash::BuildHasher::hash_one
	mov x2, x28
	mov x28, x0
	ldur x8, [x2, #-16]
	cbz x8, LBB568_1248
LBB568_1045:
	mov x13, #0
	mov x12, #0
	ldp x8, x10, [x27, #168]
	lsr x9, x28, #57
	dup.8b v0, w9
	and x14, x28, x10
	ldr d1, [x8, x14]
	cmeq.8b v2, v1, v0
	fmov x15, d2
	ands x15, x15, #0x8080808080808080
	b.eq LBB568_1048
LBB568_1046:
	rbit x16, x15
	clz x16, x16
	add x16, x14, x16, lsr #3
	and x16, x16, x10
	sub x17, x8, x16, lsl #3
	ldur w17, [x17, #-8]
	cmp w19, w17
	b.eq LBB568_1055
	sub x16, x15, #1
	ands x15, x16, x15
	b.ne LBB568_1046
LBB568_1048:
	cmp x13, #1
	b.eq LBB568_1051
	cmlt.8b v2, v1, #0
	fmov x11, d2
	cbz x11, LBB568_1053
	rbit x11, x11
	clz x11, x11
	add x11, x14, x11, lsr #3
	and x11, x11, x10
LBB568_1051:
	movi.2d v2, #0xffffffffffffffff
	cmeq.8b v1, v1, v2
	umaxv.8b b1, v1
	fmov w13, s1
	tbnz w13, #0, LBB568_1056
	mov w13, #1
	b LBB568_1054
LBB568_1053:
	mov x13, #0
LBB568_1054:
	add x12, x12, #8
	add x28, x12, x14
	and x14, x28, x10
	ldr d1, [x8, x14]
	cmeq.8b v2, v1, v0
	fmov x15, d2
	ands x15, x15, #0x8080808080808080
	b.ne LBB568_1046
	b LBB568_1048
LBB568_1055:
	neg x9, x16
	b LBB568_1058
LBB568_1056:
	ldrsb w12, [x8, x11]
	tbz w12, #31, LBB568_1252
LBB568_1057:
	and x12, x12, #0x1
	ldr x13, [x27, #184]
	sub x12, x13, x12
	str x12, [x27, #184]
	sub x12, x11, #8
	and x10, x12, x10
	strb w9, [x8, x11]
	add x10, x8, x10
	strb w9, [x10, #8]
	ldr x9, [x27, #192]
	add x9, x9, #1
	str x9, [x27, #192]
	neg x9, x11
	sub x10, x8, x11, lsl #3
	stur w19, [x10, #-8]
LBB568_1058:
	ldr x10, [sp, #160]
	add x8, x8, x9, lsl #3
	stur w10, [x8, #-4]
	ldr x8, [x27, #160]
	add x8, x8, #1
	str x8, [x27, #160]
	ldr x28, [sp, #128]
LBB568_1059:
	ldr x8, [x22, #184]
	sub x8, x8, #1
	str x8, [x22, #184]
LBB568_1060:
	ldr x8, [x26, #184]
	cbnz x8, LBB568_1334
	mov x8, #-1
	str x8, [x26, #184]
	add x8, sp, #1984
	add x0, x8, #16
	add x1, sp, #560
	mov w2, #272
	bl _memcpy
	mov w8, #1
	str x8, [sp, #1984]
	str x8, [sp, #1992]
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov w0, #288
	mov w1, #8
	bl __rustc::__rust_alloc
	cbz x0, LBB568_1332
	mov x22, x0
	add x1, sp, #1984
	mov w2, #288
	bl _memcpy
	str x22, [sp, #1984]
	ldr x19, [x21]
	ldr x8, [x20]
	cmp x19, x8
	b.ne LBB568_1064
	mov x0, x20
	bl alloc::raw_vec::RawVec<T,A>::grow_one
LBB568_1064:
	ldr x8, [x26, #200]
	str x22, [x8, x19, lsl #3]
	add x8, x19, #1
	str x8, [x26, #208]
	ldr x8, [x26, #184]
	add x8, x8, #1
	str x8, [x26, #184]
	ldr x26, [sp, #216]
	ldr x8, [x26, #680]
	add x8, x8, #1
	str x8, [x26, #680]
	ldr x1, [x24, #16]
	cbnz x1, LBB568_38
	b LBB568_39
LBB568_1065:
	sturb w8, [x29, #-112]
	ldur x8, [x29, #-128]
	ldp x9, x12, [sp, #64]
	str x8, [x9]
	ldur x8, [x10, #231]
	stur x8, [x9, #7]
	lsr w8, w28, #7
	ldr x9, [sp, #224]
	add w8, w9, w8, uxtb
	strb w21, [sp, #1984]
	ldr w9, [sp, #1744]
	str w9, [x12]
	ldur w9, [x24, #3]
	stur w9, [x12, #3]
	str x19, [sp, #1992]
	strb w23, [sp, #2000]
	ldur w9, [x29, #-160]
	stur w9, [x11, #17]
	ldur w9, [x10, #195]
	stur w9, [x11, #20]
	str x22, [sp, #2008]
	str w8, [sp, #292]
	strb wzr, [sp, #288]
Lloh3975:
	adrp x6, l_anon.89dbc2968085ea1691689a13183de4a7.736@PAGE
Lloh3976:
	add x6, x6, l_anon.89dbc2968085ea1691689a13183de4a7.736@PAGEOFF
LBB568_1066:
	add x0, sp, #560
	sub x2, x29, #112
	add x3, sp, #1984
	add x5, sp, #288
	mov x1, x26
	mov w4, #2
	mov w7, #3
	bl luna_core::vm::exec::Vm::begin_meta_call
	ldrb w8, [sp, #560]
	cmp w8, #11
	b.eq LBB568_2
	b LBB568_1268
LBB568_1067:
	ldr x0, [x28, #16]
	ldr q0, [sp, #288]
	str q0, [sp, #560]
	ldr x8, [sp, #304]
	str x8, [sp, #576]
	add x8, sp, #1984
	add x2, sp, #560
	mov x1, x22
	mov w3, #0
	bl luna_core::jit::trace_types::TraceRecord::start
	mov w0, #8
	mov w1, #152
	bl alloc::boxed::box_new_uninit
	mov x22, x0
	ldr q0, [sp, #2080]
	str q0, [x0, #96]
	ldr q0, [sp, #2096]
	str q0, [x0, #112]
	ldr q0, [sp, #2112]
	str q0, [x0, #128]
	ldr x8, [sp, #2128]
	str x8, [x0, #144]
	ldr q0, [sp, #2016]
	str q0, [x0, #32]
	ldr q0, [sp, #2032]
	str q0, [x0, #48]
	ldr q0, [sp, #2048]
	str q0, [x0, #64]
	ldr q0, [sp, #2064]
	str q0, [x0, #80]
	ldr q0, [sp, #1984]
	str q0, [x0]
	ldr q0, [sp, #2000]
	str q0, [x0, #16]
	ldr x0, [x26, #960]
	bl core::ptr::drop_in_place<core::option::Option<alloc::boxed::Box<luna_core::jit::trace_types::TraceRecord>>>
	str x22, [x26, #960]
	ldr x8, [x26, #336]
	sub x8, x8, #1
	str x8, [x26, #968]
LBB568_1069:
	ldp x9, x8, [x26, #328]
	add x9, x9, x8, lsl #6
	ldr x10, [x9, #-64]!
	cmp x8, #0
	csel x8, xzr, x9, eq
	cmp x10, #1
	b.eq LBB568_1335
	mov w9, #-16777215
	ldr w10, [x8, #36]
	add w9, w19, w9
	add w9, w9, w10
	str w9, [x8, #36]
	b LBB568_2
LBB568_1071:
	add x0, sp, #1984
	mov x1, x24
	bl alloc::raw_vec::RawVecInner<A>::grow_exact
	mov x8, #-9223372036854775807
	cmp x0, x8
	b.ne LBB568_1431
	ldr x20, [sp, #1984]
	ldr x21, [x23, #72]
	cbnz x21, LBB568_320
	b LBB568_971
LBB568_1073:
	cmp w27, #9
	b.ne LBB568_1330
	mov x25, x20
	add x8, x8, #16
	b LBB568_1085
LBB568_1075:
	ldr x9, [x10, #56]
	cbz x9, LBB568_679
	sub w8, w21, #1
	ldr x11, [x10, #48]
	lsl x12, x9, #5
	add x9, x11, x12
	sub x9, x9, #12
	neg x11, x12
	b LBB568_1078
LBB568_1077:
	sub x9, x9, #32
	adds x11, x11, #32
	b.eq LBB568_679
LBB568_1078:
	ldrb w12, [x9, #4]
	cmp w12, w8, uxtb
	b.ne LBB568_1077
	ldr w12, [x9]
	and w12, w12, #0x7f
	cmp w12, #47
	b.ne LBB568_1077
	ldur w8, [x9, #-4]
	add w8, w8, #1
	ldur x9, [x9, #-12]
	str x9, [x10, #120]
	str w8, [x10, #128]
	mov w8, #1
	strb w8, [x10, #132]
	add x0, x26, #536
Lloh3977:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.959@PAGE
Lloh3978:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.959@PAGEOFF
	mov w2, #26
	bl luna_core::vm::jit_state::JitCounters::bump_close_cause
	b LBB568_16
LBB568_1081:
	mov w8, #3
	b LBB568_1084
LBB568_1082:
	mov x25, x20
	add x8, x8, #152
	b LBB568_1085
LBB568_1083:
	mov w8, #1
LBB568_1084:
	mov x25, x20
	ldr x9, [sp, #88]
	add x8, x9, x8, lsl #3
LBB568_1085:
	ldr x0, [x8]
	ldr x20, [sp, #216]
	cbz x0, LBB568_1331
	ldr x1, [x20, #408]
	cmp x1, #2
	b.ls LBB568_1382
	ldr x8, [x20, #400]
	ldr x8, [x8, #16]
	str x8, [sp, #1992]
	mov w8, #4
	strb w8, [sp, #1984]
	add x8, sp, #560
	add x1, sp, #1984
	bl luna_core::runtime::table::Table::get
	ldrb w8, [sp, #560]
	cbz w8, LBB568_1331
	mov w21, #0
	ldrb w8, [x20, #1840]
	mov w9, #200
	mov w10, #15
	cmp w8, #4
	csel w28, w10, w9, hi
	ldr x8, [sp, #224]
	add w8, w24, w8
	add w22, w8, w19
	add w19, w22, #1
	mov x20, x25
	add w25, w26, #1
LBB568_1089:
	cmp w27, #4
	b.gt LBB568_1091
	mov w8, w27
	sub w9, w27, #2
	cmp w9, #2
	mov w9, #2
	mov w10, #3
	csel x9, x9, x10, lo
	cmp w27, #0
	csinc x8, x8, xzr, eq
	cmp w27, #1
	csel x8, x9, x8, gt
	b LBB568_1094
LBB568_1091:
	ldr x8, [sp, #1704]
	cmp w27, #7
	b.gt LBB568_1112
	sub w9, w27, #6
	cmp w9, #2
	b.hs LBB568_1114
	mov w8, #4
LBB568_1094:
	ldr x9, [sp, #88]
	add x8, x9, x8, lsl #3
LBB568_1095:
	ldr x0, [x8]
	cbz x0, LBB568_1260
	ldr x8, [sp, #216]
	ldr x1, [x8, #408]
	cmp x1, #2
	b.ls LBB568_1382
	ldr x8, [x8, #400]
	ldr x8, [x8, #16]
	str x8, [sp, #1992]
	mov w8, #4
	strb w8, [sp, #1984]
	add x8, sp, #1712
	add x1, sp, #1984
	bl luna_core::runtime::table::Table::get
	ldrb w8, [sp, #1712]
	cbz w8, LBB568_1260
	cmp w21, w28
	b.eq LBB568_1266
	mov w19, w19
	ldr x8, [sp, #216]
	ldr x27, [x8, #312]
	add w8, w24, w25
	cmp x27, x8
	b.hi LBB568_1106
	sub x8, x8, x27
	add x25, x8, #1
	ldr x8, [sp, #216]
	ldr x8, [x8, #296]
	sub x8, x8, x27
	cmp x25, x8
	b.hi LBB568_1115
	mov x8, x27
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x27, lsl #4
	cmp x25, #2
	b.lo LBB568_1105
LBB568_1102:
	sub x10, x19, x27
LBB568_1103:
	strb wzr, [x9], #16
	subs x10, x10, #1
	b.ne LBB568_1103
	add x8, x25, x8
	sub x8, x8, #1
LBB568_1105:
	strb wzr, [x9]
	add x8, x8, #1
	ldr x9, [sp, #216]
	str x8, [x9, #312]
	add w25, w26, #1
LBB568_1106:
	mov w8, w24
	add x9, x8, #1
	add w21, w21, #1
	mov x8, x22
LBB568_1107:
	mov w0, w8
	ldr x8, [sp, #216]
	ldr x1, [x8, #312]
	cmp x1, x0
	b.ls LBB568_1357
	add w8, w0, #1
	cmp x1, x8
	b.ls LBB568_1358
	ldr x10, [sp, #232]
	ldr x10, [x10]
	ldr q0, [x10, x0, lsl #4]
	str q0, [x10, x8, lsl #4]
	sub w8, w0, #1
	subs x9, x9, #1
	b.ne LBB568_1107
	ldr x9, [sp, #216]
	ldr x1, [x9, #312]
	cmp x1, x26
	b.ls LBB568_1385
	ldr x8, [x9, #304]
	ldr q0, [sp, #1712]
	str q0, [x8, x26, lsl #4]
	add w24, w24, #1
	add w8, w24, w25
	str w8, [x9, #1792]
	str q0, [sp, #1696]
	ldrb w27, [sp, #1696]
	and w8, w27, #0xe
	add w19, w19, #1
	add w22, w22, #1
	cmp w8, #6
	b.ne LBB568_1089
	b LBB568_303
LBB568_1112:
	cmp w27, #9
	b.ne LBB568_1260
	add x8, x8, #16
	b LBB568_1095
LBB568_1114:
	add x8, x8, #152
	b LBB568_1095
LBB568_1115:
	str w20, [sp, #224]
	ldr x20, [sp, #216]
	add x0, x20, #296
	mov x1, x27
	mov x2, x25
	mov w3, #8
	mov w4, #16
	bl alloc::raw_vec::RawVecInner<A>::reserve::do_reserve_and_handle
	ldr x8, [x20, #312]
	ldr w20, [sp, #224]
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x8, lsl #4
	cmp x25, #2
	b.hs LBB568_1102
	b LBB568_1105
LBB568_1116:
	tbz w10, #0, LBB568_2
	b LBB568_1118
LBB568_1117:
	tbnz w10, #0, LBB568_2
LBB568_1118:
	add x8, x8, x9, lsl #6
	ldr x10, [x8, #-64]!
	cmp x9, #0
	csel x8, xzr, x8, eq
	cmp x10, #1
	b.ne LBB568_392
	b LBB568_1336
LBB568_1119:
	cmp x23, x22
	b.hs LBB568_1433
	ldrb w8, [x24, #16]
	ldur x9, [x24, #17]
	str x9, [sp, #1088]
	ldr x9, [x24, #24]
	add x10, sp, #1088
	stur x9, [x10, #7]
	lsr w9, w28, #7
	ldr x10, [sp, #224]
	add w0, w10, w9, uxtb
	ldr x1, [x26, #312]
	cmp x1, x0
	b.ls LBB568_1417
LBB568_1121:
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x0, lsl #4
	strb w8, [x9]
	ldr x8, [sp, #1088]
	stur x8, [x9, #1]
	add x8, sp, #1088
	ldur x8, [x8, #7]
	str x8, [x9, #8]
	b LBB568_2
LBB568_1122:
	cbnz x21, LBB568_1018
LBB568_1123:
	ldr x8, [sp, #216]
	ldr x1, [x8, #312]
	add x8, x8, #304
	cmp x1, x0
	b.hi LBB568_1150
	b LBB568_1438
LBB568_1124:
	cmp w8, #9
	b.ne LBB568_1292
	add x8, x9, #16
	b LBB568_1008
LBB568_1126:
	cmp w10, #9
	b.ne LBB568_1294
	add x8, x9, #16
	b LBB568_1024
LBB568_1128:
	cmp x8, #0
	b.gt LBB568_1170
	b LBB568_754
LBB568_1129:
	fcmp d1, d0
	b.lt LBB568_754
	b LBB568_1170
LBB568_1130:
	add x8, x9, #152
	b LBB568_1008
LBB568_1131:
	add x8, x9, #152
	b LBB568_1024
LBB568_1132:
	tbz w13, #0, LBB568_1166
	fmov d0, x10
	fcmp d0, d0
	b.vs LBB568_1167
	cmp x8, #0
	b.le LBB568_1201
	mov x10, #4890909195324358656
	fmov d1, x10
	fcmp d0, d1
	b.ge LBB568_1204
	frintm d1, d0
	mov x10, #-4332462841530417152
	fmov d2, x10
	fcmp d1, d2
	b.mi LBB568_1167
	fcvtms x10, d0
	cmp x9, x10
	b.le LBB568_1205
	b LBB568_1167
LBB568_1138:
	add x0, x26, #48
	bl alloc::raw_vec::RawVec<T,A>::grow_one
	b LBB568_795
LBB568_1139:
	mov w8, #1
	str x22, [sp, #1984]
	str x8, [sp, #1992]
	str xzr, [sp, #2000]
LBB568_1140:
	ldp x9, x8, [sp, #216]
	ldr x24, [x9, #312]
	add x23, x19, x8
	cmp x23, x24
	b.hs LBB568_1427
	ldr x8, [sp, #232]
	ldr x8, [x8]
	add x9, x8, x23, lsl #4
	ldrb w10, [x9]
	cmp w10, #7
	str x28, [sp, #128]
	b.ne LBB568_1143
	ldr x9, [x9, #8]
	ldr x9, [x9, #16]
	str x9, [sp, #176]
	mov w9, #1
	str x9, [sp, #200]
	b LBB568_1144
LBB568_1143:
	str xzr, [sp, #200]
LBB568_1144:
	add x27, x23, #5
	cmp x27, x24
	b.hs LBB568_1225
	add x8, x8, x27, lsl #4
	ldrb w28, [x8]
Lloh3979:
	adrp x9, LJTI568_4@PAGE
Lloh3980:
	add x9, x9, LJTI568_4@PAGEOFF
	adr x10, LBB568_1146
	ldrh w11, [x9, x28, lsl #1]
	add x10, x10, x11, lsl #2
	br x10
LBB568_1146:
	ldr w8, [x8, #8]
	tst w8, #0x1
	mov w8, #1
	cinc w28, w8, ne
	b LBB568_1225
LBB568_1147:
	and w8, w8, #0xf8
	orr w8, w8, #0x4
	strb w8, [x19, #9]
	b LBB568_2
LBB568_1148:
	ldr x10, [sp, #216]
	ldr x8, [x10, #1048]
	mov x9, #-9223372036854775808
	cmp x8, x9
	b.eq LBB568_1369
	ldr x1, [x10, #1064]
	add x8, x10, #1056
	cmp x1, x0
	b.ls LBB568_1440
LBB568_1150:
	mov x20, x2
	mov x19, x3
	ldr x8, [x8]
	add x8, x8, x0, lsl #4
	ldr x21, [x8], #8
LBB568_1151:
	ldr x24, [x8]
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov w0, #40
	mov w1, #8
	bl __rustc::__rust_alloc
	cbz x0, LBB568_1351
	mov w8, #4
	strb w8, [x0, #8]
	mov w9, #1
	str w9, [x0, #16]
	stp x21, x24, [x0, #24]
	ldr x12, [sp, #216]
	ldp x9, x10, [x12, #224]
	str x9, [x0]
	ldrb w9, [x12, #271]
	ldrb w11, [x12, #272]
	cmp w9, #1
	csel w8, w8, w11, eq
	strb w8, [x0, #9]
	add x8, x10, #1
	stp x0, x8, [x12, #224]
	ldr x8, [x12, #240]
	add x8, x8, #40
	str x8, [x12, #240]
	mov x2, x20
	str x0, [x20, x26, lsl #3]
	mov x3, x19
LBB568_1153:
	ldr x26, [sp, #216]
	mov x0, x26
	mov x1, x23
	bl luna_core::runtime::heap::Heap::new_closure_inline
LBB568_1154:
	lsr w8, w28, #7
	ldr x9, [x26, #304]
	ldr x10, [sp, #224]
	add w8, w10, w8, uxtb
	add x9, x9, w8, uxtw #4
	mov w10, #6
	strb w10, [x9]
	str x0, [x9, #8]
	ldrb w9, [x26, #1832]
	tbz w9, #0, LBB568_1158
LBB568_1155:
	ldr x8, [sp, #1984]
	cbz x8, LBB568_2
	lsl x1, x8, #3
	mov x0, x22
LBB568_1157:
	mov w2, #8
	bl __rustc::__rust_dealloc
	b LBB568_2
LBB568_1158:
	ldrb w9, [x26, #268]
	tbnz w9, #0, LBB568_1155
	ldp x9, x10, [x26, #240]
	cmp x9, x10
	b.lo LBB568_1155
	add w8, w8, #1
	str w8, [x26, #1816]
	ldr x8, [x26, #1640]
	cmp x8, #1
	csinc x8, x8, xzr, gt
	mov w9, #400
	udiv x8, x9, x8
	cmp x8, #1
	csinc x8, x8, xzr, hi
	ldr x9, [x26, #232]
	udiv x8, x9, x8
	mov w9, #64000
	cmp x8, x9
	csel x1, x8, x9, hi
	mov x0, x26
	bl luna_core::vm::exec::Vm::gc_step
	cbz w0, LBB568_1155
	ldr x8, [x26, #1632]
	bic x8, x8, x8, asr #63
	ldr x9, [x26, #240]
	umulh x10, x9, x8
	cbnz x10, LBB568_1251
	mul x8, x9, x8
LBB568_1164:
	lsr x8, x8, #2
	mov x9, #62915
	movk x9, #23592, lsl #16
	movk x9, #49807, lsl #32
	movk x9, #10485, lsl #48
	umulh x8, x8, x9
	lsr x8, x8, #2
	mov w9, #1048576
	cmp x8, #256, lsl #12
	csel x8, x8, x9, hi
	str x8, [x26, #248]
	b LBB568_1155
LBB568_1165:
	ldp x26, x12, [sp, #216]
	ldp x15, x13, [sp, #200]
	b LBB568_1208
LBB568_1166:
	cmp x9, x10
	cset w11, gt
	cset w12, lt
	cmp x8, #0
	csel w11, w11, w12, gt
	tbz w11, #0, LBB568_1195
LBB568_1167:
	ldr x26, [sp, #216]
	ldp x9, x8, [x26, #328]
	add x9, x9, x8, lsl #6
	ldr x10, [x9, #-64]!
	cmp x8, #0
	csel x8, xzr, x9, eq
	cmp x10, #1
	b.eq LBB568_1335
	ldr w9, [x8, #36]
	add w9, w9, w14, lsr #15
	str w9, [x8, #36]
	b LBB568_2
LBB568_1169:
	cmp x9, x8
	b.lt LBB568_754
LBB568_1170:
	ldr x8, [x23, #16]
	ldr w9, [x8, #168]
	mov w10, #2147483646
	cmp w9, w10
	b.hi LBB568_754
	add w10, w9, #1
	str w10, [x8, #168]
	cmp w9, #64
	b.ne LBB568_754
	ldr x8, [x26, #960]
	cbnz x8, LBB568_754
	ldr w24, [sp, #284]
	str x23, [sp, #168]
	ldr x8, [x23, #16]
	ldrb w22, [x8, #84]
	add x0, sp, #1984
	mov x1, x22
	mov w2, #0
	mov w3, #1
	mov w4, #1
	bl alloc::raw_vec::RawVecInner<A>::try_allocate_in
	ldr x8, [sp, #1984]
	ldr x0, [sp, #1992]
	cmp x8, #1
	b.eq LBB568_1364
	ldr x8, [sp, #2000]
	stp x0, x8, [sp, #288]
	str xzr, [sp, #304]
	cbz w22, LBB568_1191
	mov x27, #0
	ldr x23, [sp, #224]
	lsl x26, x23, #4
	b LBB568_1177
LBB568_1176:
	ldr x8, [sp, #296]
	strb w20, [x8, x27]
	str x25, [sp, #304]
	add x23, x23, #1
	add x26, x26, #16
	mov x27, x25
	cmp x22, x25
	b.eq LBB568_1191
LBB568_1177:
	ldr x8, [sp, #216]
	ldr x1, [x8, #312]
	cmp x23, x1
	b.hs LBB568_1412
	add x25, x27, #1
	ldr x8, [sp, #232]
	ldr x8, [x8]
	ldrb w20, [x8, x26]
Lloh3981:
	adrp x9, LJTI568_5@PAGE
Lloh3982:
	add x9, x9, LJTI568_5@PAGEOFF
	adr x10, LBB568_1179
	ldrb w11, [x9, x20]
	add x10, x10, x11, lsl #2
	br x10
LBB568_1179:
	add x8, x8, x26
	ldr w8, [x8, #8]
	tst w8, #0x1
	mov w8, #1
	cinc w20, w8, ne
	b LBB568_1189
	mov w20, #5
	b LBB568_1189
	mov w20, #10
	b LBB568_1189
	mov w20, #3
	b LBB568_1189
	mov w20, #4
	b LBB568_1189
	mov w20, #8
	b LBB568_1189
	mov w20, #6
	b LBB568_1189
	mov w20, #7
	b LBB568_1189
	mov w20, #11
	b LBB568_1189
	mov w20, #9
LBB568_1189:
	ldr x8, [sp, #288]
	cmp x27, x8
	b.ne LBB568_1176
	add x0, sp, #288
	bl <alloc::raw_vec::RawVec<u8>>::grow_one
	b LBB568_1176
LBB568_1191:
	sub w8, w24, w28, lsr #15
	add w8, w8, #1
	bic w1, w8, w8, asr #31
	ldr x8, [sp, #168]
	ldr x0, [x8, #16]
	ldr q0, [sp, #288]
	str q0, [sp, #560]
	ldr x8, [sp, #304]
	str x8, [sp, #576]
	add x8, sp, #1984
	add x2, sp, #560
	mov w3, #0
	bl luna_core::jit::trace_types::TraceRecord::start
	mov w0, #8
	mov w1, #152
	bl alloc::boxed::box_new_uninit
	mov x22, x0
	ldr q0, [sp, #2080]
	str q0, [x0, #96]
	ldr q0, [sp, #2096]
	str q0, [x0, #112]
	ldr q0, [sp, #2112]
	str q0, [x0, #128]
	ldr x8, [sp, #2128]
	str x8, [x0, #144]
	ldr q0, [sp, #2016]
	str q0, [x0, #32]
	ldr q0, [sp, #2032]
	str q0, [x0, #48]
	ldr q0, [sp, #2048]
	str q0, [x0, #64]
	ldr q0, [sp, #2064]
	str q0, [x0, #80]
	ldr q0, [sp, #1984]
	str q0, [x0]
	ldr q0, [sp, #2000]
	str q0, [x0, #16]
	ldr x26, [sp, #216]
	ldr x0, [x26, #960]
	bl core::ptr::drop_in_place<core::option::Option<alloc::boxed::Box<luna_core::jit::trace_types::TraceRecord>>>
	str x22, [x26, #960]
	ldr x8, [x26, #336]
	sub x8, x8, #1
	str x8, [x26, #968]
	b LBB568_754
LBB568_1193:
	add x0, x26, #48
	bl alloc::raw_vec::RawVec<T,A>::grow_one
	b LBB568_737
LBB568_1194:
	add x0, sp, #288
	mov x1, #0
	mov w3, #1
	mov w4, #1
	bl alloc::raw_vec::RawVecInner<A>::reserve::do_reserve_and_handle
	b LBB568_58
LBB568_1195:
	b.gt LBB568_1205
	b LBB568_1214
LBB568_1196:
	lsr x11, x8, #63
	mov x10, #9223372036854775807
	b LBB568_1207
LBB568_1197:
	bl std::sync::once_lock::OnceLock<T>::initialize
	b LBB568_151
LBB568_1198:
	add x0, sp, #864
	mov x1, #0
	mov x2, x24
	mov w3, #8
	mov w4, #8
	bl alloc::raw_vec::RawVecInner<A>::reserve::do_reserve_and_handle
	ldr x19, [sp, #880]
	ldr x20, [sp, #872]
	add x0, x20, x19, lsl #3
	cmp x24, #1
	b.ne LBB568_61
	b LBB568_62
LBB568_1200:
	bl std::sync::once_lock::OnceLock<T>::initialize
	b LBB568_574
LBB568_1201:
	mov x10, #-4332462841530417152
	fmov d1, x10
	fcmp d0, d1
	b.ls LBB568_1213
	frintp d1, d0
	mov x10, #4890909195324358656
	fmov d2, x10
	fcmp d1, d2
	b.ge LBB568_1167
	fcvtps x10, d0
	cmp x9, x10
	b.ge LBB568_1214
	b LBB568_1167
LBB568_1204:
	mov x10, #9223372036854775807
LBB568_1205:
	sub x10, x10, x9
	udiv x10, x10, x8
	b LBB568_1215
LBB568_1206:
	lsr x10, x8, #63
	eor w11, w10, #0x1
	mov x10, #-9223372036854775808
LBB568_1207:
	cmp w11, #0
	csel x9, xzr, x9, ne
LBB568_1208:
	sub x9, x9, x8
	mov w11, #2
	strb w11, [x15]
	str x9, [x15, #8]
	strb w11, [x13]
	str x10, [x13, #8]
	strb w11, [x12]
	str x8, [x12, #8]
LBB568_1209:
	ldp x9, x8, [x26, #328]
	add x9, x9, x8, lsl #6
	ldr x10, [x9, #-64]!
	cmp x8, #0
	csel x8, xzr, x9, eq
	cmp x10, #1
	b.eq LBB568_1335
	ldr w9, [x8, #36]
	add w9, w9, w14, lsr #15
	sub w9, w9, #1
	str w9, [x8, #36]
	b LBB568_2
LBB568_1211:
	ldr x8, [sp, #216]
	add x0, x8, #296
	mov x1, x26
	mov x2, x22
	mov w3, #8
	mov w4, #16
	bl alloc::raw_vec::RawVecInner<A>::reserve::do_reserve_and_handle
	ldr x8, [sp, #216]
	ldr x8, [x8, #312]
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x8, lsl #4
	cmp x22, #2
	b.hs LBB568_541
	b LBB568_544
LBB568_1213:
	mov x10, #-9223372036854775808
LBB568_1214:
	sub x10, x9, x10
	neg x11, x8
	udiv x10, x10, x11
LBB568_1215:
	mov w11, #2
	ldp x13, x12, [sp, #200]
	strb w11, [x13]
	str x9, [x13, #8]
	strb w11, [x12]
	str x10, [x12, #8]
	ldr x10, [sp, #224]
	strb w11, [x10]
	str x8, [x10, #8]
	add w8, w25, #3
	add x8, x26, w8, uxtw #4
	strb w11, [x8]
	str x9, [x8, #8]
	ldr x26, [sp, #216]
	b LBB568_2
	mov w28, #5
	b LBB568_1225
	mov w28, #10
	b LBB568_1225
	mov w28, #3
	b LBB568_1225
	mov w28, #4
	b LBB568_1225
	mov w28, #8
	b LBB568_1225
	mov w28, #6
	b LBB568_1225
	mov w28, #7
	b LBB568_1225
	mov w28, #11
	b LBB568_1225
	mov w28, #9
LBB568_1225:
	ldr x20, [x25, #16]
	ldr x23, [sp, #1984]
	ldr x26, [sp, #1992]
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov w0, #8192
	mov w1, #8
	bl __rustc::__rust_alloc
	cbz x0, LBB568_1430
	ldr x8, [sp, #128]
	ldr w9, [sp, #208]
	sub w8, w9, w8, lsr #15
	add w8, w8, #1
	bic w8, w8, w8, asr #31
	str w8, [sp, #1920]
	str x23, [sp, #1800]
	str x26, [sp, #1808]
	cmp x27, x24
	cset w8, lo
	mov w9, #256
	str x22, [sp, #1816]
	str x9, [sp, #1824]
	str x0, [sp, #1832]
	str xzr, [sp, #1840]
	strh wzr, [sp, #1926]
	mov w9, #2
	strb w9, [sp, #1928]
	mov w9, #8
	str xzr, [sp, #1848]
	str x9, [sp, #1856]
	str x20, [sp, #1872]
	str xzr, [sp, #1864]
	str xzr, [sp, #1880]
	str xzr, [sp, #1904]
	ldr x9, [sp, #200]
	str x9, [sp, #1784]
	ldr x9, [sp, #176]
	str x9, [sp, #1792]
	strb w8, [sp, #1924]
	strb w28, [sp, #1925]
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov w0, #152
	mov w1, #8
	bl __rustc::__rust_alloc
	cbz x0, LBB568_1361
	mov x22, x0
	add x9, sp, #1744
	ldur q0, [x9, #136]
	ldur q1, [x9, #152]
	stp q0, q1, [x0, #96]
	ldur q0, [x9, #168]
	str q0, [x0, #128]
	ldr x8, [sp, #1928]
	str x8, [x0, #144]
	ldur q0, [x9, #72]
	ldur q1, [x9, #88]
	stp q0, q1, [x0, #32]
	ldur q0, [x9, #104]
	ldur q1, [x9, #120]
	stp q0, q1, [x0, #64]
	ldur q0, [x9, #40]
	ldur q1, [x9, #56]
	stp q0, q1, [x0]
	ldr x26, [sp, #216]
	ldr x23, [x26, #960]
	ldr x28, [sp, #128]
	cbz x23, LBB568_1235
	ldr x1, [x23, #16]
	cbz x1, LBB568_1230
	ldr x0, [x23, #24]
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_1230:
	ldr x8, [x23, #40]
	cbz x8, LBB568_1232
	ldr x0, [x23, #48]
	lsl x1, x8, #5
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_1232:
	ldr x8, [x23, #64]
	cbz x8, LBB568_1234
	ldr x0, [x23, #72]
	lsl x1, x8, #4
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_1234:
	mov x0, x23
	mov w1, #152
	mov w2, #8
	bl __rustc::__rust_dealloc
LBB568_1235:
	str x22, [x26, #960]
	ldr x8, [x26, #336]
	sub x8, x8, #1
	str x8, [x26, #968]
LBB568_1236:
	ldr x8, [x26, #304]
	ldr x9, [sp, #224]
	add w9, w9, w19
	add w9, w9, #2
	add x8, x8, w9, uxtw #4
	strb w21, [x8]
	ldr x9, [sp, #1768]
	stur x9, [x8, #1]
	add x9, sp, #1744
	ldur x9, [x9, #31]
	str x9, [x8, #8]
LBB568_1237:
	ldp x9, x8, [x26, #328]
	add x9, x9, x8, lsl #6
	ldr x10, [x9, #-64]!
	cmp x8, #0
	csel x8, xzr, x9, eq
	cmp x10, #1
	b.eq LBB568_1335
	ldr w9, [x8, #36]
	sub w9, w9, w28, lsr #15
	str w9, [x8, #36]
	b LBB568_2
LBB568_1239:
	add x0, x26, #48
	bl alloc::raw_vec::RawVec<T,A>::grow_one
	b LBB568_403
LBB568_1240:
	ldr x20, [sp, #216]
	add x0, x20, #296
	mov x1, x26
	mov x2, x24
	mov w3, #8
	mov w4, #16
	bl alloc::raw_vec::RawVecInner<A>::reserve::do_reserve_and_handle
	ldr x8, [x20, #312]
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x8, lsl #4
	cmp x24, #2
	b.hs LBB568_587
	b LBB568_590
LBB568_1242:
	fcvtps x10, d0
	b LBB568_1208
LBB568_1243:
	bl std::sync::once_lock::OnceLock<T>::initialize
	b LBB568_517
LBB568_1244:
	add x0, x26, #296
	mov x1, x23
	mov x2, x24
	mov w3, #8
	mov w4, #16
	bl alloc::raw_vec::RawVecInner<A>::reserve::do_reserve_and_handle
	ldr x8, [x26, #312]
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x8, lsl #4
	cmp x24, #2
	b.hs LBB568_460
	b LBB568_463
LBB568_1245:
	ldr x20, [sp, #216]
	add x0, x20, #296
	mov x1, x24
	mov x2, x25
	mov w3, #8
	mov w4, #16
	bl alloc::raw_vec::RawVecInner<A>::reserve::do_reserve_and_handle
	ldr x8, [x20, #312]
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x8, lsl #4
	cmp x25, #2
	b.hs LBB568_766
	b LBB568_769
LBB568_1246:
	bl std::sync::once_lock::OnceLock<T>::initialize
	b LBB568_510
LBB568_1247:
	add x0, x26, #296
	mov x1, x25
	mov x2, x24
	mov w3, #8
	mov w4, #16
	bl alloc::raw_vec::RawVecInner<A>::reserve::do_reserve_and_handle
	ldr x8, [x26, #312]
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x8, lsl #4
	cmp x24, #2
	b.hs LBB568_636
	b LBB568_639
LBB568_1248:
	sub x0, x2, #32
	mov w1, #1
	mov w3, #1
	bl hashbrown::raw::RawTable<T,A>::reserve_rehash
	b LBB568_1045
LBB568_1249:
	add x0, x26, #296
	mov x1, x24
	mov x2, x25
	mov w3, #8
	mov w4, #16
	bl alloc::raw_vec::RawVecInner<A>::reserve::do_reserve_and_handle
	ldr x8, [x26, #312]
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x8, lsl #4
	cmp x25, #2
	b.hs LBB568_667
	b LBB568_670
LBB568_1250:
	add x0, x26, #296
	mov x1, x24
	mov x2, x23
	mov w3, #8
	mov w4, #16
	bl alloc::raw_vec::RawVecInner<A>::reserve::do_reserve_and_handle
	ldr x8, [x26, #312]
	ldr x9, [sp, #232]
	ldr x9, [x9]
	add x9, x9, x8, lsl #4
	cmp x23, #2
	b.hs LBB568_649
	b LBB568_652
LBB568_1251:
	mov x8, #-1
	b LBB568_1164
LBB568_1252:
	ldr d0, [x8]
	cmlt.8b v0, v0, #0
	fmov x11, d0
	rbit x11, x11
	clz x11, x11
	lsr x11, x11, #3
	ldrb w12, [x8, x11]
	b LBB568_1057
LBB568_1253:
	mov x10, #0
	mov x9, #0
	b LBB568_1208
LBB568_1254:
	str xzr, [x26, #280]
	ldrb w8, [x26, #1838]
	tbz w8, #0, LBB568_1257
	mov w8, #0
	mov w9, #1
	strb w9, [x26, #1839]
	ldr x9, [sp, #80]
	b LBB568_1258
LBB568_1256:
Lloh3983:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.1009@PAGE
Lloh3984:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.1009@PAGEOFF
	add x8, sp, #1984
	mov x0, x26
	mov w2, #14
	bl luna_core::vm::exec::Vm::rt_err
	b LBB568_1295
LBB568_1257:
	mov w8, #2
	strb w8, [x26, #1841]
Lloh3985:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.955@PAGE
Lloh3986:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.955@PAGEOFF
	mov x0, x26
	mov w2, #27
	bl luna_core::runtime::heap::Heap::intern
	ldr x9, [sp, #80]
	str x0, [x9, #16]
	mov w8, #4
LBB568_1258:
	strb w8, [x9, #8]
	b LBB568_1297
LBB568_1259:
	str xzr, [x26]
Lloh3987:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.954@PAGE
Lloh3988:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.954@PAGEOFF
	mov x0, x26
	mov w2, #19
	bl luna_core::runtime::heap::Heap::intern
	mov w8, #4
	ldr x9, [sp, #80]
	strb w8, [x9, #8]
	str x0, [x9, #16]
	b LBB568_1297
LBB568_1260:
	add x0, sp, #1984
	add x2, sp, #1696
	ldr x1, [sp, #216]
LBB568_1261:
	bl luna_core::vm::exec::Vm::call_err
	b LBB568_1295
LBB568_1262:
	mov x9, #-9223372036854775807
	cmp x8, x9
	b.ne LBB568_1264
	mov x8, #-9223372036854775808
LBB568_1264:
	ldr q0, [sp, #1728]
LBB568_1265:
	ldr x9, [sp, #80]
	stur q0, [x9, #8]
	b LBB568_1298
LBB568_1266:
Lloh3989:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.769@PAGE
Lloh3990:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.769@PAGEOFF
	add x8, sp, #1984
	ldr x0, [sp, #216]
	mov w2, #23
	bl luna_core::vm::exec::Vm::rt_err
	b LBB568_1295
LBB568_1267:
	ldr q0, [sp, #560]
	b LBB568_1296
LBB568_1268:
	add x9, sp, #560
	ldur x9, [x9, #1]
	stur x9, [x29, #-144]
	ldr x9, [sp, #568]
	add x10, sp, #2320
	stur x9, [x10, #215]
LBB568_1269:
	ldur x9, [x29, #-144]
	ldr x10, [sp, #80]
	stur x9, [x10, #9]
	add x9, sp, #2320
	ldur x9, [x9, #215]
LBB568_1270:
	str x9, [x10, #16]
	strb w8, [x10, #8]
	mov x8, #-9223372036854775808
	str x8, [x10]
	b LBB568_1299
LBB568_1271:
	ldrb w9, [sp, #1985]
	add x10, sp, #1744
	ldur x10, [x10, #242]
	b LBB568_1289
LBB568_1272:
	mov x9, #-9223372036854775807
	cmp x8, x9
	b.ne LBB568_1274
	mov x8, #-9223372036854775808
LBB568_1274:
	ldr q0, [sp, #256]
	b LBB568_1265
LBB568_1275:
Lloh3991:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGE
Lloh3992:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGEOFF
	bl core::cell::panic_already_mutably_borrowed
LBB568_1276:
Lloh3993:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.1029@PAGE
Lloh3994:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.1029@PAGEOFF
Lloh3995:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1030@PAGE
Lloh3996:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1030@PAGEOFF
	mov w1, #127
	bl core::panicking::panic_fmt
LBB568_1277:
	ldr x8, [sp, #1992]
	ldr x9, [sp, #2000]
	ldr x10, [sp, #80]
	stp x8, x9, [x10, #8]
	mov x9, #-9223372036854775808
	str x9, [x10]
	b LBB568_1299
LBB568_1278:
	add x8, sp, #1744
	ldur q0, [x8, #248]
	add x8, sp, #560
	stur q0, [x8, #7]
	b LBB568_1296
LBB568_1279:
	ldr x8, [sp, #576]
	ldr x9, [sp, #80]
	stp x0, x8, [x9, #8]
	b LBB568_1297
LBB568_1280:
	lsr w9, w28, #15
	cbz w9, LBB568_1300
	ldr x8, [x23, #16]
	sub w0, w9, #1
	ldr x1, [x8, #40]
	cmp x1, x0
	b.ls LBB568_1444
	ldr x8, [x8, #32]
	add x8, x8, x0, lsl #4
	ldrb w9, [x8]
	cmp w9, #4
	b.ne LBB568_1300
	ldr x9, [x8, #8]
	ldr w1, [x9, #32]
	add x8, sp, #560
	add x0, x9, #40
	bl <alloc::string::String>::from_utf8_lossy
	ldr x8, [sp, #560]
	mov x9, #-9223372036854775808
	cmp x8, x9
	b.ne LBB568_1328
	ldr x22, [sp, #568]
	ldr x21, [sp, #576]
	add x0, sp, #1984
	mov x1, x21
	mov w2, #0
	mov w3, #1
	mov w4, #1
	bl alloc::raw_vec::RawVecInner<A>::try_allocate_in
	ldr x8, [sp, #1984]
	ldr x23, [sp, #1992]
	cmp x8, #1
	b.eq LBB568_1386
	ldr x24, [sp, #2000]
	cbz x21, LBB568_1287
	mov x0, x24
	mov x1, x22
	mov x2, x21
	bl _memcpy
LBB568_1287:
	stp x23, x24, [sp, #288]
	str x21, [sp, #304]
	b LBB568_1303
LBB568_1288:
	ldrb w9, [sp, #1985]
	ldur x10, [x19, #242]
LBB568_1289:
	ldr x11, [sp, #80]
	stur x10, [x11, #10]
	ldr x10, [sp, #1992]
	str x10, [x11, #16]
	strb w8, [x11, #8]
	strb w9, [x11, #9]
LBB568_1290:
	mov x8, #-9223372036854775808
	str x8, [x11]
	b LBB568_1299
LBB568_1291:
Lloh3997:
	adrp x1, l_anon.89dbc2968085ea1691689a13183de4a7.1017@PAGE
Lloh3998:
	add x1, x1, l_anon.89dbc2968085ea1691689a13183de4a7.1017@PAGEOFF
	add x8, sp, #1984
	mov x0, x26
	mov w2, #30
	bl luna_core::vm::exec::Vm::rt_err
	b LBB568_1295
LBB568_1292:
Lloh3999:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.846@PAGE
Lloh4000:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.846@PAGEOFF
	add x0, sp, #1984
	add x4, sp, #1360
	mov x1, x26
	mov w3, #28
	bl luna_core::vm::exec::Vm::type_err
	b LBB568_1295
LBB568_1293:
	ldr q0, [sp, #1744]
	b LBB568_1296
LBB568_1294:
Lloh4001:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.847@PAGE
Lloh4002:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.847@PAGEOFF
	add x0, sp, #1984
	add x4, sp, #1328
	mov x1, x26
	mov w3, #21
	bl luna_core::vm::exec::Vm::type_err
LBB568_1295:
	ldr q0, [sp, #1984]
LBB568_1296:
	ldr x9, [sp, #80]
	stur q0, [x9, #8]
LBB568_1297:
	mov x8, #-9223372036854775808
LBB568_1298:
	str x8, [x9]
LBB568_1299:
	add sp, sp, #2592
	.cfi_def_cfa wsp, 96
	ldp x29, x30, [sp, #80]
	ldp x20, x19, [sp, #64]
	ldp x22, x21, [sp, #48]
	ldp x24, x23, [sp, #32]
	ldp x26, x25, [sp, #16]
	ldp x28, x27, [sp], #96
	.cfi_def_cfa_offset 0
	.cfi_restore w30
	.cfi_restore w29
	.cfi_restore w19
	.cfi_restore w20
	.cfi_restore w21
	.cfi_restore w22
	.cfi_restore w23
	.cfi_restore w24
	.cfi_restore w25
	.cfi_restore w26
	.cfi_restore w27
	.cfi_restore w28
	ret
LBB568_1300:
	.cfi_restore_state
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov w0, #1
	mov w1, #1
	bl __rustc::__rust_alloc
	cbz x0, LBB568_1443
	mov w8, #63
	strb w8, [x0]
	mov w8, #1
	stp x8, x0, [sp, #288]
LBB568_1302:
	str x8, [sp, #304]
LBB568_1303:
	add x8, sp, #288
Lloh4003:
	adrp x9, <alloc::string::String as core::fmt::Display>::fmt@PAGE
Lloh4004:
	add x9, x9, <alloc::string::String as core::fmt::Display>::fmt@PAGEOFF
	str x8, [sp, #560]
	str x9, [sp, #568]
Lloh4005:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.953@PAGE
Lloh4006:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.953@PAGEOFF
	add x8, sp, #1984
	add x1, sp, #560
	bl alloc::fmt::format::format_inner
	ldr x21, [sp, #1984]
	ldr x22, [sp, #1992]
	ldr x2, [sp, #2000]
	add x8, sp, #1984
	mov x0, x26
	mov x1, x22
	bl luna_core::vm::exec::Vm::rt_err
	ldr q0, [sp, #1984]
	ldr x9, [sp, #80]
	stur q0, [x9, #8]
	mov x8, #-9223372036854775808
	str x8, [x9]
	cbz x21, LBB568_1307
	mov x0, x22
	mov x1, x21
	mov w2, #1
	bl __rustc::__rust_dealloc
LBB568_1307:
	ldr x1, [sp, #288]
	cbz x1, LBB568_1299
	ldr x0, [sp, #296]
	mov w2, #1
	bl __rustc::__rust_dealloc
	b LBB568_1299
LBB568_1309:
	ldr x2, [x26, #312]
	cmp w22, w23
	b.lo LBB568_1365
	cmp x2, x22
	b.lo LBB568_1365
	sub x19, x22, x23
	cmp w22, w23
	b.ne LBB568_1317
	mov x8, #0
	mov w22, #8
	b LBB568_1319
LBB568_1313:
	ldr x2, [x26, #312]
	cmn w23, #5
	b.hi LBB568_1365
	cmp x2, x22
	b.lo LBB568_1365
	ldr x20, [x26, #304]
	sub x19, x22, x23
	lsl x21, x19, #4
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov x0, x21
	mov w1, #8
	bl __rustc::__rust_alloc
	cbz x0, LBB568_1439
	mov x22, x0
	add x1, x20, x23, lsl #4
	mov x2, x21
	bl _memcpy
	str x23, [x26, #312]
	str w23, [x26, #1792]
	ldr x8, [sp, #80]
	stp x19, x22, [x8]
	str x19, [x8, #16]
	b LBB568_1299
LBB568_1317:
	ldr x20, [x26, #304]
	lsl x21, x19, #4
	bl __rustc::__rust_no_alloc_shim_is_unstable_v2
	mov x0, x21
	mov w1, #8
	bl __rustc::__rust_alloc
	cbz x0, LBB568_1439
	mov x22, x0
	add x1, x20, x23, lsl #4
	mov x2, x21
	bl _memcpy
	mov x8, x19
LBB568_1319:
	ldr x9, [sp, #80]
	str x23, [x26, #312]
	str w23, [x26, #1792]
	stp x19, x22, [x9]
	str x8, [x9, #16]
	b LBB568_1299
LBB568_1320:
	ldur x10, [x19, #242]
	ldr x11, [sp, #80]
	stur x10, [x11, #10]
	ldr x10, [sp, #1992]
	str x10, [x11, #16]
	strb w9, [x11, #8]
	strb w8, [x11, #9]
	b LBB568_1290
LBB568_1321:
Lloh4007:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGE
Lloh4008:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGEOFF
	bl core::cell::panic_already_mutably_borrowed
	b LBB568_1442
LBB568_1322:
Lloh4009:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.971@PAGE
Lloh4010:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.971@PAGEOFF
Lloh4011:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.972@PAGE
Lloh4012:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.972@PAGEOFF
	mov w1, #141
	ldp x21, x19, [sp, #200]
	bl core::panicking::panic_fmt
	b LBB568_1442
LBB568_1323:
Lloh4013:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGE
Lloh4014:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGEOFF
	bl core::cell::panic_already_mutably_borrowed
	b LBB568_1442
LBB568_1324:
	mov w0, #8
	mov w1, #152
	ldr x25, [sp, #176]
	bl alloc::alloc::handle_alloc_error
	b LBB568_1442
LBB568_1325:
Lloh4015:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.836@PAGE
Lloh4016:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.836@PAGEOFF
Lloh4017:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.868@PAGE
Lloh4018:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.868@PAGEOFF
	mov w1, #17
	bl core::option::expect_failed
LBB568_1326:
	add x9, sp, #1744
	ldur x9, [x9, #241]
	ldr x10, [sp, #80]
	stur x9, [x10, #9]
	ldr x9, [sp, #1992]
	b LBB568_1270
LBB568_1327:
	ldur w10, [x19, #241]
	ldr x11, [sp, #80]
	stur w10, [x11, #9]
	ldr w10, [sp, #1988]
	str w10, [x11, #12]
	strb w8, [x11, #8]
	str x9, [x11, #16]
	b LBB568_1290
LBB568_1328:
	ldr q0, [sp, #560]
	str q0, [sp, #288]
	ldr x8, [sp, #576]
	b LBB568_1302
LBB568_1329:
	add x0, sp, #1984
	mov x1, x26
	mov x2, x23
	bl luna_core::vm::exec::Vm::take_results
	ldr q0, [sp, #1984]
	ldr x9, [sp, #80]
	str q0, [x9]
	ldr x8, [sp, #2000]
	str x8, [x9, #16]
	b LBB568_1299
LBB568_1330:
	ldr x20, [sp, #216]
LBB568_1331:
	add x0, sp, #1984
	add x2, sp, #1696
	mov x1, x20
	b LBB568_1261
LBB568_1332:
	mov w0, #8
	mov w1, #288
	bl alloc::alloc::handle_alloc_error
	b LBB568_1442
LBB568_1333:
Lloh4019:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.836@PAGE
Lloh4020:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.836@PAGEOFF
Lloh4021:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.841@PAGE
Lloh4022:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.841@PAGEOFF
	mov w1, #17
	bl core::option::expect_failed
LBB568_1334:
Lloh4023:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.937@PAGE
Lloh4024:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.937@PAGEOFF
	bl core::cell::panic_already_borrowed
	b LBB568_1442
LBB568_1335:
Lloh4025:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.1033@PAGE
Lloh4026:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.1033@PAGEOFF
Lloh4027:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1034@PAGE
Lloh4028:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1034@PAGEOFF
	mov w1, #125
	bl core::panicking::panic_fmt
LBB568_1336:
Lloh4029:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.1046@PAGE
Lloh4030:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.1046@PAGEOFF
Lloh4031:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1047@PAGE
Lloh4032:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1047@PAGEOFF
	mov w1, #127
	bl core::panicking::panic_fmt
LBB568_1337:
Lloh4033:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGE
Lloh4034:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGEOFF
	bl core::cell::panic_already_mutably_borrowed
	b LBB568_1442
LBB568_1338:
Lloh4035:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGE
Lloh4036:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGEOFF
	bl core::cell::panic_already_mutably_borrowed
	b LBB568_1442
Lloh4037:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.1027@PAGE
Lloh4038:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.1027@PAGEOFF
Lloh4039:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1028@PAGE
Lloh4040:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1028@PAGEOFF
	mov w1, #137
	bl core::panicking::panic_fmt
LBB568_1340:
	mov w8, #3
	strb w8, [sp, #1984]
Lloh4041:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.1020@PAGE
Lloh4042:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.1020@PAGEOFF
Lloh4043:
	adrp x3, l_anon.89dbc2968085ea1691689a13183de4a7.65@PAGE
Lloh4044:
	add x3, x3, l_anon.89dbc2968085ea1691689a13183de4a7.65@PAGEOFF
Lloh4045:
	adrp x4, l_anon.89dbc2968085ea1691689a13183de4a7.1021@PAGE
Lloh4046:
	add x4, x4, l_anon.89dbc2968085ea1691689a13183de4a7.1021@PAGEOFF
	add x2, sp, #1984
	mov w1, #18
	bl core::result::unwrap_failed
LBB568_1341:
Lloh4047:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.1010@PAGE
Lloh4048:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.1010@PAGEOFF
Lloh4049:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1011@PAGE
Lloh4050:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1011@PAGEOFF
	mov w1, #125
	bl core::panicking::panic_fmt
LBB568_1342:
Lloh4051:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.976@PAGE
Lloh4052:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.976@PAGEOFF
	mov x0, x9
	mov x24, x8
	b LBB568_1344
LBB568_1343:
	mov x0, x24
Lloh4053:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.977@PAGE
Lloh4054:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.977@PAGEOFF
LBB568_1344:
	mov x1, x24
	bl core::panicking::panic_bounds_check
	b LBB568_1442
LBB568_1345:
Lloh4055:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.1057@PAGE
Lloh4056:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.1057@PAGEOFF
Lloh4057:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1058@PAGE
Lloh4058:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1058@PAGEOFF
	mov w1, #129
	bl core::panicking::panic_fmt
LBB568_1346:
Lloh4059:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGE
Lloh4060:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGEOFF
Lloh4061:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1056@PAGE
Lloh4062:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1056@PAGEOFF
	mov w1, #40
	bl core::panicking::panic
LBB568_1347:
Lloh4063:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGE
Lloh4064:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGEOFF
Lloh4065:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1055@PAGE
Lloh4066:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1055@PAGEOFF
	mov w1, #40
	bl core::panicking::panic
LBB568_1348:
Lloh4067:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.989@PAGE
Lloh4068:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.989@PAGEOFF
Lloh4069:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.990@PAGE
Lloh4070:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.990@PAGEOFF
	mov w1, #137
	bl core::panicking::panic_fmt
LBB568_1349:
Lloh4071:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.975@PAGE
Lloh4072:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.975@PAGEOFF
	mov x22, x9
	mov x1, x8
	b LBB568_1380
LBB568_1350:
	mov x22, x24
	mov x1, x24
Lloh4073:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.974@PAGE
Lloh4074:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.974@PAGEOFF
	b LBB568_1380
LBB568_1351:
	mov w0, #8
	mov w1, #40
	bl alloc::alloc::handle_alloc_error
	b LBB568_1442
LBB568_1352:
Lloh4075:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.970@PAGE
Lloh4076:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.970@PAGEOFF
	mov x0, x28
	bl core::panicking::panic_bounds_check
	b LBB568_1442
LBB568_1353:
Lloh4077:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.937@PAGE
Lloh4078:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.937@PAGEOFF
	bl core::cell::panic_already_borrowed
	b LBB568_1442
LBB568_1354:
Lloh4079:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGE
Lloh4080:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGEOFF
Lloh4081:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1052@PAGE
Lloh4082:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1052@PAGEOFF
	mov w1, #40
	bl core::panicking::panic
LBB568_1355:
Lloh4083:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGE
Lloh4084:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGEOFF
Lloh4085:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1054@PAGE
Lloh4086:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1054@PAGEOFF
	mov w1, #40
	bl core::panicking::panic
LBB568_1356:
Lloh4087:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGE
Lloh4088:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGEOFF
Lloh4089:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1053@PAGE
Lloh4090:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1053@PAGEOFF
	mov w1, #40
	bl core::panicking::panic
LBB568_1357:
Lloh4091:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.997@PAGE
Lloh4092:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.997@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1358:
Lloh4093:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.998@PAGE
Lloh4094:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.998@PAGEOFF
	mov x0, x8
	bl core::panicking::panic_bounds_check
LBB568_1359:
Lloh4095:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGE
Lloh4096:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.852@PAGEOFF
	bl core::cell::panic_already_mutably_borrowed
	b LBB568_1442
LBB568_1360:
Lloh4097:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGE
Lloh4098:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.37@PAGEOFF
Lloh4099:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1051@PAGE
Lloh4100:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1051@PAGEOFF
	mov w1, #40
	bl core::panicking::panic
LBB568_1361:
	mov w0, #8
	mov w1, #152
	bl alloc::alloc::handle_alloc_error
	b LBB568_1442
LBB568_1362:
	mov x1, x22
Lloh4101:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.966@PAGE
Lloh4102:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.966@PAGEOFF
	b LBB568_1380
LBB568_1363:
	mov x22, x10
	mov x1, x12
Lloh4103:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.967@PAGE
Lloh4104:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.967@PAGEOFF
	b LBB568_1380
LBB568_1364:
	ldr x1, [sp, #2000]
	bl alloc::raw_vec::handle_error
LBB568_1365:
Lloh4105:
	adrp x3, l_anon.89dbc2968085ea1691689a13183de4a7.842@PAGE
Lloh4106:
	add x3, x3, l_anon.89dbc2968085ea1691689a13183de4a7.842@PAGEOFF
	mov x0, x23
	mov x1, x22
	bl core::slice::index::slice_index_fail
LBB568_1366:
Lloh4107:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1023@PAGE
Lloh4108:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1023@PAGEOFF
	mov x0, x24
	bl core::panicking::panic_bounds_check
LBB568_1367:
Lloh4109:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1019@PAGE
Lloh4110:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1019@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1368:
	mov x1, x8
Lloh4111:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.973@PAGE
Lloh4112:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.973@PAGEOFF
	b LBB568_1380
LBB568_1369:
Lloh4113:
	adrp x0, l_anon.89dbc2968085ea1691689a13183de4a7.797@PAGE
Lloh4114:
	add x0, x0, l_anon.89dbc2968085ea1691689a13183de4a7.797@PAGEOFF
Lloh4115:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1107@PAGE
Lloh4116:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1107@PAGEOFF
	mov w1, #12
	bl core::option::expect_failed
	b LBB568_1442
LBB568_1370:
Lloh4117:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.880@PAGE
Lloh4118:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.880@PAGEOFF
	mov x1, x24
	bl core::panicking::panic_bounds_check
LBB568_1371:
Lloh4119:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.965@PAGE
Lloh4120:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.965@PAGEOFF
	b LBB568_1344
LBB568_1372:
Lloh4121:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.957@PAGE
Lloh4122:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.957@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1373:
Lloh4123:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1624@PAGE
Lloh4124:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1624@PAGEOFF
	mov x0, x23
	ldp x21, x19, [sp, #200]
	bl core::panicking::panic_bounds_check
	b LBB568_1442
LBB568_1374:
	mov x1, x21
Lloh4125:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.968@PAGE
Lloh4126:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.968@PAGEOFF
	b LBB568_1380
LBB568_1375:
Lloh4127:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.994@PAGE
Lloh4128:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.994@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1376:
Lloh4129:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.995@PAGE
Lloh4130:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.995@PAGEOFF
	mov x0, x8
	bl core::panicking::panic_bounds_check
LBB568_1377:
	mov w0, #8
	mov w1, #8192
	bl alloc::raw_vec::handle_error
	b LBB568_1442
LBB568_1378:
Lloh4131:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1624@PAGE
Lloh4132:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1624@PAGEOFF
	bl core::panicking::panic_bounds_check
	b LBB568_1442
LBB568_1379:
	mov x22, x8
Lloh4133:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.969@PAGE
Lloh4134:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.969@PAGEOFF
LBB568_1380:
	ldp x21, x19, [sp, #200]
	mov x0, x22
	ldr x25, [sp, #176]
	bl core::panicking::panic_bounds_check
	b LBB568_1442
LBB568_1381:
	mov w0, #1
	mov x1, x22
	ldp x21, x19, [sp, #200]
	bl alloc::raw_vec::handle_error
	b LBB568_1442
LBB568_1382:
Lloh4135:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1035@PAGE
Lloh4136:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1035@PAGEOFF
	mov w0, #2
	bl core::panicking::panic_bounds_check
LBB568_1383:
Lloh4137:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1018@PAGE
Lloh4138:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1018@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1384:
Lloh4139:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1709@PAGE
Lloh4140:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1709@PAGEOFF
	mov x0, x23
	mov x1, x22
	bl core::panicking::panic_bounds_check
LBB568_1385:
Lloh4141:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.996@PAGE
Lloh4142:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.996@PAGEOFF
	mov x0, x26
	bl core::panicking::panic_bounds_check
LBB568_1386:
	ldr x1, [sp, #2000]
	mov x0, x23
	bl alloc::raw_vec::handle_error
LBB568_1387:
Lloh4143:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1007@PAGE
Lloh4144:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1007@PAGEOFF
LBB568_1388:
	mov x0, x23
	mov x1, x24
	bl core::panicking::panic_bounds_check
	b LBB568_1442
LBB568_1389:
Lloh4145:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.991@PAGE
Lloh4146:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.991@PAGEOFF
	mov x0, x27
	bl core::panicking::panic_bounds_check
	b LBB568_1442
LBB568_1390:
Lloh4147:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.980@PAGE
Lloh4148:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.980@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1391:
Lloh4149:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.982@PAGE
Lloh4150:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.982@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1392:
Lloh4151:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1016@PAGE
Lloh4152:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1016@PAGEOFF
	mov x0, x25
	mov x1, x24
	bl core::panicking::panic_bounds_check
LBB568_1393:
Lloh4153:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.992@PAGE
Lloh4154:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.992@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1394:
Lloh4155:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1111@PAGE
Lloh4156:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1111@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1395:
Lloh4157:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.987@PAGE
Lloh4158:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.987@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1396:
Lloh4159:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.993@PAGE
Lloh4160:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.993@PAGEOFF
	mov x0, x26
	bl core::panicking::panic_bounds_check
LBB568_1397:
Lloh4161:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.983@PAGE
Lloh4162:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.983@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1398:
Lloh4163:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1012@PAGE
Lloh4164:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1012@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1399:
Lloh4165:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.986@PAGE
Lloh4166:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.986@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1400:
Lloh4167:
	adrp x8, l_anon.89dbc2968085ea1691689a13183de4a7.1001@PAGE
Lloh4168:
	add x8, x8, l_anon.89dbc2968085ea1691689a13183de4a7.1001@PAGEOFF
	mov x0, x2
	mov x1, x23
	mov x2, x8
	bl core::panicking::panic_bounds_check
LBB568_1401:
Lloh4169:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1002@PAGE
Lloh4170:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1002@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1402:
Lloh4171:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1005@PAGE
Lloh4172:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1005@PAGEOFF
	mov x0, x8
	bl core::panicking::panic_bounds_check
LBB568_1403:
Lloh4173:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1003@PAGE
Lloh4174:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1003@PAGEOFF
	mov x0, x8
	bl core::panicking::panic_bounds_check
LBB568_1404:
Lloh4175:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1004@PAGE
Lloh4176:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1004@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1405:
Lloh4177:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1000@PAGE
Lloh4178:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1000@PAGEOFF
	mov x0, x22
	mov x1, x23
	bl core::panicking::panic_bounds_check
LBB568_1406:
Lloh4179:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1022@PAGE
Lloh4180:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1022@PAGEOFF
	mov x0, x25
	bl core::panicking::panic_bounds_check
LBB568_1407:
Lloh4181:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.984@PAGE
Lloh4182:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.984@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1408:
Lloh4183:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.981@PAGE
Lloh4184:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.981@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1409:
Lloh4185:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.958@PAGE
Lloh4186:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.958@PAGEOFF
	mov x0, x23
	bl core::panicking::panic_bounds_check
LBB568_1410:
Lloh4187:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.988@PAGE
Lloh4188:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.988@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1411:
Lloh4189:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1008@PAGE
Lloh4190:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1008@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1412:
Lloh4191:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.999@PAGE
Lloh4192:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.999@PAGEOFF
	mov x0, x23
	bl core::panicking::panic_bounds_check
	b LBB568_1442
LBB568_1413:
Lloh4193:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1035@PAGE
Lloh4194:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1035@PAGEOFF
	mov w0, #24
	bl core::panicking::panic_bounds_check
LBB568_1414:
Lloh4195:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.956@PAGE
Lloh4196:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.956@PAGEOFF
	mov x0, x23
	bl core::panicking::panic_bounds_check
LBB568_1415:
Lloh4197:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1035@PAGE
Lloh4198:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1035@PAGEOFF
	mov w0, #23
	bl core::panicking::panic_bounds_check
LBB568_1416:
	mov x0, x11
	b LBB568_1424
LBB568_1417:
Lloh4199:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.985@PAGE
Lloh4200:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.985@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1418:
	mov x1, x3
Lloh4201:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1013@PAGE
Lloh4202:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1013@PAGEOFF
	b LBB568_1437
LBB568_1419:
Lloh4203:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.814@PAGE
Lloh4204:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.814@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1420:
Lloh4205:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.813@PAGE
Lloh4206:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.813@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1421:
	ldrb w8, [x19, #65]
	tbz w8, #0, LBB568_1428
	ldrb w8, [x19, #64]
	ldp x0, x9, [sp, #216]
	add w1, w9, w8
	bl luna_core::vm::exec::Vm::find_or_create_upval
	b LBB568_1429
LBB568_1423:
Lloh4207:
	adrp x8, l_anon.89dbc2968085ea1691689a13183de4a7.964@PAGE
Lloh4208:
	add x8, x8, l_anon.89dbc2968085ea1691689a13183de4a7.964@PAGEOFF
	str x8, [sp, #24]
LBB568_1424:
	ldr x2, [sp, #24]
	bl core::panicking::panic_bounds_check
	b LBB568_1442
LBB568_1425:
Lloh4209:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1025@PAGE
Lloh4210:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1025@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1426:
Lloh4211:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1024@PAGE
Lloh4212:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1024@PAGEOFF
	bl core::panicking::panic_bounds_check
LBB568_1427:
Lloh4213:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1006@PAGE
Lloh4214:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1006@PAGEOFF
	b LBB568_1388
LBB568_1428:
	ldr w8, [x27, #32]
	ldrb w26, [x19, #64]
	cmp x8, x26
	b.ls LBB568_1435
LBB568_1429:
Lloh4215:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1015@PAGE
Lloh4216:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1015@PAGEOFF
	mov w26, #2
	mov w1, #2
	b LBB568_1437
LBB568_1430:
	mov w0, #8
	mov w1, #8192
	bl alloc::raw_vec::handle_error
	b LBB568_1442
LBB568_1431:
	bl alloc::raw_vec::handle_error
	b LBB568_1442
LBB568_1432:
Lloh4217:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1109@PAGE
Lloh4218:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1109@PAGEOFF
	b LBB568_1441
LBB568_1433:
Lloh4219:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1707@PAGE
Lloh4220:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1707@PAGEOFF
	mov x0, x23
	mov x1, x22
	bl core::panicking::panic_bounds_check
LBB568_1434:
	mov w0, #1
	mov x1, x22
	bl alloc::raw_vec::handle_error
LBB568_1435:
	mov x1, x8
LBB568_1436:
Lloh4221:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1014@PAGE
Lloh4222:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1014@PAGEOFF
LBB568_1437:
	mov x0, x26
	bl core::panicking::panic_bounds_check
	b LBB568_1442
LBB568_1438:
Lloh4223:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1106@PAGE
Lloh4224:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1106@PAGEOFF
	b LBB568_1441
LBB568_1439:
	mov w0, #8
	mov x1, x21
	bl alloc::raw_vec::handle_error
LBB568_1440:
Lloh4225:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1108@PAGE
Lloh4226:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1108@PAGEOFF
LBB568_1441:
	bl core::panicking::panic_bounds_check
LBB568_1442:
	brk #0x1
LBB568_1443:
	mov w0, #1
	mov w1, #1
	bl alloc::raw_vec::handle_error
LBB568_1444:
Lloh4227:
	adrp x2, l_anon.89dbc2968085ea1691689a13183de4a7.1026@PAGE
Lloh4228:
	add x2, x2, l_anon.89dbc2968085ea1691689a13183de4a7.1026@PAGEOFF
	bl core::panicking::panic_bounds_check
	mov x20, x0
	ldr x8, [x27, #160]
	add x8, x8, #1
	str x8, [x27, #160]
	b LBB568_1504
	b LBB568_1494
	mov x20, x0
	ldr x1, [sp, #288]
	cbz x1, LBB568_1528
	b LBB568_1531
	mov x20, x0
	add x0, sp, #1984
	bl core::ptr::drop_in_place<luna_core::jit::trace_types::TraceRecord>
	mov x0, x20
	bl __Unwind_Resume
	mov x20, x0
	cbz x21, LBB568_1468
	mov x0, x22
	mov x1, x21
	b LBB568_1460
	b LBB568_1467
	b LBB568_1501
	mov x20, x0
	add x0, sp, #1984
	bl core::ptr::drop_in_place<luna_core::jit::trace_types::TraceRecord>
	mov x0, x20
	bl __Unwind_Resume
	mov x20, x0
	cbz x23, LBB568_1532
	mov x0, x26
	mov x1, x23
	mov w2, #1
	bl __rustc::__rust_dealloc
	mov x0, x20
	bl __Unwind_Resume
	b LBB568_1467
	b LBB568_1467
	mov x20, x0
	cbz x22, LBB568_1468
	mov x0, x23
	mov x1, x22
LBB568_1460:
	mov w2, #1
	bl __rustc::__rust_dealloc
	b LBB568_1468
	b LBB568_1467
	b LBB568_1467
	b LBB568_1501
	b LBB568_1479
	b LBB568_1501
LBB568_1467:
	mov x20, x0
LBB568_1468:
	ldr x1, [sp, #288]
	cbz x1, LBB568_1532
	ldr x0, [sp, #296]
	mov w2, #1
	bl __rustc::__rust_dealloc
	mov x0, x20
	bl __Unwind_Resume
	mov x20, x0
	ldr x8, [x24]
	subs x8, x8, #1
	str x8, [x24]
	b.ne LBB568_1474
	add x0, sp, #560
	bl alloc::rc::Rc<T,A>::drop_slow
	ldr x9, [sp, #200]
	ldr x8, [x9]
	subs x8, x8, #1
	str x8, [x9]
	b.eq LBB568_1475
LBB568_1472:
	ldr x9, [sp, #208]
	ldr x8, [x9]
	subs x8, x8, #1
	str x8, [x9]
	b.ne LBB568_1476
LBB568_1473:
	sub x0, x29, #128
	bl alloc::rc::Rc<T,A>::drop_slow
	ldr x8, [x25]
	subs x8, x8, #1
	str x8, [x25]
	b.ne LBB568_1526
	b LBB568_1477
LBB568_1474:
	ldr x9, [sp, #200]
	ldr x8, [x9]
	subs x8, x8, #1
	str x8, [x9]
	b.ne LBB568_1472
LBB568_1475:
	sub x0, x29, #112
	bl alloc::rc::Rc<T,A>::drop_slow
	ldr x9, [sp, #208]
	ldr x8, [x9]
	subs x8, x8, #1
	str x8, [x9]
	b.eq LBB568_1473
LBB568_1476:
	ldr x8, [x25]
	subs x8, x8, #1
	str x8, [x25]
	b.ne LBB568_1526
LBB568_1477:
	sub x0, x29, #144
	bl alloc::rc::Rc<T,A>::drop_slow
	b LBB568_1526
LBB568_1479:
	mov x20, x0
	ldr x1, [sp, #1984]
	cbz x1, LBB568_1532
	ldr x0, [sp, #1992]
	mov w2, #1
	bl __rustc::__rust_dealloc
	mov x0, x20
	bl __Unwind_Resume
	mov x20, x0
	ldr x8, [x22]
	subs x8, x8, #1
	str x8, [x22]
	b.ne LBB568_1508
	add x0, sp, #1984
	bl alloc::rc::Rc<T,A>::drop_slow
	b LBB568_1508
	b LBB568_1506
	b LBB568_1501
	b LBB568_1510
	mov x20, x0
	cbz x22, LBB568_1495
	mov x0, x24
	mov x1, x22
	b LBB568_1497
	mov x20, x0
	b LBB568_1513
	mov x20, x0
	mov x0, x24
	bl core::ptr::drop_in_place<alloc::boxed::Box<luna_core::jit::trace_types::TraceRecord>>
	mov x0, x20
	bl __Unwind_Resume
	b LBB568_1525
	mov x20, x0
	add x0, sp, #1784
	bl core::ptr::drop_in_place<luna_core::jit::trace_types::TraceRecord>
	mov x0, x20
	bl __Unwind_Resume
	b LBB568_1494
LBB568_1494:
	mov x20, x0
	ldr x1, [sp, #560]
	cbnz x1, LBB568_1496
LBB568_1495:
	ldr x25, [sp, #176]
	b LBB568_1513
LBB568_1496:
	ldr x0, [sp, #568]
LBB568_1497:
	mov w2, #1
	bl __rustc::__rust_dealloc
	ldr x25, [sp, #176]
	b LBB568_1513
	b LBB568_1525
	mov x20, x0
	ldr x25, [sp, #176]
	b LBB568_1513
LBB568_1501:
	mov x20, x0
	ldr x8, [sp, #1984]
	cbz x8, LBB568_1532
	ldr x0, [sp, #1992]
	lsl x1, x8, #3
	mov w2, #8
	bl __rustc::__rust_dealloc
	mov x0, x20
	bl __Unwind_Resume
	mov x20, x0
LBB568_1504:
	ldr x8, [x22, #184]
	sub x8, x8, #1
	str x8, [x22, #184]
	b LBB568_1511
LBB568_1506:
	mov x20, x0
	ldr x8, [x23, #184]
	sub x8, x8, #1
	str x8, [x23, #184]
	b LBB568_1526
	mov x20, x0
	add x8, sp, #1984
	add x0, x8, #16
	bl core::ptr::drop_in_place<luna_core::jit::trace_types::CompiledTrace>
LBB568_1508:
	ldr x8, [x26, #184]
	add x8, x8, #1
	str x8, [x26, #184]
	mov x0, x24
	bl core::ptr::drop_in_place<alloc::boxed::Box<luna_core::jit::trace_types::TraceRecord>>
	mov x0, x20
	bl __Unwind_Resume
LBB568_1510:
	mov x20, x0
LBB568_1511:
	add x0, sp, #560
	bl core::ptr::drop_in_place<luna_core::jit::trace_types::CompiledTrace>
	mov x0, x24
	bl core::ptr::drop_in_place<alloc::boxed::Box<luna_core::jit::trace_types::TraceRecord>>
	mov x0, x20
	bl __Unwind_Resume
	mov x20, x0
	add x0, sp, #1984
	bl core::ptr::drop_in_place<luna_core::jit::trace_types::TraceRecord>
LBB568_1513:
	ldp x21, x19, [sp, #200]
	b LBB568_1516
	mov x20, x0
	mov x0, x24
	bl core::ptr::drop_in_place<alloc::boxed::Box<luna_core::jit::trace_types::TraceRecord>>
	mov x0, x20
	bl __Unwind_Resume
	mov x20, x0
LBB568_1516:
	ldr x9, [sp, #160]
	ldr x8, [x9]
	subs x8, x8, #1
	str x8, [x9]
	b.ne LBB568_1520
	sub x0, x29, #112
	bl alloc::rc::Rc<T,A>::drop_slow
	ldr x8, [x21]
	subs x8, x8, #1
	str x8, [x21]
	b.eq LBB568_1521
LBB568_1518:
	ldr x8, [x19]
	subs x8, x8, #1
	str x8, [x19]
	b.ne LBB568_1522
LBB568_1519:
	sub x0, x29, #144
	bl alloc::rc::Rc<T,A>::drop_slow
	ldr x8, [x25]
	subs x8, x8, #1
	str x8, [x25]
	b.ne LBB568_1526
	b LBB568_1523
LBB568_1520:
	ldr x8, [x21]
	subs x8, x8, #1
	str x8, [x21]
	b.ne LBB568_1518
LBB568_1521:
	sub x0, x29, #128
	bl alloc::rc::Rc<T,A>::drop_slow
	ldr x8, [x19]
	subs x8, x8, #1
	str x8, [x19]
	b.eq LBB568_1519
LBB568_1522:
	ldr x8, [x25]
	subs x8, x8, #1
	str x8, [x25]
	b.ne LBB568_1526
LBB568_1523:
	sub x0, x29, #160
	bl alloc::rc::Rc<T,A>::drop_slow
	b LBB568_1526
LBB568_1525:
	mov x20, x0
LBB568_1526:
	ldr x8, [sp, #864]
	cbnz x8, LBB568_1530
	ldr x1, [sp, #288]
	cbnz x1, LBB568_1531
LBB568_1528:
	ldr x8, [x27]
	subs x8, x8, #1
	str x8, [x27]
	b.ne LBB568_1532
LBB568_1529:
	add x0, sp, #840
	bl alloc::rc::Rc<T,A>::drop_slow
	mov x0, x20
	bl __Unwind_Resume
LBB568_1530:
	ldr x0, [sp, #872]
	lsl x1, x8, #3
	mov w2, #8
	bl __rustc::__rust_dealloc
	ldr x1, [sp, #288]
	cbz x1, LBB568_1528
LBB568_1531:
	ldr x0, [sp, #296]
	mov w2, #1
	bl __rustc::__rust_dealloc
	ldr x8, [x27]
	subs x8, x8, #1
	str x8, [x27]
	b.eq LBB568_1529
LBB568_1532:
	mov x0, x20
	bl __Unwind_Resume
	.loh AdrpAdd	Lloh3857, Lloh3858
	.loh AdrpAdd	Lloh3859, Lloh3860
	.loh AdrpAdd	Lloh3861, Lloh3862
	.loh AdrpAdd	Lloh3863, Lloh3864
	.loh AdrpAdd	Lloh3865, Lloh3866
	.loh AdrpAdd	Lloh3867, Lloh3868
	.loh AdrpAdd	Lloh3871, Lloh3872
	.loh AdrpLdrGot	Lloh3869, Lloh3870
	.loh AdrpAdd	Lloh3873, Lloh3874
	.loh AdrpAdd	Lloh3875, Lloh3876
	.loh AdrpAdd	Lloh3877, Lloh3878
	.loh AdrpAdd	Lloh3879, Lloh3880
	.loh AdrpAdd	Lloh3881, Lloh3882
	.loh AdrpAdd	Lloh3883, Lloh3884
	.loh AdrpAdd	Lloh3885, Lloh3886
	.loh AdrpAdd	Lloh3887, Lloh3888
	.loh AdrpAdd	Lloh3889, Lloh3890
	.loh AdrpAdd	Lloh3899, Lloh3900
	.loh AdrpLdrGot	Lloh3897, Lloh3898
	.loh AdrpLdrGot	Lloh3895, Lloh3896
	.loh AdrpLdrGot	Lloh3893, Lloh3894
	.loh AdrpLdrGot	Lloh3891, Lloh3892
	.loh AdrpAdd	Lloh3901, Lloh3902
	.loh AdrpAdd	Lloh3907, Lloh3908
	.loh AdrpLdrGot	Lloh3905, Lloh3906
	.loh AdrpLdrGot	Lloh3903, Lloh3904
	.loh AdrpAdd	Lloh3909, Lloh3910
	.loh AdrpAdd	Lloh3919, Lloh3920
	.loh AdrpLdrGot	Lloh3917, Lloh3918
	.loh AdrpLdrGot	Lloh3915, Lloh3916
	.loh AdrpLdrGot	Lloh3913, Lloh3914
	.loh AdrpLdrGot	Lloh3911, Lloh3912
	.loh AdrpAdd	Lloh3921, Lloh3922
	.loh AdrpAdd	Lloh3923, Lloh3924
	.loh AdrpAdd	Lloh3925, Lloh3926
	.loh AdrpAdd	Lloh3927, Lloh3928
	.loh AdrpAdd	Lloh3929, Lloh3930
	.loh AdrpAdd	Lloh3931, Lloh3932
	.loh AdrpAdd	Lloh3933, Lloh3934
	.loh AdrpAdd	Lloh3935, Lloh3936
	.loh AdrpAdd	Lloh3937, Lloh3938
	.loh AdrpAdd	Lloh3939, Lloh3940
	.loh AdrpAdd	Lloh3941, Lloh3942
	.loh AdrpAdd	Lloh3943, Lloh3944
	.loh AdrpAdd	Lloh3945, Lloh3946
	.loh AdrpAdd	Lloh3947, Lloh3948
	.loh AdrpAdd	Lloh3949, Lloh3950
	.loh AdrpAdd	Lloh3951, Lloh3952
	.loh AdrpAdd	Lloh3953, Lloh3954
	.loh AdrpAdd	Lloh3955, Lloh3956
	.loh AdrpAdd	Lloh3957, Lloh3958
	.loh AdrpAdd	Lloh3959, Lloh3960
	.loh AdrpAdd	Lloh3961, Lloh3962
	.loh AdrpAdd	Lloh3967, Lloh3968
	.loh AdrpAdd	Lloh3965, Lloh3966
	.loh AdrpAdd	Lloh3963, Lloh3964
	.loh AdrpAdd	Lloh3969, Lloh3970
	.loh AdrpAdd	Lloh3971, Lloh3972
	.loh AdrpAdd	Lloh3973, Lloh3974
	.loh AdrpAdd	Lloh3975, Lloh3976
	.loh AdrpAdd	Lloh3977, Lloh3978
	.loh AdrpAdd	Lloh3979, Lloh3980
	.loh AdrpAdd	Lloh3981, Lloh3982
	.loh AdrpAdd	Lloh3983, Lloh3984
	.loh AdrpAdd	Lloh3985, Lloh3986
	.loh AdrpAdd	Lloh3987, Lloh3988
	.loh AdrpAdd	Lloh3989, Lloh3990
	.loh AdrpAdd	Lloh3991, Lloh3992
	.loh AdrpAdd	Lloh3995, Lloh3996
	.loh AdrpAdd	Lloh3993, Lloh3994
	.loh AdrpAdd	Lloh3997, Lloh3998
	.loh AdrpAdd	Lloh3999, Lloh4000
	.loh AdrpAdd	Lloh4001, Lloh4002
	.loh AdrpAdd	Lloh4005, Lloh4006
	.loh AdrpAdd	Lloh4003, Lloh4004
	.loh AdrpAdd	Lloh4007, Lloh4008
	.loh AdrpAdd	Lloh4011, Lloh4012
	.loh AdrpAdd	Lloh4009, Lloh4010
	.loh AdrpAdd	Lloh4013, Lloh4014
	.loh AdrpAdd	Lloh4017, Lloh4018
	.loh AdrpAdd	Lloh4015, Lloh4016
	.loh AdrpAdd	Lloh4021, Lloh4022
	.loh AdrpAdd	Lloh4019, Lloh4020
	.loh AdrpAdd	Lloh4023, Lloh4024
	.loh AdrpAdd	Lloh4027, Lloh4028
	.loh AdrpAdd	Lloh4025, Lloh4026
	.loh AdrpAdd	Lloh4031, Lloh4032
	.loh AdrpAdd	Lloh4029, Lloh4030
	.loh AdrpAdd	Lloh4033, Lloh4034
	.loh AdrpAdd	Lloh4035, Lloh4036
	.loh AdrpAdd	Lloh4039, Lloh4040
	.loh AdrpAdd	Lloh4037, Lloh4038
	.loh AdrpAdd	Lloh4045, Lloh4046
	.loh AdrpAdd	Lloh4043, Lloh4044
	.loh AdrpAdd	Lloh4041, Lloh4042
	.loh AdrpAdd	Lloh4049, Lloh4050
	.loh AdrpAdd	Lloh4047, Lloh4048
	.loh AdrpAdd	Lloh4051, Lloh4052
	.loh AdrpAdd	Lloh4053, Lloh4054
	.loh AdrpAdd	Lloh4057, Lloh4058
	.loh AdrpAdd	Lloh4055, Lloh4056
	.loh AdrpAdd	Lloh4061, Lloh4062
	.loh AdrpAdd	Lloh4059, Lloh4060
	.loh AdrpAdd	Lloh4065, Lloh4066
	.loh AdrpAdd	Lloh4063, Lloh4064
	.loh AdrpAdd	Lloh4069, Lloh4070
	.loh AdrpAdd	Lloh4067, Lloh4068
	.loh AdrpAdd	Lloh4071, Lloh4072
	.loh AdrpAdd	Lloh4073, Lloh4074
	.loh AdrpAdd	Lloh4075, Lloh4076
	.loh AdrpAdd	Lloh4077, Lloh4078
	.loh AdrpAdd	Lloh4081, Lloh4082
	.loh AdrpAdd	Lloh4079, Lloh4080
	.loh AdrpAdd	Lloh4085, Lloh4086
	.loh AdrpAdd	Lloh4083, Lloh4084
	.loh AdrpAdd	Lloh4089, Lloh4090
	.loh AdrpAdd	Lloh4087, Lloh4088
	.loh AdrpAdd	Lloh4091, Lloh4092
	.loh AdrpAdd	Lloh4093, Lloh4094
	.loh AdrpAdd	Lloh4095, Lloh4096
	.loh AdrpAdd	Lloh4099, Lloh4100
	.loh AdrpAdd	Lloh4097, Lloh4098
	.loh AdrpAdd	Lloh4101, Lloh4102
	.loh AdrpAdd	Lloh4103, Lloh4104
	.loh AdrpAdd	Lloh4105, Lloh4106
	.loh AdrpAdd	Lloh4107, Lloh4108
	.loh AdrpAdd	Lloh4109, Lloh4110
	.loh AdrpAdd	Lloh4111, Lloh4112
	.loh AdrpAdd	Lloh4115, Lloh4116
	.loh AdrpAdd	Lloh4113, Lloh4114
	.loh AdrpAdd	Lloh4117, Lloh4118
	.loh AdrpAdd	Lloh4119, Lloh4120
	.loh AdrpAdd	Lloh4121, Lloh4122
	.loh AdrpAdd	Lloh4123, Lloh4124
	.loh AdrpAdd	Lloh4125, Lloh4126
	.loh AdrpAdd	Lloh4127, Lloh4128
	.loh AdrpAdd	Lloh4129, Lloh4130
	.loh AdrpAdd	Lloh4131, Lloh4132
	.loh AdrpAdd	Lloh4133, Lloh4134
	.loh AdrpAdd	Lloh4135, Lloh4136
	.loh AdrpAdd	Lloh4137, Lloh4138
	.loh AdrpAdd	Lloh4139, Lloh4140
	.loh AdrpAdd	Lloh4141, Lloh4142
	.loh AdrpAdd	Lloh4143, Lloh4144
	.loh AdrpAdd	Lloh4145, Lloh4146
	.loh AdrpAdd	Lloh4147, Lloh4148
	.loh AdrpAdd	Lloh4149, Lloh4150
	.loh AdrpAdd	Lloh4151, Lloh4152
	.loh AdrpAdd	Lloh4153, Lloh4154
	.loh AdrpAdd	Lloh4155, Lloh4156
	.loh AdrpAdd	Lloh4157, Lloh4158
	.loh AdrpAdd	Lloh4159, Lloh4160
	.loh AdrpAdd	Lloh4161, Lloh4162
	.loh AdrpAdd	Lloh4163, Lloh4164
	.loh AdrpAdd	Lloh4165, Lloh4166
	.loh AdrpAdd	Lloh4167, Lloh4168
	.loh AdrpAdd	Lloh4169, Lloh4170
	.loh AdrpAdd	Lloh4171, Lloh4172
	.loh AdrpAdd	Lloh4173, Lloh4174
	.loh AdrpAdd	Lloh4175, Lloh4176
	.loh AdrpAdd	Lloh4177, Lloh4178
	.loh AdrpAdd	Lloh4179, Lloh4180
	.loh AdrpAdd	Lloh4181, Lloh4182
	.loh AdrpAdd	Lloh4183, Lloh4184
	.loh AdrpAdd	Lloh4185, Lloh4186
	.loh AdrpAdd	Lloh4187, Lloh4188
	.loh AdrpAdd	Lloh4189, Lloh4190
	.loh AdrpAdd	Lloh4191, Lloh4192
	.loh AdrpAdd	Lloh4193, Lloh4194
	.loh AdrpAdd	Lloh4195, Lloh4196
	.loh AdrpAdd	Lloh4197, Lloh4198
	.loh AdrpAdd	Lloh4199, Lloh4200
	.loh AdrpAdd	Lloh4201, Lloh4202
	.loh AdrpAdd	Lloh4203, Lloh4204
	.loh AdrpAdd	Lloh4205, Lloh4206
	.loh AdrpAdd	Lloh4207, Lloh4208
	.loh AdrpAdd	Lloh4209, Lloh4210
	.loh AdrpAdd	Lloh4211, Lloh4212
	.loh AdrpAdd	Lloh4213, Lloh4214
	.loh AdrpAdd	Lloh4215, Lloh4216
	.loh AdrpAdd	Lloh4217, Lloh4218
	.loh AdrpAdd	Lloh4219, Lloh4220
	.loh AdrpAdd	Lloh4221, Lloh4222
	.loh AdrpAdd	Lloh4223, Lloh4224
	.loh AdrpAdd	Lloh4225, Lloh4226
	.loh AdrpAdd	Lloh4227, Lloh4228
