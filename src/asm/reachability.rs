//! Removes unreachable routine blocks from emitted assembly.
//!
//! Routine boundaries come only from the labels supplied by the caller. This
//! deliberately leaves local labels within their owning routine block.

use crate::compat::prelude::*;

/// A target-specific description of one control-transfer mnemonic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TransferRule<'a> {
    /// The instruction mnemonic, matched without regard to ASCII case.
    pub mnemonic: &'a str,
    /// The transfer behavior for this mnemonic.
    pub kind: TransferKind,
}

/// The control-flow behavior of a transfer instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransferKind {
    /// A transfer with a direct label operand.
    ///
    /// `target_operand` is zero-based. Conditional calls and branches normally
    /// fall through; unconditional jumps do not.
    Direct {
        target_operand: usize,
        falls_through: bool,
    },
    /// A return instruction, which has no direct target and does not fall through.
    Return,
}

/// Parsing rules for the direct control transfers of one assembly target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TransferParsingProfile<'a> {
    pub rules: &'a [TransferRule<'a>],
}

impl<'a> TransferParsingProfile<'a> {
    pub const fn new(rules: &'a [TransferRule<'a>]) -> Self {
        Self { rules }
    }
}

/// Control-transfer syntax used by one emitted assembly backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RoutineProfile {
    Ez80,
    Z80,
    I8086,
    Lr35902,
    Mos6502,
    Avr,
    M68k,
    M6800,
    Tms9900,
    Dcpu,
}

/// Remove unreachable generated function and runtime blocks.
///
/// Source functions, roots, direct call targets, and externally referenced
/// generated labels are routine entries. Other generated labels stay inside the
/// nearest known routine, so compiler-created control-flow labels cannot be
/// removed independently from their owner. Static-storage labels are excluded.
pub(crate) fn strip_unreachable_generated_routines(
    assembly: &str,
    profile: RoutineProfile,
) -> String {
    let profile = transfer_profile(profile);
    let routine_labels = discover_generated_routine_labels(assembly, profile);
    let banked_payload_roots = labels_referenced_from_banked_payload(assembly, &routine_labels);
    let root_labels = routine_labels
        .iter()
        .copied()
        .filter(|label| is_routine_root(label) || banked_payload_roots.contains(label))
        .collect::<Vec<_>>();

    strip_unreachable_routines(assembly, &routine_labels, &root_labels, profile)
}

/// Strip routine blocks that cannot be reached from `root_labels`.
///
/// `routine_labels` names every routine entry label in `assembly`. Labels not
/// in that list, including local labels, remain part of the surrounding block.
/// All text outside of routine blocks is retained unchanged.
///
/// Direct transfers create edges only when their target is also a supplied
/// routine label. Each routine also falls through to the next supplied routine
/// label unless its final instruction is a return or an unconditional transfer.
pub(crate) fn strip_unreachable_routines(
    assembly: &str,
    routine_labels: &[&str],
    root_labels: &[&str],
    profile: TransferParsingProfile<'_>,
) -> String {
    let lines = assembly.split_inclusive('\n').collect::<Vec<_>>();
    let mut blocks = Vec::<RoutineBlock>::new();
    let mut open_block: Option<usize> = None;
    let mut in_banked_payload = false;

    for (line_index, line) in lines.iter().enumerate() {
        if is_section_boundary(line) {
            if let Some(previous) = open_block.take() {
                blocks[previous].end = line_index;
            }
            continue;
        }
        let Some(label) = line_label(line) else {
            continue;
        };
        if is_banked_payload_start(label) {
            in_banked_payload = true;
            if let Some(previous) = open_block.take() {
                blocks[previous].end = line_index;
            }
            continue;
        }
        if is_banked_payload_end(label) {
            in_banked_payload = false;
            if let Some(previous) = open_block.take() {
                blocks[previous].end = line_index;
            }
            continue;
        }
        if in_banked_payload {
            continue;
        }
        if is_static_data_label(label) {
            if let Some(previous) = open_block.take() {
                blocks[previous].end = line_index;
            }
            continue;
        }
        let Some(label_index) = routine_labels
            .iter()
            .position(|candidate| *candidate == label)
        else {
            continue;
        };
        if let Some(previous) = open_block {
            blocks[previous].end = line_index;
        }
        blocks.push(RoutineBlock {
            label_index,
            start: line_index,
            end: lines.len(),
        });
        open_block = Some(blocks.len() - 1);
    }

    if blocks.is_empty() {
        return assembly.to_owned();
    }

    let mut reachable = vec![false; blocks.len()];
    let mut pending = Vec::new();
    for root in root_labels {
        if let Some(index) = blocks
            .iter()
            .position(|block| routine_labels[block.label_index] == *root)
        {
            pending.push(index);
        }
    }

    while let Some(block_index) = pending.pop() {
        if reachable[block_index] {
            continue;
        }
        reachable[block_index] = true;
        let block = blocks[block_index];

        for line in &lines[block.start..block.end] {
            let Some((_, operands)) = instruction_parts(line) else {
                continue;
            };
            for (target_block, candidate) in blocks.iter().enumerate() {
                if operands_reference_label(operands, routine_labels[candidate.label_index]) {
                    pending.push(target_block);
                }
            }
        }

        if !block_ends_transfer(block, &lines, profile) && block_index + 1 < blocks.len() {
            pending.push(block_index + 1);
        }
    }

    let mut output = String::new();
    for (line_index, line) in lines.iter().enumerate() {
        let in_unreachable_block = blocks.iter().enumerate().any(|(block_index, block)| {
            !reachable[block_index] && (block.start..block.end).contains(&line_index)
        });
        if !in_unreachable_block {
            output.push_str(line);
        }
    }
    output
}

#[derive(Clone, Copy)]
struct RoutineBlock {
    label_index: usize,
    start: usize,
    end: usize,
}

fn block_ends_transfer(
    block: RoutineBlock,
    lines: &[&str],
    profile: TransferParsingProfile<'_>,
) -> bool {
    for line in lines[block.start..block.end].iter().rev() {
        let Some((mnemonic, operands)) = instruction_parts(line) else {
            continue;
        };
        return matches!(
            transfer_kind(mnemonic, operands, profile),
            Some(
                TransferKind::Return
                    | TransferKind::Direct {
                        falls_through: false,
                        ..
                    }
            )
        );
    }
    false
}

fn transfer_kind(
    mnemonic: &str,
    operands: &str,
    profile: TransferParsingProfile<'_>,
) -> Option<TransferKind> {
    profile.rules.iter().find_map(|rule| {
        if !rule.mnemonic.eq_ignore_ascii_case(mnemonic) {
            return None;
        }
        match rule.kind {
            TransferKind::Direct { target_operand, .. }
                if operand_at(operands, target_operand).is_none() =>
            {
                None
            }
            kind => Some(kind),
        }
    })
}

#[derive(Clone, Copy)]
struct LabelDefinition<'a> {
    name: &'a str,
    line: usize,
}

fn discover_generated_routine_labels<'a>(
    assembly: &'a str,
    profile: TransferParsingProfile<'_>,
) -> Vec<&'a str> {
    let lines = assembly.lines().collect::<Vec<_>>();
    let labels = lines
        .iter()
        .enumerate()
        .filter_map(|(line, text)| {
            let name = line_label(text)?;
            (!is_static_data_label(name) && !is_banked_payload_start(name))
                .then_some(LabelDefinition { name, line })
        })
        .collect::<Vec<_>>();

    let mut routine = labels
        .iter()
        .map(|label| is_source_routine_label(label.name) || is_routine_root(label.name))
        .collect::<Vec<_>>();

    for line in &lines {
        let Some((mnemonic, operands)) = instruction_parts(line) else {
            continue;
        };
        if !is_call_mnemonic(mnemonic) {
            continue;
        }
        for (index, label) in labels.iter().enumerate() {
            if operands_reference_label(operands, label.name) {
                routine[index] = true;
            }
        }
    }

    for (index, label) in labels.iter().enumerate() {
        if routine[index] || !label.name.starts_with("__ezra_") {
            continue;
        }
        let region_start = labels[..index]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(candidate, label)| routine[candidate].then_some(label.line))
            .unwrap_or(0);
        let region_end = labels[index + 1..]
            .iter()
            .enumerate()
            .find_map(|(offset, label)| routine[index + 1 + offset].then_some(label.line))
            .unwrap_or(lines.len());
        let referenced_inside_region = lines[region_start..region_end].iter().any(|line| {
            instruction_parts(line)
                .is_some_and(|(_, operands)| operands_reference_label(operands, label.name))
        });
        let referenced_elsewhere = lines[..region_start]
            .iter()
            .chain(&lines[region_end..])
            .any(|line| {
                instruction_parts(line)
                    .is_some_and(|(_, operands)| operands_reference_label(operands, label.name))
            });
        let follows_return = lines[..label.line].iter().rev().find_map(|line| {
            let (mnemonic, operands) = instruction_parts(line)?;
            Some(matches!(
                transfer_kind(mnemonic, operands, profile),
                Some(TransferKind::Return)
            ))
        }) == Some(true);

        if referenced_elsewhere || (!referenced_inside_region && follows_return) {
            routine[index] = true;
        }
    }

    labels
        .iter()
        .zip(routine)
        .filter_map(|(label, routine)| routine.then_some(label.name))
        .collect()
}

fn is_source_routine_label(label: &str) -> bool {
    label.starts_with('_') && !label.starts_with("__")
        || matches!(label, "main" | "start" | "exit" | "reset")
}

fn is_call_mnemonic(mnemonic: &str) -> bool {
    ["call", "jsr", "bsr", "rcall", "bl"]
        .iter()
        .any(|call| mnemonic.eq_ignore_ascii_case(call))
}

fn operands_reference_label(operands: &str, label: &str) -> bool {
    operands
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.')))
        .any(|token| token.eq_ignore_ascii_case(label))
}

fn labels_referenced_from_banked_payload<'a>(
    assembly: &str,
    routine_labels: &[&'a str],
) -> HashSet<&'a str> {
    let mut in_banked_payload = false;
    let mut referenced = HashSet::new();
    for line in assembly.lines() {
        if let Some(label) = line_label(line) {
            if is_banked_payload_start(label) {
                in_banked_payload = true;
                continue;
            }
            if is_banked_payload_end(label) {
                in_banked_payload = false;
                continue;
            }
        }
        if !in_banked_payload {
            continue;
        }
        let Some((_, operands)) = instruction_parts(line) else {
            continue;
        };
        for label in routine_labels {
            if operands_reference_label(operands, label) {
                referenced.insert(*label);
            }
        }
    }
    referenced
}

fn is_banked_payload_start(label: &str) -> bool {
    label.starts_with("__ezra_bank_") && label.ends_with("_start")
}

fn is_banked_payload_end(label: &str) -> bool {
    label.starts_with("__ezra_bank_") && label.ends_with("_end")
}

fn is_static_data_label(label: &str) -> bool {
    matches!(
        label,
        label if label.starts_with("__ezra_global_")
            || label.starts_with("__ezra_embed_")
            || label.starts_with("__ezra_banked_data_")
            || label.starts_with("__ezra_bank_")
            || label.starts_with("__ezra_far_")
    )
}

fn is_routine_root(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "__ezra_start"
            | "_start"
            | "start"
            | "_main"
            | "main"
            | "__ezra_exit"
            | "_exit"
            | "exit"
            | "reset"
            | "_reset"
    ) || lower.starts_with("_avr_")
        || lower.contains("interrupt")
        || lower.contains("vector")
        || lower.contains("_irq")
        || lower.contains("_nmi")
}

fn transfer_profile(profile: RoutineProfile) -> TransferParsingProfile<'static> {
    match profile {
        RoutineProfile::Ez80 | RoutineProfile::Z80 => Z80_PROFILE,
        RoutineProfile::I8086 => I8086_PROFILE,
        RoutineProfile::Lr35902 => LR35902_PROFILE,
        RoutineProfile::Mos6502 => MOS6502_PROFILE,
        RoutineProfile::Avr => AVR_PROFILE,
        RoutineProfile::M68k => M68K_PROFILE,
        RoutineProfile::M6800 => M6800_PROFILE,
        RoutineProfile::Tms9900 => TMS9900_PROFILE,
        RoutineProfile::Dcpu => DCPU_PROFILE,
    }
}

const fn direct(
    mnemonic: &'static str,
    target_operand: usize,
    falls_through: bool,
) -> TransferRule<'static> {
    TransferRule {
        mnemonic,
        kind: TransferKind::Direct {
            target_operand,
            falls_through,
        },
    }
}

const fn returns(mnemonic: &'static str) -> TransferRule<'static> {
    TransferRule {
        mnemonic,
        kind: TransferKind::Return,
    }
}

const Z80_PROFILE: TransferParsingProfile<'static> = TransferParsingProfile::new(&[
    direct("call", 0, true),
    direct("jp", 1, true),
    direct("jp", 0, false),
    direct("jr", 1, true),
    direct("jr", 0, false),
    direct("djnz", 0, true),
    returns("ret"),
    returns("reti"),
]);
const LR35902_PROFILE: TransferParsingProfile<'static> = TransferParsingProfile::new(&[
    direct("call", 1, true),
    direct("call", 0, true),
    direct("jp", 1, true),
    direct("jp", 0, false),
    direct("jr", 1, true),
    direct("jr", 0, false),
    returns("ret"),
    returns("reti"),
]);
const I8086_PROFILE: TransferParsingProfile<'static> = TransferParsingProfile::new(&[
    direct("call", 0, true),
    direct("jmp", 0, false),
    returns("ret"),
    returns("iret"),
]);
const MOS6502_PROFILE: TransferParsingProfile<'static> = TransferParsingProfile::new(&[
    direct("jsr", 0, true),
    direct("jmp", 0, false),
    direct("beq", 0, true),
    direct("bne", 0, true),
    direct("bcc", 0, true),
    direct("bcs", 0, true),
    direct("bpl", 0, true),
    direct("bmi", 0, true),
    returns("rts"),
    returns("rti"),
]);
const AVR_PROFILE: TransferParsingProfile<'static> = TransferParsingProfile::new(&[
    direct("call", 0, true),
    direct("rcall", 0, true),
    direct("jmp", 0, false),
    direct("rjmp", 0, false),
    returns("ret"),
    returns("reti"),
]);
const M68K_PROFILE: TransferParsingProfile<'static> = TransferParsingProfile::new(&[
    direct("jsr", 0, true),
    direct("bsr", 0, true),
    direct("jmp", 0, false),
    direct("bra", 0, false),
    returns("rts"),
    returns("rte"),
]);
const M6800_PROFILE: TransferParsingProfile<'static> = TransferParsingProfile::new(&[
    direct("jsr", 0, true),
    direct("bsr", 0, true),
    direct("jmp", 0, false),
    direct("bra", 0, false),
    returns("rts"),
    returns("rti"),
]);
const TMS9900_PROFILE: TransferParsingProfile<'static> = TransferParsingProfile::new(&[
    direct("bl", 0, true),
    direct("b", 0, false),
    returns("rtwp"),
]);
const DCPU_PROFILE: TransferParsingProfile<'static> =
    TransferParsingProfile::new(&[direct("jsr", 0, true), direct("set", 1, false)]);

fn line_label(line: &str) -> Option<&str> {
    let text = line.trim().trim_end_matches('\r');
    let (label, _) = text.split_once(':')?;
    (!label.is_empty() && !label.chars().any(char::is_whitespace)).then_some(label)
}

fn is_section_boundary(line: &str) -> bool {
    let text = line
        .trim_end_matches(['\r', '\n'])
        .split(';')
        .next()
        .unwrap_or_default()
        .trim_start();
    let mnemonic = text.split_whitespace().next().unwrap_or_default();
    mnemonic.eq_ignore_ascii_case("section") || mnemonic.eq_ignore_ascii_case(".section")
}

fn instruction_parts(line: &str) -> Option<(&str, &str)> {
    let mut text = line
        .trim_end_matches(['\r', '\n'])
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    if let Some((label, rest)) = text.split_once(':')
        && !label.is_empty()
        && !label.chars().any(char::is_whitespace)
    {
        text = rest.trim_start();
    }
    let mnemonic_end = text.find(char::is_whitespace).unwrap_or(text.len());
    (mnemonic_end > 0).then(|| (&text[..mnemonic_end], text[mnemonic_end..].trim()))
}

fn operand_at(operands: &str, index: usize) -> Option<&str> {
    operands
        .split(',')
        .nth(index)
        .map(str::trim)
        .filter(|operand| !operand.is_empty())
}

#[cfg(test)]
mod tests {
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
}
