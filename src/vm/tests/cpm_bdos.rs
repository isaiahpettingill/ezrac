use super::*;

fn cpm_options(instruction_budget: u64) -> TestRunOptions {
    TestRunOptions {
        instruction_budget,
        initial_ports: Vec::new(),
        initial_memory: Vec::new(),
        stack_top: 0xFF00,
    }
}

fn assert_cpm_run(run: &TestRun, expected_output: &[u8]) {
    assert!(run.halted, "{run:?}");
    assert_eq!(run.failure, None, "{run:?}");
    assert_eq!(run.debug_output, expected_output, "{run:?}");
}

fn seed_fcb(memory: &mut Vec<(u32, u8)>, address: u32, name: &[u8; 11]) {
    memory.push((address, 0));
    for (offset, byte) in name.iter().copied().enumerate() {
        memory.push((address + 1 + offset as u32, byte));
    }
    for offset in 12..36 {
        memory.push((address + offset, 0));
    }
}

#[test]
fn cpm_console_tail_status_and_buffered_input_work_on_all_cpm_cpus() {
    let z80_assembly = r#"
        start:
            ld hl, 0080h
            ld a, (hl)
            call emit_a
            inc hl
            ld a, (hl)
            call emit_a

            ld c, 0Bh
            call 0005h
            call emit_a
            ld c, 01h
            call 0005h
            call emit_a

            ld de, 0200h
            ld c, 0Ah
            call 0005h
            ld hl, 0201h
            ld a, (hl)
            call emit_a
            ld hl, 0200h
            inc hl
            inc hl
            ld a, (hl)
            call emit_a

            ld c, 00h
            call 0005h

        emit_a:
            push af
            ld e, a
            ld c, 02h
            call 0005h
            pop af
            ret

        input_buffer:
            db 8
        input_length:
            db 0
        input_data:
            db 0, 0, 0, 0, 0, 0, 0, 0
    "#;
    let intel_assembly = r#"
        start:
            lxi h, 0080h
            mov a, m
            call emit_a
            inx h
            mov a, m
            call emit_a

            mvi c, 0Bh
            call 0005h
            call emit_a
            mvi c, 01h
            call 0005h
            call emit_a

            lxi d, 0200h
            mvi c, 0Ah
            call 0005h
            lxi h, 0201h
            mov a, m
            call emit_a
            lxi h, 0200h
            inx h
            inx h
            mov a, m
            call emit_a

            mvi c, 00h
            call 0005h

        emit_a:
            push psw
            mov e, a
            mvi c, 02h
            call 0005h
            pop psw
            ret

    "#;
    for cpu in [CpuFamily::Z80, CpuFamily::I8080, CpuFamily::I8085] {
        let assembly = if matches!(cpu, CpuFamily::Z80) {
            z80_assembly
        } else {
            intel_assembly
        };
        let mut files: [CpmFixtureFile<'_>; 0] = [];
        let command_tail = *b"AB CD";
        let console_input = [b'X', b'Y', 13];
        let mut fixture = CpmBdosFixture::new(&mut files, &command_tail, &console_input);
        let mut options = cpm_options(10_000);
        options.initial_memory = vec![(0x0200, 8), (0x0201, 0)];
        let run =
            run_assembly_test_with_cpm_bdos_fixture(cpu, assembly, &options, &mut fixture).unwrap();
        assert_cpm_run(&run, &[5, b'A', 0xFF, b'X', 1, b'Y']);
    }
}

#[test]
fn cpm_file_fixture_supports_record_and_directory_operations() {
    let assembly = r#"
        start:
            ld de, 0500h
            ld c, 1Ah
            call 0005h

            ld de, 0400h
            ld c, 0Fh
            call 0005h
            call emit_a
            ld c, 14h
            call 0005h
            call emit_a
            ld hl, 0500h
            ld a, (hl)
            call emit_a
            ld c, 14h
            call 0005h
            call emit_a
            ld hl, 0500h
            ld a, (hl)
            call emit_a
            ld c, 14h
            call 0005h
            call emit_a

            ld c, 23h
            call 0005h
            call emit_a
            ld hl, 0421h
            ld a, (hl)
            call emit_a

            ld a, 01h
            ld (hl), a
            ld c, 21h
            call 0005h
            call emit_a
            ld hl, 0500h
            ld a, (hl)
            call emit_a
            ld hl, 0420h
            ld a, 01h
            ld (hl), a
            ld c, 24h
            call 0005h
            ld hl, 0421h
            ld a, (hl)
            call emit_a
            ld hl, 0500h
            ld a, 43h
            ld (hl), a

            ld c, 10h
            call 0005h
            call emit_a

            ld de, 0424h
            ld c, 16h
            call 0005h
            call emit_a
            ld hl, 0445h
            ld a, 01h
            ld (hl), a
            ld c, 22h
            call 0005h
            call emit_a
            ld hl, 0500h
            ld a, 44h
            ld (hl), a
            ld c, 15h
            call 0005h
            call emit_a
            ld c, 10h
            call 0005h
            call emit_a

            ld de, 0448h
            ld c, 17h
            call 0005h
            call emit_a

            ld de, 046Ch
            ld c, 11h
            call 0005h
            call emit_a
            ld hl, 0501h
            ld a, (hl)
            call emit_a
            ld hl, 0509h
            ld a, (hl)
            call emit_a
            ld c, 12h
            call 0005h
            call emit_a
            ld hl, 0501h
            ld a, (hl)
            call emit_a
            ld c, 12h
            call 0005h
            call emit_a

            ld de, 0490h
            ld c, 13h
            call 0005h
            call emit_a
            ld c, 0Fh
            call 0005h
            call emit_a

            ld c, 00h
            call 0005h

        emit_a:
            push af
            push de
            ld e, a
            ld c, 02h
            call 0005h
            pop de
            pop af
            ret
    "#;

    let mut source_records = [[0u8; CPM_RECORD_SIZE]; 2];
    source_records[0][0] = b'A';
    source_records[1][0] = b'B';
    let mut alpha_records = [[0u8; CPM_RECORD_SIZE]; 1];
    let mut beta_records = [[0u8; CPM_RECORD_SIZE]; 1];
    let mut gamma_records = [[0u8; CPM_RECORD_SIZE]; 1];
    let mut delta_records = [[0u8; CPM_RECORD_SIZE]; 1];
    let mut created_records = [[0u8; CPM_RECORD_SIZE]; 2];
    let mut source = CpmFixtureFile::with_record_count(*b"SOURCE  TXT", &mut source_records, 2);
    source.attributes = 0x80;
    let mut files = [
        source,
        CpmFixtureFile::new(*b"ALPHA   TXT", &mut alpha_records),
        CpmFixtureFile::new(*b"BETA    TXT", &mut beta_records),
        CpmFixtureFile::new(*b"GAMMA   TXT", &mut gamma_records),
        CpmFixtureFile::new(*b"DELTA   TXT", &mut delta_records),
        CpmFixtureFile::empty(&mut created_records),
    ];

    let mut options = cpm_options(20_000);
    seed_fcb(&mut options.initial_memory, 0x0400, b"SOURCE  TXT");
    seed_fcb(&mut options.initial_memory, 0x0424, b"NEWFILE TXT");
    seed_fcb(&mut options.initial_memory, 0x0448, b"NEWFILE TXT");
    for offset in 0u32..12 {
        let byte = if offset == 0 {
            0
        } else {
            b"FINAL   TXT"[offset as usize - 1]
        };
        options.initial_memory.push((0x0448 + 16 + offset, byte));
    }
    seed_fcb(&mut options.initial_memory, 0x046C, b"???????????");
    seed_fcb(&mut options.initial_memory, 0x0490, b"FINAL   TXT");
    options.initial_memory.push((0x0500, b'C'));

    for file in &mut files {
        file.drive = 1;
    }
    let command_tail: [u8; 0] = [];
    let console_input: [u8; 0] = [];
    let mut fixture = CpmBdosFixture::new(&mut files, &command_tail, &console_input);
    fixture.current_drive = 1;
    let run =
        run_assembly_test_with_cpm_bdos_fixture(CpuFamily::Z80, assembly, &options, &mut fixture)
            .unwrap();

    assert_cpm_run(
        &run,
        &[
            0, 0, b'A', 0, b'B', 1, 0, 2, 0, b'B', 1, 0, 0, 0, 0, 0, 0, 0, 83, 0xD4, 0, 68, 255, 0,
            255,
        ],
    );
    assert_eq!(fixture.files[5].record_count, 2);
    assert_eq!(fixture.files[5].records[0][0], b'D');
    assert_eq!(fixture.files[5].records[1][0], b'C');
    assert!(!fixture.files[5].present);
}

#[test]
fn cpm_fixture_injects_missing_file_and_disk_full_results() {
    let assembly = r#"
        start:
            ld de, 0440h
            ld c, 0Fh
            call 0005h
            call emit_a
            ld de, 0400h
            ld c, 16h
            call 0005h
            call emit_a
            ld de, 0500h
            ld c, 1Ah
            call 0005h
            ld de, 0400h
            ld c, 15h
            call 0005h
            call emit_a
            ld c, 10h
            call 0005h
            call emit_a
            ld c, 13h
            call 0005h
            call emit_a
            ld c, 00h
            call 0005h

        emit_a:
            push af
            push de
            ld e, a
            ld c, 02h
            call 0005h
            pop de
            pop af
            ret
    "#;
    let mut records = [[0u8; CPM_RECORD_SIZE]; 1];
    let mut files = [CpmFixtureFile::empty(&mut records)];
    let mut options = cpm_options(5_000);
    seed_fcb(&mut options.initial_memory, 0x0400, b"NEWFILE TXT");
    seed_fcb(&mut options.initial_memory, 0x0440, b"MISSING TXT");
    options.initial_memory.push((0x0500, b'X'));
    let command_tail: [u8; 0] = [];
    let console_input: [u8; 0] = [];
    let mut fixture = CpmBdosFixture::new(&mut files, &command_tail, &console_input);
    fixture.inject_disk_full_after(0);
    let run =
        run_assembly_test_with_cpm_bdos_fixture(CpuFamily::Z80, assembly, &options, &mut fixture)
            .unwrap();
    assert_cpm_run(&run, &[0xFF, 0, 1, 0, 0]);
    assert!(!fixture.files[0].present);
}
