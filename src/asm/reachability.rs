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
/// The discovery deliberately recognizes only generated callable labels: source
/// functions use a single leading underscore and runtime labels use
/// `__ezra_`. Static-storage labels are excluded. Startup, exit, reset, and
/// interrupt/vector labels are roots so platform entry paths remain intact.
pub(crate) fn strip_unreachable_generated_routines(
    assembly: &str,
    profile: RoutineProfile,
) -> String {
    let routine_labels = assembly
        .lines()
        .filter_map(line_label)
        .filter(|label| is_generated_routine_label(label))
        .collect::<Vec<_>>();
    let root_labels = routine_labels
        .iter()
        .copied()
        .filter(|label| is_routine_root(label))
        .collect::<Vec<_>>();

    strip_unreachable_routines(
        assembly,
        &routine_labels,
        &root_labels,
        transfer_profile(profile),
    )
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
            let Some((mnemonic, operands)) = instruction_parts(line) else {
                continue;
            };
            let Some(target_operand) = direct_transfer(mnemonic, operands, profile) else {
                continue;
            };
            let Some(target) = operand_at(operands, target_operand) else {
                continue;
            };
            if let Some(target_block) = blocks
                .iter()
                .position(|candidate| target_matches(routine_labels[candidate.label_index], target))
            {
                pending.push(target_block);
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

fn direct_transfer(
    mnemonic: &str,
    operands: &str,
    profile: TransferParsingProfile<'_>,
) -> Option<usize> {
    match transfer_kind(mnemonic, operands, profile)? {
        TransferKind::Direct {
            target_operand,
            falls_through: _,
        } => Some(target_operand),
        TransferKind::Return => None,
    }
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

fn target_matches(label: &str, operand: &str) -> bool {
    let target = operand
        .trim()
        .trim_start_matches(['@', '#'])
        .strip_prefix("near ")
        .or_else(|| {
            operand
                .trim()
                .trim_start_matches(['@', '#'])
                .strip_prefix("far ")
        })
        .or_else(|| {
            operand
                .trim()
                .trim_start_matches(['@', '#'])
                .strip_prefix("short ")
        })
        .unwrap_or_else(|| operand.trim().trim_start_matches(['@', '#']));
    label.eq_ignore_ascii_case(target.trim())
}

fn is_generated_routine_label(label: &str) -> bool {
    if !label.starts_with('_') {
        return matches!(label, "main" | "start" | "exit" | "reset");
    }
    !matches!(
        label,
        label if label.starts_with("__ezra_global_")
            || label.starts_with("__ezra_embed_")
            || label.starts_with("__ezra_banked_data_")
            || label.starts_with("__ezra_bank_")
            || label.starts_with("__ezra_far_")
    )
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
    fn preserves_interrupt_roots() {
        let assembly =
            "__ezra_start:\n    ret\n_avr_timer0_ovf:\n    ret\n__ezra_unused_helper:\n    ret\n";

        assert!(
            strip_unreachable_generated_routines(assembly, RoutineProfile::Avr)
                .contains("_avr_timer0_ovf:")
        );
    }
}
