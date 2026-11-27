//! Pure PTX 8.0 Tensor Core kernels for Qwen3-0.6B Embedding & Reranker inference.
//!
//! JIT-compiled directly by Windows `nvcuda.dll` (SM >= 8.0; runs natively on RTX 30/40/50)
//! with zero external CUDA Toolkit, NVRTC, or cuBLAS dependencies.

use std::fmt::Write;

pub fn build_module_ptx() -> String {
    let mut ptx = String::with_capacity(96 * 1024);
    ptx.push_str(".version 8.0\n.target sm_80\n.address_size 64\n\n");
    ptx.push_str(KERNEL_DEQUANT_SOA_Q8_F16);
    emit_gemm_f16_128x128_2stage(&mut ptx, "gemm_f16_128x128_2s", false);
    emit_gemm_f16_128x128_2stage(&mut ptx, "gemm_f16_128x128_2s_res", true);
    emit_gemm_f16_64x128_3stage(&mut ptx, "gemm_f16_64x128_3s", false);
    emit_gemm_f16_64x128_3stage(&mut ptx, "gemm_f16_64x128_3s_res", true);
    ptx.push_str(KERNEL_RMSNORM_1024_F16);
    ptx.push_str(KERNEL_QK_NORM_ROPE_F16);
    ptx.push_str(KERNEL_COPY_PREFIX_KV_F16);
    emit_flash_attn_tc_varlen(&mut ptx);
    emit_swiglu_3072_fast(&mut ptx);
    ptx
}

const KERNEL_DEQUANT_SOA_Q8_F16: &str = r#"
.visible .entry dequant_soa_q8_f16(
    .param .u64 pQs,
    .param .u64 pD,
    .param .u64 pOut,
    .param .u32 pN8
) {
    .reg .u64 %rQs, %rD, %rOut, %off;
    .reg .u32 %n8, %idx, %blk, %q0, %q1, %s0;
    .reg .s32 %i0, %i1, %i2, %i3;
    .reg .b16 %dh;
    .reg .f32 %df, %f0, %f1, %f2, %f3;
    .reg .b32 %h0, %h1, %h2, %h3;
    .reg .pred %p0;

    ld.param.u64 %rQs, [pQs];
    ld.param.u64 %rD, [pD];
    ld.param.u64 %rOut, [pOut];
    ld.param.u32 %n8, [pN8];

    mov.u32 %idx, %ctaid.x;
    mov.u32 %s0, %tid.x;
    mad.lo.u32 %idx, %idx, 256, %s0;
    setp.ge.u32 %p0, %idx, %n8;
    @%p0 bra DONE;

    shr.u32 %blk, %idx, 2;
    cvt.u64.u32 %off, %blk;
    shl.b64 %off, %off, 1;
    add.u64 %off, %rD, %off;
    ld.global.nc.u16 %dh, [%off];
    cvt.f32.f16 %df, %dh;

    cvt.u64.u32 %off, %idx;
    shl.b64 %off, %off, 3;
    add.u64 %off, %rQs, %off;
    ld.global.nc.v2.u32 {%q0, %q1}, [%off];

    bfe.s32 %i0, %q0, 0, 8;
    bfe.s32 %i1, %q0, 8, 8;
    bfe.s32 %i2, %q0, 16, 8;
    bfe.s32 %i3, %q0, 24, 8;
    cvt.rn.f32.s32 %f0, %i0; mul.f32 %f0, %f0, %df;
    cvt.rn.f32.s32 %f1, %i1; mul.f32 %f1, %f1, %df;
    cvt.rn.f32.s32 %f2, %i2; mul.f32 %f2, %f2, %df;
    cvt.rn.f32.s32 %f3, %i3; mul.f32 %f3, %f3, %df;
    cvt.rn.f16x2.f32 %h0, %f1, %f0;
    cvt.rn.f16x2.f32 %h1, %f3, %f2;

    bfe.s32 %i0, %q1, 0, 8;
    bfe.s32 %i1, %q1, 8, 8;
    bfe.s32 %i2, %q1, 16, 8;
    bfe.s32 %i3, %q1, 24, 8;
    cvt.rn.f32.s32 %f0, %i0; mul.f32 %f0, %f0, %df;
    cvt.rn.f32.s32 %f1, %i1; mul.f32 %f1, %f1, %df;
    cvt.rn.f32.s32 %f2, %i2; mul.f32 %f2, %f2, %df;
    cvt.rn.f32.s32 %f3, %i3; mul.f32 %f3, %f3, %df;
    cvt.rn.f16x2.f32 %h2, %f1, %f0;
    cvt.rn.f16x2.f32 %h3, %f3, %f2;

    cvt.u64.u32 %off, %idx;
    shl.b64 %off, %off, 4;
    add.u64 %off, %rOut, %off;
    st.global.v4.u32 [%off], {%h0, %h1, %h2, %h3};
DONE:
    ret;
}
"#;

/// Race-free 2-stage `cp.async` + `ldmatrix` + `mma.sync.aligned.m16n8k16` FP16 Tensor Core GEMM.
/// Tile: 128x128x32 in 40960 B static shared memory -> 2 CTAs/SM occupancy (~46+ TFLOPS on RTX 5060).
fn emit_gemm_f16_128x128_2stage(out: &mut String, name: &str, add_res: bool) {
    let res_param = if add_res { "\n    .param .u64 pR," } else { "" };
    let res_load = if add_res { "\n    ld.param.u64 %rR, [pR];" } else { "" };
    let _ = writeln!(
        out,
        r#"
.visible .entry {name}(
    .param .u64 pA,
    .param .u64 pB,
    .param .u64 pC,{res_param}
    .param .u32 pM,
    .param .u32 pN,
    .param .u32 pK
) {{
    .shared .align 16 .b8 smem[40960];

    .reg .u64 %rA, %rB, %rC, %rR, %gA0, %gA1, %gB0, %gB1, %gC, %gR, %off64, %tmp64;
    .reg .u32 %M, %N, %K, %bx, %by, %ltid, %warp, %lane, %wy, %wx;
    .reg .u32 %row_base, %col_base, %k_idx, %stage, %next_stage;
    .reg .u32 %sA_base, %sB_base, %s_off, %s_ptr;
    .reg .u32 %ld_row, %ld_col, %g_row, %g_col, %t1, %t2, %t3, %t4, %t5, %t6;
    .reg .u32 %sA_rd0, %sA_rd1, %sA_rd2, %sA_rd3, %sB_rd0, %sB_rd1;
    .reg .b32 %a<16>, %b<8>, %c<32>, %h0, %h1;
    .reg .pred %p<8>;

    ld.param.u64 %rA, [pA];
    ld.param.u64 %rB, [pB];
    ld.param.u64 %rC, [pC];{res_load}
    ld.param.u32 %M, [pM];
    ld.param.u32 %N, [pN];
    ld.param.u32 %K, [pK];

    mov.u32 %bx, %ctaid.x;
    mov.u32 %by, %ctaid.y;
    mov.u32 %ltid, %tid.x;
    shr.u32 %warp, %ltid, 5;
    and.b32 %lane, %ltid, 31;
    shr.u32 %wy, %warp, 2;
    and.b32 %wx, %warp, 3;

    shl.b32 %col_base, %bx, 7;
    shl.b32 %row_base, %by, 7;"#
    );
    for i in 0..32 {
        let _ = writeln!(out, "    mov.b32 %c{i}, 0;");
    }
    out.push_str(
        r#"
    shr.u32 %ld_row, %ltid, 2;
    and.b32 %t1, %ltid, 3;
    shl.b32 %ld_col, %t1, 3;
    mul.lo.u32 %t2, %ld_row, 80;
    shl.b32 %t3, %ld_col, 1;
    add.u32 %s_off, %t2, %t3;

    and.b32 %t1, %lane, 15;
    shr.u32 %t2, %lane, 4; shl.b32 %t2, %t2, 4;
    shl.b32 %t3, %wy, 6; add.u32 %t3, %t3, %t1;
    mul.lo.u32 %t3, %t3, 80;
    add.u32 %sA_rd0, %t3, %t2;
    add.u32 %sA_rd1, %sA_rd0, 1280;
    add.u32 %sA_rd2, %sA_rd0, 2560;
    add.u32 %sA_rd3, %sA_rd0, 3840;

    shr.u32 %t4, %lane, 4; shl.b32 %t4, %t4, 3;
    and.b32 %t5, %lane, 7; or.b32 %t4, %t4, %t5;
    shr.u32 %t5, %lane, 3; and.b32 %t5, %t5, 1; shl.b32 %t5, %t5, 4;
    shl.b32 %t6, %wx, 5; add.u32 %t4, %t4, %t6;
    mul.lo.u32 %t4, %t4, 80;
    add.u32 %sB_rd0, %t4, %t5;
    add.u32 %sB_rd1, %sB_rd0, 1280;

    add.u32 %g_row, %row_base, %ld_row;
    setp.lt.u32 %p1, %g_row, %M;
    mul.wide.u32 %off64, %g_row, %K;
    cvt.u64.u32 %tmp64, %ld_col;
    add.u64 %off64, %off64, %tmp64; shl.b64 %off64, %off64, 1;
    add.u64 %gA0, %rA, %off64;

    add.u32 %g_row, %g_row, 64;
    setp.lt.u32 %p2, %g_row, %M;
    mul.wide.u32 %off64, %g_row, %K;
    add.u64 %off64, %off64, %tmp64; shl.b64 %off64, %off64, 1;
    add.u64 %gA1, %rA, %off64;

    add.u32 %g_col, %col_base, %ld_row;
    mul.wide.u32 %off64, %g_col, %K;
    add.u64 %off64, %off64, %tmp64; shl.b64 %off64, %off64, 1;
    add.u64 %gB0, %rB, %off64;

    add.u32 %g_col, %g_col, 64;
    mul.wide.u32 %off64, %g_col, %K;
    add.u64 %off64, %off64, %tmp64; shl.b64 %off64, %off64, 1;
    add.u64 %gB1, %rB, %off64;

    // Prefetch Stage 0 (k=0)
    mov.u32 %sA_base, smem;
    add.u32 %sB_base, %sA_base, 10240;
    add.u32 %s_ptr, %sA_base, %s_off;
    @%p1 cp.async.cg.shared.global [%s_ptr], [%gA0], 16;
    @!%p1 st.shared.v4.u32 [%s_ptr], {0, 0, 0, 0};
    @%p2 cp.async.cg.shared.global [%s_ptr + 5120], [%gA1], 16;
    @!%p2 st.shared.v4.u32 [%s_ptr + 5120], {0, 0, 0, 0};
    add.u32 %s_ptr, %sB_base, %s_off;
    cp.async.cg.shared.global [%s_ptr], [%gB0], 16;
    cp.async.cg.shared.global [%s_ptr + 5120], [%gB1], 16;
    cp.async.commit_group;
    add.u64 %gA0, %gA0, 64; add.u64 %gA1, %gA1, 64;
    add.u64 %gB0, %gB0, 64; add.u64 %gB1, %gB1, 64;

    mov.u32 %k_idx, 0;
    mov.u32 %stage, 0;

K_LOOP:
    add.u32 %t1, %k_idx, 32;
    setp.lt.u32 %p3, %t1, %K;
    xor.b32 %next_stage, %stage, 1;
    @!%p3 bra SKIP_PREFETCH;

    mov.u32 %sA_base, smem;
    mul.lo.u32 %t2, %next_stage, 20480;
    add.u32 %sA_base, %sA_base, %t2;
    add.u32 %sB_base, %sA_base, 10240;
    add.u32 %s_ptr, %sA_base, %s_off;
    @%p1 cp.async.cg.shared.global [%s_ptr], [%gA0], 16;
    @!%p1 st.shared.v4.u32 [%s_ptr], {0, 0, 0, 0};
    @%p2 cp.async.cg.shared.global [%s_ptr + 5120], [%gA1], 16;
    @!%p2 st.shared.v4.u32 [%s_ptr + 5120], {0, 0, 0, 0};
    add.u32 %s_ptr, %sB_base, %s_off;
    cp.async.cg.shared.global [%s_ptr], [%gB0], 16;
    cp.async.cg.shared.global [%s_ptr + 5120], [%gB1], 16;
    cp.async.commit_group;
    add.u64 %gA0, %gA0, 64; add.u64 %gA1, %gA1, 64;
    add.u64 %gB0, %gB0, 64; add.u64 %gB1, %gB1, 64;
    cp.async.wait_group 1;
    bra SYNC_COMPUTE;

SKIP_PREFETCH:
    cp.async.wait_group 0;

SYNC_COMPUTE:
    bar.sync 0;

    mov.u32 %sA_base, smem;
    mul.lo.u32 %t2, %stage, 20480;
    add.u32 %sA_base, %sA_base, %t2;
    add.u32 %sB_base, %sA_base, 10240;
"#,
    );
    for k_sub in 0..2 {
        let kb = k_sub * 32;
        for mi in 0..4 {
            let ao = mi * 4;
            let _ = writeln!(
                out,
                "    add.u32 %s_ptr, %sA_base, %sA_rd{mi}; ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%a{ao}, %a{}, %a{}, %a{}}}, [%s_ptr + {kb}];",
                ao + 1, ao + 2, ao + 3
            );
        }
        let _ = writeln!(out, "    add.u32 %s_ptr, %sB_base, %sB_rd0; ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%b0, %b1, %b2, %b3}}, [%s_ptr + {kb}];");
        let _ = writeln!(out, "    add.u32 %s_ptr, %sB_base, %sB_rd1; ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%b4, %b5, %b6, %b7}}, [%s_ptr + {kb}];");
        for mi in 0..4 {
            let ao = mi * 4;
            for ni in 0..4 {
                let bo = ni * 2;
                let ci = (mi * 4 + ni) * 2;
                let _ = writeln!(
                    out,
                    "    mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {{%c{ci}, %c{}}}, {{%a{ao}, %a{}, %a{}, %a{}}}, {{%b{bo}, %b{}}}, {{%c{ci}, %c{}}};",
                    ci + 1, ao + 1, ao + 2, ao + 3, bo + 1, ci + 1
                );
            }
        }
    }
    out.push_str(
        r#"
    mov.u32 %stage, %next_stage;
    add.u32 %k_idx, %k_idx, 32;
    setp.lt.u32 %p4, %k_idx, %K;
    @%p4 bar.sync 0;
    @%p4 bra K_LOOP;

    shr.u32 %t1, %lane, 2;
    and.b32 %t2, %lane, 3; shl.b32 %t2, %t2, 1;
    shl.b32 %t3, %wy, 6; add.u32 %t3, %t3, %row_base; add.u32 %t3, %t3, %t1;
    shl.b32 %t4, %wx, 5; add.u32 %t4, %t4, %col_base; add.u32 %t4, %t4, %t2;
"#,
    );
    for mi in 0..4 {
        for (half, r_off) in [(0usize, mi * 16), (1usize, mi * 16 + 8)] {
            let res_ptr = if add_res { "\n    add.u64 %gR, %rR, %off64;" } else { "" };
            let _ = writeln!(
                out,
                r#"
    add.u32 %g_row, %t3, {r_off};
    setp.lt.u32 %p5, %g_row, %M;
    @!%p5 bra SKIP_ST_{mi}_{half};
    mul.wide.u32 %off64, %g_row, %N;
    cvt.u64.u32 %tmp64, %t4;
    add.u64 %off64, %off64, %tmp64;
    shl.b64 %off64, %off64, 1;
    add.u64 %gC, %rC, %off64;{res_ptr}"#
            );
            for ni in 0..4 {
                let ci = (mi * 4 + ni) * 2 + half;
                let boff = ni * 16;
                if add_res {
                    let _ = writeln!(out, "    ld.global.u32 %h1, [%gR + {boff}];");
                    let _ = writeln!(out, "    add.rn.f16x2 %h0, %c{ci}, %h1;");
                    let _ = writeln!(out, "    st.global.u32 [%gC + {boff}], %h0;");
                } else {
                    let _ = writeln!(out, "    st.global.u32 [%gC + {boff}], %c{ci};");
                }
            }
            let _ = writeln!(out, "SKIP_ST_{mi}_{half}:");
        }
    }
    out.push_str("    ret;\n}\n");
}

/// Race-free 3-stage `cp.async` + `ldmatrix` + `mma.sync.aligned.m16n8k16` FP16 Tensor Core GEMM.
/// Tile: 64x128x32 in 46080 B static shared memory (ideal for small M <= 64).
fn emit_gemm_f16_64x128_3stage(out: &mut String, name: &str, add_res: bool) {
    let bm: u32 = 64;
    let m_tiles: usize = 2;
    let sa_bytes: u32 = bm * 80;
    let sb_bytes: u32 = 128 * 80;
    let stage_bytes: u32 = sa_bytes + sb_bytes;
    let smem_decl = format!(".shared .align 16 .b8 smem[{}];", stage_bytes * 3);
    let smem_sym = "smem";
    let res_param = if add_res { "\n    .param .u64 pR," } else { "" };
    let res_load = if add_res { "\n    ld.param.u64 %rR, [pR];" } else { "" };
    let row_shift = 6;
    let wy_shift = 5;
    let a_regs = m_tiles * 4;
    let c_regs = m_tiles * 4 * 2;

    let _ = writeln!(
        out,
        r#"
.visible .entry {name}(
    .param .u64 pA,
    .param .u64 pB,
    .param .u64 pC,{res_param}
    .param .u32 pM,
    .param .u32 pN,
    .param .u32 pK
) {{
    {smem_decl}

    .reg .u64 %rA, %rB, %rC, %rR, %gA0, %gA1, %gB0, %gB1, %gC, %gR, %off64, %tmp64;
    .reg .u32 %M, %N, %K, %bx, %by, %ltid, %warp, %lane, %wy, %wx;
    .reg .u32 %row_base, %col_base, %k_idx, %cp_stage, %ld_stage;
    .reg .u32 %sA_base, %sB_base, %sA_off, %sB_off, %s_ptr;
    .reg .u32 %ld_row, %ld_col, %g_row, %g_col, %t1, %t2, %t3, %t4, %t5, %t6;
    .reg .u32 %sA_rd0, %sA_rd1, %sA_rd2, %sA_rd3, %sB_rd0, %sB_rd1;
    .reg .b32 %a<{a_regs}>, %b<8>, %c<{c_regs}>, %h0, %h1;
    .reg .pred %p<8>;

    ld.param.u64 %rA, [pA];
    ld.param.u64 %rB, [pB];
    ld.param.u64 %rC, [pC];{res_load}
    ld.param.u32 %M, [pM];
    ld.param.u32 %N, [pN];
    ld.param.u32 %K, [pK];

    mov.u32 %bx, %ctaid.x;
    mov.u32 %by, %ctaid.y;
    mov.u32 %ltid, %tid.x;
    shr.u32 %warp, %ltid, 5;
    and.b32 %lane, %ltid, 31;
    shr.u32 %wy, %warp, 2;
    and.b32 %wx, %warp, 3;

    shl.b32 %col_base, %bx, 7;
    shl.b32 %row_base, %by, {row_shift};"#
    );

    for i in 0..c_regs {
        let _ = writeln!(out, "    mov.b32 %c{i}, 0;");
    }

    let _ = writeln!(
        out,
        r#"
    shr.u32 %ld_row, %ltid, 2;
    and.b32 %t1, %ltid, 3;
    shl.b32 %ld_col, %t1, 3;
    mul.lo.u32 %t2, %ld_row, 80;
    shl.b32 %t3, %ld_col, 1;
    add.u32 %sA_off, %t2, %t3;
    mov.u32 %sB_off, %sA_off;

    and.b32 %t1, %lane, 15;
    shr.u32 %t2, %lane, 4; shl.b32 %t2, %t2, 4;
    shl.b32 %t3, %wy, {wy_shift}; add.u32 %t3, %t3, %t1;
    mul.lo.u32 %t3, %t3, 80;
    add.u32 %sA_rd0, %t3, %t2;
    add.u32 %sA_rd1, %sA_rd0, 1280;
    add.u32 %sA_rd2, %sA_rd0, 2560;
    add.u32 %sA_rd3, %sA_rd0, 3840;

    shr.u32 %t4, %lane, 4; shl.b32 %t4, %t4, 3;
    and.b32 %t5, %lane, 7; or.b32 %t4, %t4, %t5;
    shr.u32 %t5, %lane, 3; and.b32 %t5, %t5, 1; shl.b32 %t5, %t5, 4;
    shl.b32 %t6, %wx, 5; add.u32 %t4, %t4, %t6;
    mul.lo.u32 %t4, %t4, 80;
    add.u32 %sB_rd0, %t4, %t5;
    add.u32 %sB_rd1, %sB_rd0, 1280;

    add.u32 %g_row, %row_base, %ld_row;
    setp.lt.u32 %p1, %g_row, %M;
    mul.wide.u32 %off64, %g_row, %K;
    cvt.u64.u32 %tmp64, %ld_col;
    add.u64 %off64, %off64, %tmp64; shl.b64 %off64, %off64, 1;
    add.u64 %gA0, %rA, %off64;"#
    );

    if bm == 128 {
        out.push_str(
            r#"
    add.u32 %g_row, %g_row, 64;
    setp.lt.u32 %p2, %g_row, %M;
    mul.wide.u32 %off64, %g_row, %K;
    add.u64 %off64, %off64, %tmp64; shl.b64 %off64, %off64, 1;
    add.u64 %gA1, %rA, %off64;"#,
        );
    }

    let _ = writeln!(
        out,
        r#"
    add.u32 %g_col, %col_base, %ld_row;
    mul.wide.u32 %off64, %g_col, %K;
    add.u64 %off64, %off64, %tmp64; shl.b64 %off64, %off64, 1;
    add.u64 %gB0, %rB, %off64;

    add.u32 %g_col, %g_col, 64;
    mul.wide.u32 %off64, %g_col, %K;
    add.u64 %off64, %off64, %tmp64; shl.b64 %off64, %off64, 1;
    add.u64 %gB1, %rB, %off64;"#
    );

    let emit_stage_load = |buf: &mut String| {
        let _ = writeln!(buf, "    add.u32 %sB_base, %sA_base, {sa_bytes};");
        buf.push_str("    add.u32 %s_ptr, %sA_base, %sA_off;\n");
        buf.push_str("    @%p1 cp.async.cg.shared.global [%s_ptr], [%gA0], 16;\n");
        buf.push_str("    @!%p1 st.shared.v4.u32 [%s_ptr], {0, 0, 0, 0};\n");
        if bm == 128 {
            buf.push_str("    @%p2 cp.async.cg.shared.global [%s_ptr + 5120], [%gA1], 16;\n");
            buf.push_str("    @!%p2 st.shared.v4.u32 [%s_ptr + 5120], {0, 0, 0, 0};\n");
        }
        buf.push_str("    add.u32 %s_ptr, %sB_base, %sB_off;\n");
        buf.push_str("    cp.async.cg.shared.global [%s_ptr], [%gB0], 16;\n");
        buf.push_str("    cp.async.cg.shared.global [%s_ptr + 5120], [%gB1], 16;\n");
        buf.push_str("    cp.async.commit_group;\n");
        buf.push_str("    add.u64 %gA0, %gA0, 64;\n");
        if bm == 128 {
            buf.push_str("    add.u64 %gA1, %gA1, 64;\n");
        }
        buf.push_str("    add.u64 %gB0, %gB0, 64; add.u64 %gB1, %gB1, 64;\n");
    };

    // Prefetch Stage 0 (k=0) and Stage 1 (k=32)
    let _ = writeln!(out, "\n    mov.u32 %sA_base, {smem_sym};");
    emit_stage_load(out);
    let _ = writeln!(out, "    add.u32 %sA_base, %sA_base, {stage_bytes};");
    emit_stage_load(out);

    let _ = writeln!(
        out,
        r#"
    cp.async.wait_group 1;
    bar.sync 0;

    mov.u32 %k_idx, 0;
    mov.u32 %cp_stage, 0;
    mov.u32 %ld_stage, 2;

K_LOOP:
    add.u32 %t1, %k_idx, 64;
    setp.lt.u32 %p3, %t1, %K;
    @!%p3 bra DO_MMA;

    mov.u32 %sA_base, {smem_sym};"#
    );
    let _ = writeln!(out, "    mul.lo.u32 %t2, %ld_stage, {stage_bytes};");
    out.push_str("    add.u32 %sA_base, %sA_base, %t2;\n");
    emit_stage_load(out);

    let _ = writeln!(
        out,
        r#"
DO_MMA:
    mov.u32 %sA_base, {smem_sym};
    mul.lo.u32 %t2, %cp_stage, {stage_bytes};
    add.u32 %sA_base, %sA_base, %t2;
    add.u32 %sB_base, %sA_base, {sa_bytes};"#
    );

    for k_sub in 0..2 {
        let kb = k_sub * 32;
        for mi in 0..m_tiles {
            let ao = mi * 4;
            let _ = writeln!(
                out,
                "    add.u32 %s_ptr, %sA_base, %sA_rd{mi}; ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%a{}, %a{}, %a{}, %a{}}}, [%s_ptr + {kb}];",
                ao, ao + 1, ao + 2, ao + 3
            );
        }
        let _ = writeln!(
            out,
            "    add.u32 %s_ptr, %sB_base, %sB_rd0; ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%b0, %b1, %b2, %b3}}, [%s_ptr + {kb}];"
        );
        let _ = writeln!(
            out,
            "    add.u32 %s_ptr, %sB_base, %sB_rd1; ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%b4, %b5, %b6, %b7}}, [%s_ptr + {kb}];"
        );
        for mi in 0..m_tiles {
            let ao = mi * 4;
            for ni in 0..4 {
                let bo = ni * 2;
                let ci = (mi * 4 + ni) * 2;
                let _ = writeln!(
                    out,
                    "    mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {{%c{ci}, %c{}}}, {{%a{ao}, %a{}, %a{}, %a{}}}, {{%b{bo}, %b{}}}, {{%c{ci}, %c{}}};",
                    ci + 1,
                    ao + 1,
                    ao + 2,
                    ao + 3,
                    bo + 1,
                    ci + 1
                );
            }
        }
    }

    let _ = writeln!(
        out,
        r#"
    add.u32 %cp_stage, %cp_stage, 1;
    setp.eq.u32 %p4, %cp_stage, 3;
    selp.u32 %cp_stage, 0, %cp_stage, %p4;
    add.u32 %ld_stage, %ld_stage, 1;
    setp.eq.u32 %p4, %ld_stage, 3;
    selp.u32 %ld_stage, 0, %ld_stage, %p4;

    add.u32 %k_idx, %k_idx, 32;
    setp.lt.u32 %p4, %k_idx, %K;
    @!%p4 bra K_DONE;
    @%p3 cp.async.wait_group 1;
    @!%p3 cp.async.wait_group 0;
    bar.sync 0;
    bra K_LOOP;
K_DONE:

    shr.u32 %t1, %lane, 2;
    and.b32 %t2, %lane, 3; shl.b32 %t2, %t2, 1;
    shl.b32 %t3, %wy, {wy_shift}; add.u32 %t3, %t3, %row_base; add.u32 %t3, %t3, %t1;
    shl.b32 %t4, %wx, 5; add.u32 %t4, %t4, %col_base; add.u32 %t4, %t4, %t2;"#
    );

    for mi in 0..m_tiles {
        for (half, r_off) in [(0usize, mi * 16), (1usize, mi * 16 + 8)] {
            let res_ptr = if add_res {
                "\n    add.u64 %gR, %rR, %off64;"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                r#"
    add.u32 %g_row, %t3, {r_off};
    setp.lt.u32 %p5, %g_row, %M;
    @!%p5 bra SKIP_ST_{mi}_{half};
    mul.wide.u32 %off64, %g_row, %N;
    cvt.u64.u32 %tmp64, %t4;
    add.u64 %off64, %off64, %tmp64;
    shl.b64 %off64, %off64, 1;
    add.u64 %gC, %rC, %off64;{res_ptr}"#
            );
            for ni in 0..4 {
                let ci = (mi * 4 + ni) * 2 + half;
                let boff = ni * 16;
                if add_res {
                    let _ = writeln!(out, "    ld.global.u32 %h1, [%gR + {boff}];");
                    let _ = writeln!(out, "    add.rn.f16x2 %h0, %c{ci}, %h1;");
                    let _ = writeln!(out, "    st.global.u32 [%gC + {boff}], %h0;");
                } else {
                    let _ = writeln!(out, "    st.global.u32 [%gC + {boff}], %c{ci};");
                }
            }
            let _ = writeln!(out, "SKIP_ST_{mi}_{half}:");
        }
    }
    out.push_str("    ret;\n}\n");
}

const KERNEL_RMSNORM_1024_F16: &str = r#"
.visible .entry rmsnorm_1024_f16(
    .param .u64 pX,
    .param .u64 pW,
    .param .u64 pOut,
    .param .f32 pEps
) {
    .shared .align 4 .f32 ssum[8];
    .reg .u64 %rX, %rW, %rOut, %off;
    .reg .u32 %row, %ltid, %warp, %lane, %col, %sbase;
    .reg .b32 %h0, %h1;
    .reg .b16 %a0, %a1, %a2, %a3;
    .reg .f32 %x0, %x1, %x2, %x3, %w0, %w1, %w2, %w3;
    .reg .f32 %sum, %tmp, %eps, %inv;
    .reg .pred %p0;

    ld.param.u64 %rX, [pX];
    ld.param.u64 %rW, [pW];
    ld.param.u64 %rOut, [pOut];
    ld.param.f32 %eps, [pEps];

    mov.u32 %row, %ctaid.x;
    mov.u32 %ltid, %tid.x;
    shr.u32 %warp, %ltid, 5;
    and.b32 %lane, %ltid, 31;
    shl.b32 %col, %ltid, 2;

    mul.wide.u32 %off, %row, 2048;
    add.u64 %rX, %rX, %off;
    add.u64 %rOut, %rOut, %off;
    cvt.u64.u32 %off, %col;
    shl.b64 %off, %off, 1;
    add.u64 %rX, %rX, %off;
    add.u64 %rOut, %rOut, %off;

    ld.global.v2.u32 {%h0, %h1}, [%rX];
    mov.b32 {%a0, %a1}, %h0;
    mov.b32 {%a2, %a3}, %h1;
    cvt.f32.f16 %x0, %a0;
    cvt.f32.f16 %x1, %a1;
    cvt.f32.f16 %x2, %a2;
    cvt.f32.f16 %x3, %a3;

    mul.f32 %sum, %x0, %x0;
    fma.rn.f32 %sum, %x1, %x1, %sum;
    fma.rn.f32 %sum, %x2, %x2, %sum;
    fma.rn.f32 %sum, %x3, %x3, %sum;

    shfl.sync.bfly.b32 %tmp, %sum, 16, 31, 0xffffffff; add.f32 %sum, %sum, %tmp;
    shfl.sync.bfly.b32 %tmp, %sum, 8,  31, 0xffffffff; add.f32 %sum, %sum, %tmp;
    shfl.sync.bfly.b32 %tmp, %sum, 4,  31, 0xffffffff; add.f32 %sum, %sum, %tmp;
    shfl.sync.bfly.b32 %tmp, %sum, 2,  31, 0xffffffff; add.f32 %sum, %sum, %tmp;
    shfl.sync.bfly.b32 %tmp, %sum, 1,  31, 0xffffffff; add.f32 %sum, %sum, %tmp;

    setp.eq.u32 %p0, %lane, 0;
    shl.b32 %col, %warp, 2;
    mov.u32 %sbase, ssum;
    add.u32 %col, %sbase, %col;
    @%p0 st.shared.f32 [%col], %sum;
    bar.sync 0;

    ld.shared.f32 %sum, [ssum + 0];
    ld.shared.f32 %tmp, [ssum + 4];  add.f32 %sum, %sum, %tmp;
    ld.shared.f32 %tmp, [ssum + 8];  add.f32 %sum, %sum, %tmp;
    ld.shared.f32 %tmp, [ssum + 12]; add.f32 %sum, %sum, %tmp;
    ld.shared.f32 %tmp, [ssum + 16]; add.f32 %sum, %sum, %tmp;
    ld.shared.f32 %tmp, [ssum + 20]; add.f32 %sum, %sum, %tmp;
    ld.shared.f32 %tmp, [ssum + 24]; add.f32 %sum, %sum, %tmp;
    ld.shared.f32 %tmp, [ssum + 28]; add.f32 %sum, %sum, %tmp;

    mul.f32 %sum, %sum, 0.0009765625;
    add.f32 %sum, %sum, %eps;
    rsqrt.approx.f32 %inv, %sum;

    shl.b32 %col, %ltid, 2;
    cvt.u64.u32 %off, %col;
    shl.b64 %off, %off, 2;
    add.u64 %rW, %rW, %off;
    ld.global.nc.v4.f32 {%w0, %w1, %w2, %w3}, [%rW];

    mul.f32 %x0, %x0, %inv; mul.f32 %x0, %x0, %w0;
    mul.f32 %x1, %x1, %inv; mul.f32 %x1, %x1, %w1;
    mul.f32 %x2, %x2, %inv; mul.f32 %x2, %x2, %w2;
    mul.f32 %x3, %x3, %inv; mul.f32 %x3, %x3, %w3;

    cvt.rn.f16x2.f32 %h0, %x1, %x0;
    cvt.rn.f16x2.f32 %h1, %x3, %x2;
    st.global.v2.u32 [%rOut], {%h0, %h1};
    ret;
}
"#;

const KERNEL_QK_NORM_ROPE_F16: &str = r#"
.visible .entry qk_norm_rope_f16(
    .param .u64 pQkv,
    .param .u64 pQNorm,
    .param .u64 pKNorm,
    .param .u64 pPos,
    .param .u64 pCos,
    .param .u64 pSin,
    .param .f32 pEps
) {
    .shared .align 4 .f32 ssum[2];
    .reg .u64 %rQkv, %rW, %rPos, %rCos, %rSin, %off, %p0_addr, %p1_addr;
    .reg .u32 %t, %h, %j, %warp, %lane, %pos, %h_off, %tmp;
    .reg .b16 %b0, %b1;
    .reg .f32 %x0, %x1, %w0, %w1, %c, %s, %y0, %y1, %sum, %st, %eps, %inv;
    .reg .pred %is_k, %p_lane0;

    ld.param.u64 %rQkv, [pQkv];
    ld.param.u64 %rPos, [pPos];
    ld.param.u64 %rCos, [pCos];
    ld.param.u64 %rSin, [pSin];
    ld.param.f32 %eps, [pEps];

    mov.u32 %t, %ctaid.x;
    mov.u32 %h, %ctaid.y;
    mov.u32 %j, %tid.x;
    shr.u32 %warp, %j, 5;
    and.b32 %lane, %j, 31;

    setp.ge.u32 %is_k, %h, 16;
    @!%is_k ld.param.u64 %rW, [pQNorm];
    @%is_k  ld.param.u64 %rW, [pKNorm];

    shl.b32 %h_off, %h, 7;
    mul.wide.u32 %off, %t, 4096;
    cvt.u64.u32 %p0_addr, %h_off;
    add.u64 %off, %off, %p0_addr;
    cvt.u64.u32 %p0_addr, %j;
    add.u64 %off, %off, %p0_addr;
    shl.b64 %off, %off, 1;
    add.u64 %p0_addr, %rQkv, %off;
    add.u64 %p1_addr, %p0_addr, 128;

    ld.global.u16 %b0, [%p0_addr];
    ld.global.u16 %b1, [%p1_addr];
    cvt.f32.f16 %x0, %b0;
    cvt.f32.f16 %x1, %b1;

    mul.f32 %sum, %x0, %x0;
    fma.rn.f32 %sum, %x1, %x1, %sum;
    shfl.sync.bfly.b32 %st, %sum, 16, 31, 0xffffffff; add.f32 %sum, %sum, %st;
    shfl.sync.bfly.b32 %st, %sum, 8,  31, 0xffffffff; add.f32 %sum, %sum, %st;
    shfl.sync.bfly.b32 %st, %sum, 4,  31, 0xffffffff; add.f32 %sum, %sum, %st;
    shfl.sync.bfly.b32 %st, %sum, 2,  31, 0xffffffff; add.f32 %sum, %sum, %st;
    shfl.sync.bfly.b32 %st, %sum, 1,  31, 0xffffffff; add.f32 %sum, %sum, %st;

    setp.eq.u32 %p_lane0, %lane, 0;
    shl.b32 %tmp, %warp, 2;
    mov.u32 %h_off, ssum;
    add.u32 %tmp, %h_off, %tmp;
    @%p_lane0 st.shared.f32 [%tmp], %sum;
    bar.sync 0;

    ld.shared.f32 %sum, [ssum + 0];
    ld.shared.f32 %st,  [ssum + 4];
    add.f32 %sum, %sum, %st;
    mul.f32 %sum, %sum, 0.0078125;
    add.f32 %sum, %sum, %eps;
    rsqrt.approx.f32 %inv, %sum;

    cvt.u64.u32 %off, %j;
    shl.b64 %off, %off, 2;
    add.u64 %off, %rW, %off;
    ld.global.nc.f32 %w0, [%off];
    ld.global.nc.f32 %w1, [%off + 256];

    mul.f32 %x0, %x0, %inv; mul.f32 %x0, %x0, %w0;
    mul.f32 %x1, %x1, %inv; mul.f32 %x1, %x1, %w1;

    cvt.u64.u32 %off, %t;
    shl.b64 %off, %off, 2;
    add.u64 %off, %rPos, %off;
    ld.global.nc.u32 %pos, [%off];

    mul.wide.u32 %off, %pos, 64;
    cvt.u64.u32 %rW, %j;
    add.u64 %off, %off, %rW;
    shl.b64 %off, %off, 2;
    add.u64 %rCos, %rCos, %off;
    add.u64 %rSin, %rSin, %off;
    ld.global.nc.f32 %c, [%rCos];
    ld.global.nc.f32 %s, [%rSin];

    mul.f32 %y0, %x0, %c;
    neg.f32 %st, %s;
    fma.rn.f32 %y0, %x1, %st, %y0;

    mul.f32 %y1, %x0, %s;
    fma.rn.f32 %y1, %x1, %c, %y1;

    cvt.rn.f16.f32 %b0, %y0;
    cvt.rn.f16.f32 %b1, %y1;
    st.global.u16 [%p0_addr], %b0;
    st.global.u16 [%p1_addr], %b1;
    ret;
}
"#;

const KERNEL_COPY_PREFIX_KV_F16: &str = r#"
.visible .entry copy_prefix_kv_f16(
    .param .u64 pQkv,
    .param .u64 pK,
    .param .u64 pV
) {
    .reg .u64 %rQkv, %rK, %rV, %off_src, %off_dst;
    .reg .u32 %t, %ltid;
    .reg .b32 %v0, %v1;

    ld.param.u64 %rQkv, [pQkv];
    ld.param.u64 %rK, [pK];
    ld.param.u64 %rV, [pV];
    mov.u32 %t, %ctaid.x;
    mov.u32 %ltid, %tid.x;

    mul.wide.u32 %off_src, %t, 8192;
    cvt.u64.u32 %off_dst, %ltid;
    shl.b64 %off_dst, %off_dst, 3;
    add.u64 %off_src, %off_src, %off_dst;
    add.u64 %off_src, %off_src, %rQkv;

    mul.wide.u32 %off_dst, %t, 2048;
    cvt.u64.u32 %rQkv, %ltid;
    shl.b64 %rQkv, %rQkv, 3;
    add.u64 %off_dst, %off_dst, %rQkv;

    ld.global.v2.u32 {%v0, %v1}, [%off_src + 4096];
    add.u64 %rK, %rK, %off_dst;
    st.global.v2.u32 [%rK], {%v0, %v1};

    ld.global.v2.u32 {%v0, %v1}, [%off_src + 6144];
    add.u64 %rV, %rV, %off_dst;
    st.global.v2.u32 [%rV], {%v0, %v1};
    ret;
}
"#;

/// Tensor Core FlashAttention-2 (Br=64, Bc=32, D=128, GQA 16:8, Causal + Shared Prefix KV).
fn emit_flash_attn_tc_varlen(out: &mut String) {
    // 1 / sqrt(128) = 0.0883883476 -> IEEE-754 f16 = 0x2da8 -> f16x2 = 0x2da82da8
    let s_bits = f32_to_f16_bits(1.0 / 128.0_f32.sqrt());
    let s_f16x2 = ((s_bits as u32) << 16) | (s_bits as u32);

    out.push_str(
        r#"
.visible .entry flash_attn_tc_varlen_f16(
    .param .u64 pQkv,
    .param .u64 pOut,
    .param .u64 pTiles,
    .param .u64 pPrefK,
    .param .u64 pPrefV,
    .param .u32 pPrefLen
) {
    .shared .align 16 .b8 smem[17408];

    .reg .u64 %rQkv, %rOut, %rTiles, %rPrefK, %rPrefV, %off, %ptr;
    .reg .u32 %pref_len, %tile_id, %qh, %kvh, %ltid, %warp, %lane;
    .reg .u32 %seq_start, %seq_len, %q_off, %k_block, %k_max_tile;
    .reg .u32 %t0, %t1, %t2, %t3, %t4, %t5, %s_ptr, %s_base;
    .reg .u32 %q_r0, %q_r1;
    .reg .u32 %sK_rd0, %sK_rd1, %sV_rd0;
    .reg .b32 %q<32>;
    .reg .b32 %kb<8>;
    .reg .b32 %vb<4>;
    .reg .b32 %pb<8>;
    .reg .b32 %h0;
    .reg .f32 %s<16>;
    .reg .f32 %o<64>;
    .reg .f32 %m0, %m1, %l0, %l1, %mn0, %mn1, %al0, %al1, %ls0, %ls1, %tmp;
    .reg .f32 %flog2e, %fneg_inf;
    .reg .pred %p<8>;

    ld.param.u64 %rQkv, [pQkv];
    ld.param.u64 %rOut, [pOut];
    ld.param.u64 %rTiles, [pTiles];
    ld.param.u64 %rPrefK, [pPrefK];
    ld.param.u64 %rPrefV, [pPrefV];
    ld.param.u32 %pref_len, [pPrefLen];

    mov.u32 %tile_id, %ctaid.x;
    mov.u32 %qh, %ctaid.y;
    shr.u32 %kvh, %qh, 1;
    mov.u32 %ltid, %tid.x;
    shr.u32 %warp, %ltid, 5;
    and.b32 %lane, %ltid, 31;

    mul.wide.u32 %off, %tile_id, 12;
    add.u64 %ptr, %rTiles, %off;
    ld.global.nc.u32 %seq_start, [%ptr + 0];
    ld.global.nc.u32 %seq_len,   [%ptr + 4];
    ld.global.nc.u32 %q_off,     [%ptr + 8];

    shr.u32 %t0, %ltid, 1;
    and.b32 %t1, %ltid, 1;
    shl.b32 %t1, %t1, 6;
    add.u32 %t2, %q_off, %t0;
    setp.lt.u32 %p0, %t2, %seq_len;
    add.u32 %t2, %seq_start, %t2;

    mul.wide.u32 %off, %t2, 4096;
    shl.b32 %t3, %qh, 7;
    add.u32 %t3, %t3, %t1;
    cvt.u64.u32 %ptr, %t3;
    add.u64 %off, %off, %ptr;
    shl.b64 %off, %off, 1;
    add.u64 %ptr, %rQkv, %off;

    mul.lo.u32 %s_ptr, %t0, 272;
    shl.b32 %t3, %t1, 1;
    add.u32 %s_ptr, %s_ptr, %t3;
    mov.u32 %s_base, smem;
    add.u32 %s_ptr, %s_base, %s_ptr;
"#,
    );

    for ci in 0..8 {
        let bo = ci * 16;
        let _ = writeln!(out, "    @%p0 cp.async.cg.shared.global [%s_ptr + {bo}], [%ptr + {bo}], 16;");
        let _ = writeln!(out, "    @!%p0 st.shared.v4.u32 [%s_ptr + {bo}], {{0, 0, 0, 0}};");
    }

    out.push_str(
        r#"
    cp.async.commit_group;
    cp.async.wait_group 0;
    bar.sync 0;

    and.b32 %t0, %lane, 15;
    shl.b32 %t1, %warp, 4;
    add.u32 %t0, %t0, %t1;
    mul.lo.u32 %t0, %t0, 272;
    shr.u32 %t1, %lane, 4;
    shl.b32 %t1, %t1, 4;
    add.u32 %s_ptr, %t0, %t1;
    add.u32 %s_ptr, %s_base, %s_ptr;
"#,
    );
    for ki in 0..8 {
        let bo = ki * 32;
        let q0 = ki * 4;
        let _ = writeln!(
            out,
            "    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%q{q0}, %q{}, %q{}, %q{}}}, [%s_ptr + {bo}];",
            q0 + 1,
            q0 + 2,
            q0 + 3
        );
    }
    let _ = writeln!(out, "    mov.b32 %h0, 0x{s_f16x2:08x};");
    for qi in 0..32 {
        let _ = writeln!(out, "    mul.rn.f16x2 %q{qi}, %q{qi}, %h0;");
    }
    out.push_str("    bar.sync 0;\n");

    for i in 0..64 {
        let _ = writeln!(out, "    mov.f32 %o{i}, 0.0;");
    }

    out.push_str(
        r#"
    mov.f32 %m0, -1.0e30; mov.f32 %m1, -1.0e30;
    mov.f32 %l0, 0.0;     mov.f32 %l1, 0.0;
    mov.f32 %flog2e, 1.4426950408889634;
    mov.f32 %fneg_inf, -1.0e30;

    shr.u32 %t0, %lane, 4; shl.b32 %t0, %t0, 3;
    and.b32 %t1, %lane, 7; or.b32 %t0, %t0, %t1;
    mul.lo.u32 %t0, %t0, 272;
    shr.u32 %t1, %lane, 3; and.b32 %t1, %t1, 1; shl.b32 %t1, %t1, 4;
    add.u32 %sK_rd0, %t0, %t1;
    add.u32 %sK_rd1, %sK_rd0, 4352;

    and.b32 %t0, %lane, 15;
    mul.lo.u32 %t0, %t0, 272;
    shr.u32 %t1, %lane, 4;
    shl.b32 %t1, %t1, 4;
    add.u32 %sV_rd0, %t0, %t1;
    add.u32 %sV_rd0, %sV_rd0, 8704;

    shr.u32 %t0, %lane, 2;
    shl.b32 %t1, %warp, 4;
    add.u32 %q_r0, %q_off, %t1;
    add.u32 %q_r0, %q_r0, %t0;
    add.u32 %q_r1, %q_r0, 8;
"#,
    );

    let emit_kv_compute = |buf: &mut String, is_causal: bool| {
        for i in 0..16 {
            let _ = writeln!(buf, "    mov.f32 %s{i}, 0.0;");
        }
        for ki in 0..8 {
            let kbo = ki * 32;
            let _ = writeln!(buf, "    add.u32 %s_ptr, %s_base, %sK_rd0; ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%kb0, %kb1, %kb2, %kb3}}, [%s_ptr + {kbo}];");
            let _ = writeln!(buf, "    add.u32 %s_ptr, %s_base, %sK_rd1; ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%kb4, %kb5, %kb6, %kb7}}, [%s_ptr + {kbo}];");
            for ni in 0..4 {
                let si = ni * 4;
                let qo = ki * 4;
                let ko = ni * 2;
                let _ = writeln!(
                    buf,
                    "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%s{si}, %s{}, %s{}, %s{}}}, {{%q{qo}, %q{}, %q{}, %q{}}}, {{%kb{ko}, %kb{}}}, {{%s{si}, %s{}, %s{}, %s{}}};",
                    si + 1, si + 2, si + 3,
                    qo + 1, qo + 2, qo + 3,
                    ko + 1,
                    si + 1, si + 2, si + 3
                );
            }
        }
        buf.push_str("    and.b32 %t0, %lane, 3;\n    shl.b32 %t0, %t0, 1;\n");
        for ni in 0..4 {
            for c_sub in 0..2 {
                let col_in_32 = ni * 8 + c_sub;
                let si_top = ni * 4 + c_sub;
                let si_bot = ni * 4 + 2 + c_sub;
                let _ = writeln!(buf, "    add.u32 %t1, %t0, {col_in_32};");
                let _ = writeln!(buf, "    setp.ge.u32 %p1, %t1, %t5;");
                let _ = writeln!(buf, "    selp.f32 %s{si_top}, %fneg_inf, %s{si_top}, %p1;");
                let _ = writeln!(buf, "    selp.f32 %s{si_bot}, %fneg_inf, %s{si_bot}, %p1;");
                if is_causal {
                    let _ = writeln!(buf, "    add.u32 %t2, %k_block, %t1;");
                    let _ = writeln!(buf, "    setp.gt.u32 %p2, %t2, %q_r0;");
                    let _ = writeln!(buf, "    selp.f32 %s{si_top}, %fneg_inf, %s{si_top}, %p2;");
                    let _ = writeln!(buf, "    setp.gt.u32 %p3, %t2, %q_r1;");
                    let _ = writeln!(buf, "    selp.f32 %s{si_bot}, %fneg_inf, %s{si_bot}, %p3;");
                }
            }
        }
        buf.push_str("    mov.f32 %mn0, %m0; mov.f32 %mn1, %m1;\n");
        for ni in 0..4 {
            let si = ni * 4;
            let _ = writeln!(buf, "    max.f32 %mn0, %mn0, %s{si}; max.f32 %mn0, %mn0, %s{};", si + 1);
            let _ = writeln!(buf, "    max.f32 %mn1, %mn1, %s{}; max.f32 %mn1, %mn1, %s{};", si + 2, si + 3);
        }
        buf.push_str(
            r#"
    shfl.sync.bfly.b32 %tmp, %mn0, 2, 31, 0xffffffff; max.f32 %mn0, %mn0, %tmp;
    shfl.sync.bfly.b32 %tmp, %mn0, 1, 31, 0xffffffff; max.f32 %mn0, %mn0, %tmp;
    shfl.sync.bfly.b32 %tmp, %mn1, 2, 31, 0xffffffff; max.f32 %mn1, %mn1, %tmp;
    shfl.sync.bfly.b32 %tmp, %mn1, 1, 31, 0xffffffff; max.f32 %mn1, %mn1, %tmp;

    sub.f32 %tmp, %m0, %mn0; mul.f32 %tmp, %tmp, %flog2e; ex2.approx.f32 %al0, %tmp;
    sub.f32 %tmp, %m1, %mn1; mul.f32 %tmp, %tmp, %flog2e; ex2.approx.f32 %al1, %tmp;
    mov.f32 %m0, %mn0; mov.f32 %m1, %mn1;
    mov.f32 %ls0, 0.0; mov.f32 %ls1, 0.0;
"#,
        );
        for ni in 0..4 {
            let si = ni * 4;
            for (off_s, m_r, ls_r) in [(0, "%m0", "%ls0"), (1, "%m0", "%ls0"), (2, "%m1", "%ls1"), (3, "%m1", "%ls1")] {
                let s = si + off_s;
                let _ = writeln!(
                    buf,
                    "    sub.f32 %tmp, %s{s}, {m_r}; mul.f32 %tmp, %tmp, %flog2e; ex2.approx.f32 %s{s}, %tmp; add.f32 {ls_r}, {ls_r}, %s{s};"
                );
            }
        }
        buf.push_str(
            r#"
    shfl.sync.bfly.b32 %tmp, %ls0, 2, 31, 0xffffffff; add.f32 %ls0, %ls0, %tmp;
    shfl.sync.bfly.b32 %tmp, %ls0, 1, 31, 0xffffffff; add.f32 %ls0, %ls0, %tmp;
    shfl.sync.bfly.b32 %tmp, %ls1, 2, 31, 0xffffffff; add.f32 %ls1, %ls1, %tmp;
    shfl.sync.bfly.b32 %tmp, %ls1, 1, 31, 0xffffffff; add.f32 %ls1, %ls1, %tmp;

    fma.rn.f32 %l0, %l0, %al0, %ls0;
    fma.rn.f32 %l1, %l1, %al1, %ls1;
"#,
        );
        for ni in 0..16 {
            let oi = ni * 4;
            let _ = writeln!(buf, "    mul.f32 %o{oi}, %o{oi}, %al0; mul.f32 %o{}, %o{}, %al0;", oi + 1, oi + 1);
            let _ = writeln!(buf, "    mul.f32 %o{}, %o{}, %al1; mul.f32 %o{}, %o{}, %al1;", oi + 2, oi + 2, oi + 3, oi + 3);
        }
        for pi in 0..8 {
            let _ = writeln!(buf, "    cvt.rn.f16x2.f32 %pb{pi}, %s{}, %s{};", pi * 2 + 1, pi * 2);
        }
        for k_sub in 0..2 {
            let v_row_off = k_sub * 4352;
            let pbo = k_sub * 4;
            for d_pair in 0..8 {
                let v_off = v_row_off + d_pair * 32;
                let oi0 = (d_pair * 2) * 4;
                let oi1 = (d_pair * 2 + 1) * 4;
                let _ = writeln!(
                    buf,
                    "    add.u32 %s_ptr, %s_base, %sV_rd0; ldmatrix.sync.aligned.m8n8.x4.trans.shared.b16 {{%vb0, %vb1, %vb2, %vb3}}, [%s_ptr + {v_off}];"
                );
                let _ = writeln!(
                    buf,
                    "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%o{oi0}, %o{}, %o{}, %o{}}}, {{%pb{pbo}, %pb{}, %pb{}, %pb{}}}, {{%vb0, %vb1}}, {{%o{oi0}, %o{}, %o{}, %o{}}};",
                    oi0 + 1, oi0 + 2, oi0 + 3, pbo + 1, pbo + 2, pbo + 3, oi0 + 1, oi0 + 2, oi0 + 3
                );
                let _ = writeln!(
                    buf,
                    "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%o{oi1}, %o{}, %o{}, %o{}}}, {{%pb{pbo}, %pb{}, %pb{}, %pb{}}}, {{%vb2, %vb3}}, {{%o{oi1}, %o{}, %o{}, %o{}}};",
                    oi1 + 1, oi1 + 2, oi1 + 3, pbo + 1, pbo + 2, pbo + 3, oi1 + 1, oi1 + 2, oi1 + 3
                );
            }
        }
    };

    // Phase 1: Shared Prefix K/V
    out.push_str(
        r#"
    setp.gt.u32 %p4, %pref_len, 0;
    @!%p4 bra START_LOCAL;

    mov.u32 %k_block, 0;
PREF_LOOP:
    shr.u32 %t0, %ltid, 2;
    and.b32 %t1, %ltid, 3;
    shl.b32 %t1, %t1, 5;
    add.u32 %t2, %k_block, %t0;
    setp.lt.u32 %p0, %t2, %pref_len;

    mul.lo.u32 %s_ptr, %t0, 272;
    shl.b32 %t3, %t1, 1;
    add.u32 %s_ptr, %s_ptr, %t3;
    add.u32 %s_ptr, %s_base, %s_ptr;

    mul.wide.u32 %off, %t2, 1024;
    shl.b32 %t3, %kvh, 7;
    add.u32 %t3, %t3, %t1;
    cvt.u64.u32 %ptr, %t3;
    add.u64 %off, %off, %ptr;
    shl.b64 %off, %off, 1;
    add.u64 %ptr, %rPrefK, %off;
"#,
    );
    for ci in 0..4 {
        let bo = ci * 16;
        let _ = writeln!(out, "    @%p0 cp.async.cg.shared.global [%s_ptr + {bo}], [%ptr + {bo}], 16;");
        let _ = writeln!(out, "    @!%p0 st.shared.v4.u32 [%s_ptr + {bo}], {{0, 0, 0, 0}};");
    }
    out.push_str("    add.u64 %ptr, %rPrefV, %off;\n    add.u32 %s_ptr, %s_ptr, 8704;\n");
    for ci in 0..4 {
        let bo = ci * 16;
        let _ = writeln!(out, "    @%p0 cp.async.cg.shared.global [%s_ptr + {bo}], [%ptr + {bo}], 16;");
        let _ = writeln!(out, "    @!%p0 st.shared.v4.u32 [%s_ptr + {bo}], {{0, 0, 0, 0}};");
    }
    out.push_str(
        r#"
    cp.async.commit_group;
    cp.async.wait_group 0;
    bar.sync 0;

    sub.u32 %t5, %pref_len, %k_block;
    min.u32 %t5, %t5, 32;
"#,
    );
    emit_kv_compute(out, false);
    out.push_str(
        r#"
    bar.sync 0;
    add.u32 %k_block, %k_block, 32;
    setp.lt.u32 %p5, %k_block, %pref_len;
    @%p5 bra PREF_LOOP;

START_LOCAL:
    add.u32 %k_max_tile, %q_off, 64;
    min.u32 %k_max_tile, %k_max_tile, %seq_len;
    mov.u32 %k_block, 0;
LOC_LOOP:
    shr.u32 %t0, %ltid, 2;
    and.b32 %t1, %ltid, 3;
    shl.b32 %t1, %t1, 5;
    add.u32 %t2, %k_block, %t0;
    setp.lt.u32 %p0, %t2, %k_max_tile;

    mul.lo.u32 %s_ptr, %t0, 272;
    shl.b32 %t3, %t1, 1;
    add.u32 %s_ptr, %s_ptr, %t3;
    add.u32 %s_ptr, %s_base, %s_ptr;

    add.u32 %t2, %seq_start, %t2;
    mul.wide.u32 %off, %t2, 4096;
    shl.b32 %t3, %kvh, 7;
    add.u32 %t3, %t3, %t1;
    cvt.u64.u32 %ptr, %t3;
    add.u64 %off, %off, %ptr;
    shl.b64 %off, %off, 1;
    add.u64 %ptr, %rQkv, %off;
"#,
    );
    for ci in 0..4 {
        let bo = ci * 16;
        let _ = writeln!(out, "    @%p0 cp.async.cg.shared.global [%s_ptr + {bo}], [%ptr + {}], 16;", 4096 + bo);
        let _ = writeln!(out, "    @!%p0 st.shared.v4.u32 [%s_ptr + {bo}], {{0, 0, 0, 0}};");
    }
    out.push_str("    add.u32 %s_ptr, %s_ptr, 8704;\n");
    for ci in 0..4 {
        let bo = ci * 16;
        let _ = writeln!(out, "    @%p0 cp.async.cg.shared.global [%s_ptr + {bo}], [%ptr + {}], 16;", 6144 + bo);
        let _ = writeln!(out, "    @!%p0 st.shared.v4.u32 [%s_ptr + {bo}], {{0, 0, 0, 0}};");
    }
    out.push_str(
        r#"
    cp.async.commit_group;
    cp.async.wait_group 0;
    bar.sync 0;

    sub.u32 %t5, %k_max_tile, %k_block;
    min.u32 %t5, %t5, 32;
"#,
    );
    emit_kv_compute(out, true);
    out.push_str(
        r#"
    bar.sync 0;
    add.u32 %k_block, %k_block, 32;
    setp.lt.u32 %p5, %k_block, %k_max_tile;
    @%p5 bra LOC_LOOP;

    rcp.rn.f32 %al0, %l0;
    rcp.rn.f32 %al1, %l1;
    and.b32 %t0, %lane, 3;
    shl.b32 %t0, %t0, 1;
    shl.b32 %t1, %qh, 7;
    add.u32 %t1, %t1, %t0;

    setp.lt.u32 %p0, %q_r0, %seq_len;
    @!%p0 bra SKIP_OUT0;
    add.u32 %t2, %seq_start, %q_r0;
    mul.wide.u32 %off, %t2, 2048;
    cvt.u64.u32 %ptr, %t1;
    add.u64 %off, %off, %ptr;
    shl.b64 %off, %off, 1;
    add.u64 %ptr, %rOut, %off;
"#,
    );
    for ni in 0..16 {
        let oi = ni * 4;
        let bo = ni * 16;
        let _ = writeln!(
            out,
            "    mul.f32 %o{oi}, %o{oi}, %al0; mul.f32 %o{}, %o{}, %al0; cvt.rn.f16x2.f32 %h0, %o{}, %o{oi}; st.global.u32 [%ptr + {bo}], %h0;",
            oi + 1, oi + 1, oi + 1
        );
    }
    out.push_str(
        r#"
SKIP_OUT0:
    setp.lt.u32 %p1, %q_r1, %seq_len;
    @!%p1 bra DONE;
    add.u32 %t2, %seq_start, %q_r1;
    mul.wide.u32 %off, %t2, 2048;
    cvt.u64.u32 %ptr, %t1;
    add.u64 %off, %off, %ptr;
    shl.b64 %off, %off, 1;
    add.u64 %ptr, %rOut, %off;
"#,
    );
    for ni in 0..16 {
        let oi = ni * 4 + 2;
        let bo = ni * 16;
        let _ = writeln!(
            out,
            "    mul.f32 %o{oi}, %o{oi}, %al1; mul.f32 %o{}, %o{}, %al1; cvt.rn.f16x2.f32 %h0, %o{}, %o{oi}; st.global.u32 [%ptr + {bo}], %h0;",
            oi + 1, oi + 1, oi + 1
        );
    }
    out.push_str("DONE:\n    ret;\n}\n");
}

fn emit_swiglu_3072_fast(out: &mut String) {
    out.push_str(
        r#"
.visible .entry swiglu_3072_fast_f16(.param .u64 pGU, .param .u64 pOut) {
    .reg .u64 %rGU, %rOut, %off, %ptr;
    .reg .u32 %row, %ltid, %col;
    .reg .b32 %hg0, %hg1, %hu0, %hu1, %ho0, %ho1;
    .reg .b16 %a0, %a1, %a2, %a3;
    .reg .f32 %g0, %g1, %g2, %g3, %u0, %u1, %u2, %u3, %e;

    ld.param.u64 %rGU, [pGU];
    ld.param.u64 %rOut, [pOut];
    mov.u32 %row, %ctaid.x;
    mov.u32 %ltid, %tid.x;
    mul.wide.u32 %off, %row, 12288; add.u64 %rGU, %rGU, %off;
    mul.wide.u32 %off, %row, 6144;  add.u64 %rOut, %rOut, %off;
    shl.b32 %col, %ltid, 3; cvt.u64.u32 %ptr, %col;
    add.u64 %rGU, %rGU, %ptr; add.u64 %rOut, %rOut, %ptr;
"#,
    );
    for it in 0..3 {
        let bo = it * 2048;
        let _ = writeln!(
            out,
            r#"
    ld.global.v2.u32 {{%hg0, %hg1}}, [%rGU + {bo}];
    ld.global.v2.u32 {{%hu0, %hu1}}, [%rGU + {}];
    mov.b32 {{%a0, %a1}}, %hg0; cvt.f32.f16 %g0, %a0; cvt.f32.f16 %g1, %a1;
    mov.b32 {{%a2, %a3}}, %hg1; cvt.f32.f16 %g2, %a2; cvt.f32.f16 %g3, %a3;
    mov.b32 {{%a0, %a1}}, %hu0; cvt.f32.f16 %u0, %a0; cvt.f32.f16 %u1, %a1;
    mov.b32 {{%a2, %a3}}, %hu1; cvt.f32.f16 %u2, %a2; cvt.f32.f16 %u3, %a3;
    mul.f32 %e, %g0, -1.4426950408889634; ex2.approx.f32 %e, %e; add.f32 %e, %e, 1.0; rcp.rn.f32 %e, %e; mul.f32 %g0, %g0, %e; mul.f32 %g0, %g0, %u0;
    mul.f32 %e, %g1, -1.4426950408889634; ex2.approx.f32 %e, %e; add.f32 %e, %e, 1.0; rcp.rn.f32 %e, %e; mul.f32 %g1, %g1, %e; mul.f32 %g1, %g1, %u1;
    mul.f32 %e, %g2, -1.4426950408889634; ex2.approx.f32 %e, %e; add.f32 %e, %e, 1.0; rcp.rn.f32 %e, %e; mul.f32 %g2, %g2, %e; mul.f32 %g2, %g2, %u2;
    mul.f32 %e, %g3, -1.4426950408889634; ex2.approx.f32 %e, %e; add.f32 %e, %e, 1.0; rcp.rn.f32 %e, %e; mul.f32 %g3, %g3, %e; mul.f32 %g3, %g3, %u3;
    cvt.rn.f16x2.f32 %ho0, %g1, %g0;
    cvt.rn.f16x2.f32 %ho1, %g3, %g2;
    st.global.v2.u32 [%rOut + {bo}], {{%ho0, %ho1}};"#,
            bo + 6144
        );
    }
    out.push_str("    ret;\n}\n");
}

/// Convert IEEE-754 f32 to f16 bit representation (round-to-nearest-even).
#[inline]
pub fn f32_to_f16_bits(val: f32) -> u16 {
    let bits = val.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x007f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x0200 } else { 0 };
    }
    let new_exp = exp - 127 + 15;
    if new_exp >= 31 {
        return sign | 0x7c00;
    }
    if new_exp <= 0 {
        if new_exp < -10 {
            return sign;
        }
        let significand = mant | 0x0080_0000;
        let shift = 14 - new_exp;
        // Keep the full significand until rounding, including the sticky bits at the zero boundary.
        let bias = (1 << (shift - 1)) - 1 + ((significand >> shift) & 1);
        return sign | ((significand + bias) >> shift) as u16;
    }
    let rounded = mant + 0x0fff + ((mant >> 13) & 1);
    if (rounded & 0x0080_0000) != 0 {
        sign | (((new_exp + 1) as u16) << 10)
    } else {
        sign | ((new_exp as u16) << 10) | ((rounded >> 13) as u16)
    }
}

/// Convert IEEE-754 f16 bit representation to f32.
#[inline]
pub fn f16_bits_to_f32(h: u16) -> f32 {
    let sign = ((h as u32) & 0x8000) << 16;
    let exp = ((h >> 10) & 0x1f) as i32;
    let mant = (h & 0x03ff) as u32;
    if exp == 0 {
        if mant == 0 {
            return f32::from_bits(sign);
        }
        let mut m = mant;
        let mut e = -14i32;
        while (m & 0x0400) == 0 {
            m <<= 1;
            e -= 1;
        }
        m &= 0x03ff;
        return f32::from_bits(sign | (((e + 127) as u32) << 23) | (m << 13));
    }
    if exp == 31 {
        return f32::from_bits(sign | 0x7f80_0000 | (mant << 13));
    }
    f32::from_bits(sign | (((exp - 15 + 127) as u32) << 23) | (mant << 13))
}
