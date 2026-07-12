use super::*;

#[test]
fn runs_i8080_and_i8085_programs_with_16_bit_state() {
    for (cpu, assembly) in [
        (CpuFamily::I8080, "mvi a, 0\nout 0Dh\nmvi a, 1\nout 0Eh\n"),
        (
            CpuFamily::I8085,
            "rim\nmvi a, 0\nout 0Dh\nmvi a, 1\nout 0Eh\n",
        ),
    ] {
        let run = run_assembly_test_with_cpu_options_at(
            cpu,
            assembly,
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0xFF00,
            },
            0x0100,
        )
        .unwrap();

        assert!(run.halted, "{cpu:?}: {run:?}");
        assert_eq!(run.result_code, 0, "{cpu:?}: {run:?}");
        assert_eq!(run.failure, None, "{cpu:?}: {run:?}");
    }
}

#[test]
fn rejects_i8080_images_outside_the_16_bit_address_space() {
    let error = run_assembly_test_with_cpu_options_at(
        CpuFamily::I8080,
        "hlt\n",
        &TestRunOptions {
            instruction_budget: 1,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0xFF00,
        },
        0x01_0000,
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "test image base address 0x10000 is outside the 16-bit address space"
    );
}

#[test]
fn runs_8_bit_register_loads_on_ez80_vm() {
    let asm = r#"
            ld e, 00h
            ld a, 43h
            ld e, a
            ld a, e
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"C");
}

#[test]
fn runs_bc_de_indirect_accumulator_loads_and_stores_on_ez80_vm() {
    let asm = r#"
            ld bc, 040100h
            ld de, 040101h
            ld a, 42h
            ld (bc), a
            ld a, 44h
            ld (de), a
            ld a, (bc)
            out0 (0Ch), a
            ld a, (de)
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"BD");
}

#[test]
fn runs_bc_de_direct_memory_loads_and_stores_on_ez80_vm() {
    let asm = r#"
            ld bc, 004244h
            ld (040100h), bc
            ld de, (040100h)
            ld a, d
            out0 (0Ch), a
            ld a, e
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"BD");
}

#[test]
fn runs_hl_indirect_8_bit_loads_and_stores_on_ez80_vm() {
    let asm = r#"
            ld hl, 040100h
            ld a, 41h
            ld (hl), a
            ld b, (hl)
            inc hl
            ld (hl), b
            ld e, (hl)
            ld a, e
            add a, 02h
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"C");
}

#[test]
fn runs_hl_indirect_immediate_store_on_ez80_vm() {
    let asm = r#"
            ld hl, 040100h
            ld (hl), 43h
            ld a, (hl)
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"C");
}

#[test]
fn runs_8_bit_register_inc_and_dec_on_ez80_vm() {
    let asm = r#"
            ld e, 42h
            inc e
            ld a, e
            dec a
            inc a
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"C");
}

#[test]
fn runs_ez80_mlt_register_form_on_vm() {
    let asm = r#"
            ld b, 11h
            ld c, 0Fh
            mlt bc
            ld a, c
            cp 0FFh
            jp nz, fail
            ld a, b
            cp 00h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 02h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn runs_ez80_ldir_on_vm() {
    let asm = r#"
            ld a, 41h
            ld (040300h), a
            ld a, 42h
            ld (040301h), a
            ld a, 43h
            ld (040302h), a
            ld hl, 040300h
            ld de, 040310h
            ld bc, 000003h
            ldir
            ld a, (040310h)
            cp 41h
            jp nz, fail
            ld a, (040311h)
            cp 42h
            jp nz, fail
            ld a, (040312h)
            cp 43h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 02h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 200).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn runs_ez80_cpir_on_vm() {
    let asm = r#"
            ld a, 11h
            ld (040300h), a
            ld a, 42h
            ld (040301h), a
            ld a, 33h
            ld (040302h), a
            ld a, 42h
            ld hl, 040300h
            ld bc, 000003h
            cpir
            jp nz, fail
            ld a, c
            cp 01h
            jp nz, fail
            ld (040310h), hl
            ld a, (040310h)
            cp 02h
            jp nz, fail
            ld a, (040311h)
            cp 03h
            jp nz, fail
            ld a, (040312h)
            cp 04h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 02h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 300).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn runs_ez80_otir_on_vm() {
    let asm = r#"
            ld a, 11h
            ld (040320h), a
            ld a, 42h
            ld (040321h), a
            ld hl, 040320h
            ld bc, 000220h
            otir
            ld a, c
            cp 20h
            jp nz, fail
            ld a, b
            cp 00h
            jp nz, fail
            ld (040330h), hl
            ld a, (040330h)
            cp 22h
            jp nz, fail
            ld a, (040331h)
            cp 03h
            jp nz, fail
            ld a, (040332h)
            cp 04h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 02h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 400).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.ports[0x20], 0x42);
}

#[test]
fn runs_8_bit_accumulator_alu_register_forms_on_ez80_vm() {
    let asm = r#"
            ld a, 40h
            ld e, 04h
            add a, e
            cp 45h
            ld e, 00h
            adc a, e
            cp 46h
            ld e, 01h
            sbc a, e
            ld d, 01h
            sub d
            ld l, 03h
            or l
            ld h, 7Fh
            and h
            ld e, 00h
            xor e
            cp e
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"C");
}

#[test]
fn runs_8_bit_accumulator_alu_immediate_forms_on_ez80_vm() {
    let asm = r#"
            ld a, 40h
            add a, 04h
            cp 45h
            adc a, 00h
            cp 46h
            sbc a, 01h
            sub 01h
            or 03h
            and 7Fh
            xor 00h
            cp 43h
            jp z, ok
            ld a, 01h
            out0 (0Dh), a
            out0 (0Eh), a
        ok:
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"C");
}

#[test]
fn runs_misc_accumulator_alu_instructions_on_ez80_vm() {
    let asm = r#"
            scf
            ccf
            jp c, fail
            ld a, 0F0h
            cpl
            cp 0Fh
            jp nz, fail
            neg
            cp 0F1h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 01h
            out0 (0Dh), a
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn runs_accumulator_rotate_shorthands_on_ez80_vm() {
    let asm = r#"
            ld a, 81h
            rlca
            cp 03h
            jp nz, fail
            rrca
            cp 81h
            jp nz, fail
            rla
            rra
            cp 81h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 01h
            out0 (0Dh), a
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn runs_bit_register_instructions_on_ez80_vm() {
    let asm = r#"
            ld a, 02h
            set 0, a
            cp 03h
            jp nz, fail
            res 0, a
            cp 02h
            jp nz, fail
            bit 1, a
            jp z, fail
            bit 0, a
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 01h
            out0 (0Dh), a
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
}

#[test]
fn runs_ix_displacement_loads_and_stores() {
    let asm = r#"
            ld sp, 0F00000h
            ld ix, 040200h
            ld a, 2Ah
            ld (ix+3), a
            ld a, 00h
            ld a, (ix+3)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0x2A);
}

#[test]
fn runs_ix_push_pop_and_sp_add() {
    let asm = r#"
            ld sp, 040400h
            ld ix, 000000h
            add ix, sp
            ld a, 11h
            ld (ix+1), a
            ld b, a
            ld a, (040401h)
            cp b
            jp z, sp_ok
            ld a, 0EEh
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        sp_ok:
            ld ix, 040220h
            push ix
            ld ix, 040240h
            pop ix
            ld a, 07h
            ld (ix+0), a
            ld a, (040220h)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let run = run_assembly_test_with_options(
        asm,
        &TestRunOptions {
            instruction_budget: 200,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0x040400,
        },
    )
    .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 7);
}

#[test]
fn runs_iy_displacement_loads_and_stores() {
    let asm = r#"
            ld sp, 0F00000h
            ld iy, 040200h
            ld a, 35h
            ld (iy+3), a
            ld a, 00h
            ld a, (iy+3)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert!(
        bytes
            .windows(5)
            .any(|window| window == [0xFD, 0x21, 0x00, 0x02, 0x04])
    );
    assert!(bytes.windows(3).any(|window| window == [0xFD, 0x77, 0x03]));
    assert!(bytes.windows(3).any(|window| window == [0xFD, 0x7E, 0x03]));

    let run = run_assembly_test(asm, 100).unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0x35);
}

#[test]
fn runs_iy_push_pop_and_sp_add() {
    let asm = r#"
            ld sp, 040400h
            ld iy, 000000h
            add iy, sp
            ld a, 12h
            ld (iy+1), a
            ld b, a
            ld a, (040401h)
            cp b
            jp z, sp_ok
            ld a, 0EEh
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        sp_ok:
            ld iy, 040220h
            push iy
            ld iy, 040240h
            pop iy
            ld a, 09h
            ld (iy+0), a
            ld a, (040220h)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
    let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

    assert!(bytes.windows(2).any(|window| window == [0xFD, 0x39]));
    assert!(bytes.windows(2).any(|window| window == [0xFD, 0xE5]));
    assert!(bytes.windows(2).any(|window| window == [0xFD, 0xE1]));

    let run = run_assembly_test_with_options(
        asm,
        &TestRunOptions {
            instruction_budget: 200,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: 0x040400,
        },
    )
    .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 9);
}
