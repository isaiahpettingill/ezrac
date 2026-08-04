
use super::*;

const Z80_RULES: &[TransferRule<'_>] = &[
    TransferRule {
        mnemonic: "call",
        kind: TransferKind::Direct {
            target_operand: 0,
            falls_through: true,
        },
    },
    TransferRule {
        mnemonic: "jp",
        kind: TransferKind::Direct {
            target_operand: 1,
            falls_through: true,
        },
    },
    TransferRule {
        mnemonic: "jp",
        kind: TransferKind::Direct {
            target_operand: 0,
            falls_through: false,
        },
    },
    TransferRule {
        mnemonic: "ret",
        kind: TransferKind::Return,
    },
];

const PROFILE: TransferParsingProfile<'_> = TransferParsingProfile::new(Z80_RULES);

#[test]
fn keeps_roots_and_directly_reachable_routines() {
    let assembly =
        "setup:\n    call used\n    ret\nused:\n    jp done\ndone:\n    ret\ndead:\n    ret\n";

    assert_eq!(
        strip_unreachable_routines(
            assembly,
            &["setup", "used", "done", "dead"],
            &["setup"],
            PROFILE
        ),
        "setup:\n    call used\n    ret\nused:\n    jp done\ndone:\n    ret\n"
    );
}

#[test]
fn conditional_transfers_and_calls_fall_through() {
    let assembly =
        "root:\n    jp nz, branch\nfallthrough:\n    ret\nbranch:\n    ret\ndead:\n    ret\n";

    assert_eq!(
        strip_unreachable_routines(
            assembly,
            &["root", "branch", "fallthrough", "dead"],
            &["root"],
            PROFILE
        ),
        "root:\n    jp nz, branch\nfallthrough:\n    ret\nbranch:\n    ret\n"
    );
}

#[test]
fn local_labels_do_not_start_or_split_routine_blocks() {
    let assembly = "root:\n.local:\n    call helper\n    ret\nhelper:\n    ret\ndead:\n.local_dead:\n    ret\n";

    assert_eq!(
        strip_unreachable_routines(assembly, &["root", "helper", "dead"], &["root"], PROFILE),
        "root:\n.local:\n    call helper\n    ret\nhelper:\n    ret\n"
    );
}

#[test]
fn preserves_non_routine_text() {
    let assembly = "section code\nentry:\n    call live\nlive:\n    ret\nsection data\ntable:\n    db 1, 2\ndead:\n    ret\n";

    assert_eq!(
        strip_unreachable_routines(assembly, &["entry", "live", "dead"], &["entry"], PROFILE),
        "section code\nentry:\n    call live\nlive:\n    ret\nsection data\ntable:\n    db 1, 2\n"
    );
}

#[test]
fn discovers_generated_routines_but_not_static_data() {
    let assembly = "__ezra_start:\n    call _main\n__ezra_exit:\n    jp __ezra_exit\n_main:\n    ret\n__ezra_unused_helper:\n    ret\n__ezra_global_value:\n    db 1\n";

    assert_eq!(
        strip_unreachable_generated_routines(assembly, RoutineProfile::Ez80),
        "__ezra_start:\n    call _main\n__ezra_exit:\n    jp __ezra_exit\n_main:\n    ret\n__ezra_global_value:\n    db 1\n"
    );
}

#[test]
fn preserves_banked_payload_functions_and_symbols() {
    let assembly = "__ezra_start:\n    call _main\n_main:\n    call __ezra_far_worker\n    ret\n__ezra_bank_2_start:\n_worker:\n    ret\n__ezra_bank_2_end:\n__ezra_far_worker:\n    ret\n__ezra_unused_helper:\n    ret\n";

    let output = strip_unreachable_generated_routines(assembly, RoutineProfile::Lr35902);
    assert!(output.contains("_worker:\n"), "{output}");
    assert!(output.contains("__ezra_bank_2_end:\n"), "{output}");
    assert!(!output.contains("__ezra_unused_helper:"), "{output}");
}

#[test]
fn preserves_interrupt_roots() {
    let assembly =
        "__ezra_start:\n    ret\n_avr_timer0_ovf:\n    ret\n__ezra_unused_helper:\n    ret\n";

    assert!(
        strip_unreachable_generated_routines(assembly, RoutineProfile::Avr)
            .contains("_avr_timer0_ovf:")
    );
}

#[test]
fn compiler_control_flow_labels_stay_inside_their_routine() {
    let assembly = "__ezra_start:\n    jsr _main\n    rts\n_main:\n    cmp.l d1,d0\n    bne __ezra_true_3\n    moveq #0,d0\n    bra __ezra_done_4\n__ezra_true_3:\n    moveq #1,d0\n__ezra_done_4:\n    rts\n__ezra_unused_helper:\n    rts\n";

    let output = strip_unreachable_generated_routines(assembly, RoutineProfile::M68k);

    assert!(output.contains("__ezra_true_3:"), "{output}");
    assert!(output.contains("__ezra_done_4:"), "{output}");
    assert!(!output.contains("__ezra_unused_helper:"), "{output}");
}

#[test]
fn generated_indirect_return_label_stays_with_called_runtime() {
    let assembly = "__ezra_start:\n    call __ezra_gb_far_call\n    ret\n__ezra_gb_far_call:\n    ld de, __ezra_gb_far_return\n    push de\n    jp hl\n__ezra_gb_far_return:\n    pop af\n    ret\n__ezra_unused_helper:\n    ret\n";

    let output = strip_unreachable_generated_routines(assembly, RoutineProfile::Lr35902);

    assert!(output.contains("__ezra_gb_far_call:"), "{output}");
    assert!(output.contains("__ezra_gb_far_return:"), "{output}");
    assert!(!output.contains("__ezra_unused_helper:"), "{output}");
}

#[test]
fn address_taken_trampoline_and_callee_survive() {
    let assembly = "__ezra_start:\n    mov ax,__ezra_fn_ptr_callback\n    ret\n_callback:\n    ret\n__ezra_fn_ptr_callback:\n    call near _callback\n    ret\n__ezra_unused_helper:\n    ret\n";

    let output = strip_unreachable_generated_routines(assembly, RoutineProfile::I8086);

    assert!(output.contains("_callback:"), "{output}");
    assert!(output.contains("__ezra_fn_ptr_callback:"), "{output}");
    assert!(!output.contains("__ezra_unused_helper:"), "{output}");
}

#[test]
fn resident_routines_called_from_banked_payloads_are_roots() {
    let assembly = "__ezra_start:\n    call _main\n    ret\n_main:\n    call __ezra_far_draw\n    ret\n__ezra_bank_2_start:\n_draw:\n    call _video.begin_update\n    ret\n__ezra_bank_2_end:\n_video.begin_update:\n    ret\n_unused:\n    ret\n__ezra_far_draw:\n    ret\n";

    let output = strip_unreachable_generated_routines(assembly, RoutineProfile::Lr35902);

    assert!(output.contains("_video.begin_update:"), "{output}");
    assert!(!output.contains("_unused:"), "{output}");
}

#[test]
fn mos6502_conditional_branches_keep_generated_helper_targets_and_fallthrough() {
    let assembly = "__ezra_start:\n    beq __ezra_equal\n__ezra_fallthrough:\n    rts\n__ezra_equal:\n    bne __ezra_not_equal\n    bcc __ezra_carry_clear\n    bcs __ezra_carry_set\n    bpl __ezra_positive\n    bmi __ezra_negative\n    rts\n__ezra_not_equal:\n    rts\n__ezra_carry_clear:\n    rts\n__ezra_carry_set:\n    rts\n__ezra_positive:\n    rts\n__ezra_negative:\n    rts\n__ezra_dead:\n    rts\n";

    let output = strip_unreachable_generated_routines(assembly, RoutineProfile::Mos6502);
    for label in [
        "__ezra_fallthrough:",
        "__ezra_equal:",
        "__ezra_not_equal:",
        "__ezra_carry_clear:",
        "__ezra_carry_set:",
        "__ezra_positive:",
        "__ezra_negative:",
    ] {
        assert!(output.contains(label), "missing {label}\n{output}");
    }
    assert!(!output.contains("__ezra_dead:"), "{output}");
}
