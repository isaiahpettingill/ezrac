//! Conservative redundant register-copy cleanup for generated assembly.

use crate::{compat::prelude::*, target::CpuFamily};

/// Remove register-only copies that cannot change machine state.
///
/// The pass ignores memory operands and inline assembly. Across generated code it
/// removes adjacent duplicate copies. It also removes self-copies on targets whose
/// copy instruction does not update flags.
pub(crate) fn remove_redundant_register_copies(assembly: &str, cpu: CpuFamily) -> String {
    let mut output = String::new();
    let mut previous_copy: Option<(String, String, String)> = None;
    let mut in_inline_asm = false;

    for line in assembly.lines() {
        let trimmed = line.trim();
        if trimmed == "; end asm" {
            in_inline_asm = false;
            previous_copy = None;
            push_line(&mut output, line);
            continue;
        }
        if trimmed.starts_with("; asm") {
            in_inline_asm = true;
            previous_copy = None;
            push_line(&mut output, line);
            continue;
        }

        let copy = (!in_inline_asm)
            .then(|| parse_register_copy(trimmed, cpu))
            .flatten();
        if let Some((mnemonic, target, source, self_copy_is_safe)) = copy.as_ref() {
            let duplicate = previous_copy.as_ref().is_some_and(|previous| {
                previous.0 == *mnemonic && previous.1 == *target && previous.2 == *source
            });
            if duplicate || (*self_copy_is_safe && target == source) {
                continue;
            }
            previous_copy = Some((mnemonic.clone(), target.clone(), source.clone()));
        } else if !trimmed.is_empty() && !trimmed.starts_with(';') {
            previous_copy = None;
        }

        push_line(&mut output, line);
    }

    output
}

fn push_line(output: &mut String, line: &str) {
    output.push_str(line);
    output.push('\n');
}

fn parse_register_copy(line: &str, cpu: CpuFamily) -> Option<(String, String, String, bool)> {
    if line.is_empty() || line.ends_with(':') || line.starts_with("section ") {
        return None;
    }
    let normalized = line.to_ascii_lowercase();
    let (mnemonic, operands) = normalized.split_once(char::is_whitespace)?;
    let (first, second) = operands.split_once(',')?;
    let first = first.trim();
    let second = second.trim();

    let (target, source, self_copy_is_safe) = match cpu {
        CpuFamily::I8086 if mnemonic == "mov" => (first, second, true),
        CpuFamily::Avr if matches!(mnemonic, "mov" | "movw") => (first, second, true),
        CpuFamily::Lr35902 if mnemonic == "ld" => (first, second, true),
        CpuFamily::M6800 | CpuFamily::M6809 if mnemonic == "tfr" => (second, first, true),
        CpuFamily::M68k if mnemonic == "move" || mnemonic.starts_with("move.") => {
            // MOVE updates N/Z/V/C, so a lone self-copy is not removable.
            (second, first, false)
        }
        CpuFamily::Tms9900 if mnemonic == "mov" => {
            // MOV updates comparison/parity status bits.
            (second, first, false)
        }
        CpuFamily::Dcpu if mnemonic == "set" => (first, second, true),
        _ => return None,
    };

    if !is_register(target, cpu) || !is_register(source, cpu) {
        return None;
    }
    Some((
        mnemonic.to_owned(),
        target.to_owned(),
        source.to_owned(),
        self_copy_is_safe,
    ))
}

fn is_register(register: &str, cpu: CpuFamily) -> bool {
    match cpu {
        CpuFamily::I8086 => matches!(
            register,
            "al" | "ah"
                | "ax"
                | "bl"
                | "bh"
                | "bx"
                | "cl"
                | "ch"
                | "cx"
                | "dl"
                | "dh"
                | "dx"
                | "si"
                | "di"
                | "bp"
                | "sp"
        ),
        CpuFamily::Avr => avr_register(register),
        CpuFamily::Lr35902 => matches!(
            register,
            "a" | "b" | "c" | "d" | "e" | "h" | "l" | "af" | "bc" | "de" | "hl" | "sp"
        ),
        CpuFamily::M6800 => matches!(register, "a" | "b" | "d" | "x" | "s"),
        CpuFamily::M6809 => matches!(register, "a" | "b" | "d" | "x" | "y" | "u" | "s" | "dp"),
        CpuFamily::M68k => m68k_register(register),
        CpuFamily::Tms9900 => tms_register(register),
        CpuFamily::Dcpu => matches!(
            register,
            "a" | "b" | "c" | "x" | "y" | "z" | "i" | "j" | "sp" | "ex"
        ),
        _ => false,
    }
}

fn avr_register(register: &str) -> bool {
    register
        .strip_prefix('r')
        .and_then(|value| value.parse::<u8>().ok())
        .is_some_and(|value| value <= 31)
}

fn m68k_register(register: &str) -> bool {
    register == "sp"
        || register
            .strip_prefix(['d', 'a'])
            .and_then(|value| value.parse::<u8>().ok())
            .is_some_and(|value| value <= 7)
}

fn tms_register(register: &str) -> bool {
    register
        .strip_prefix('r')
        .and_then(|value| value.parse::<u8>().ok())
        .is_some_and(|value| value <= 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_safe_self_copies_and_adjacent_duplicate_copies() {
        let i8086 = remove_redundant_register_copies(
            "    mov ax, ax\n    mov bx, ax\n; generated comment\n    mov bx, ax\n",
            CpuFamily::I8086,
        );
        assert!(!i8086.contains("mov ax, ax"), "{i8086}");
        assert_eq!(i8086.matches("mov bx, ax").count(), 1, "{i8086}");

        let m68k = remove_redundant_register_copies(
            "    move.w d0, d0\n    move.w d1, d0\n    move.w d1, d0\n",
            CpuFamily::M68k,
        );
        assert!(m68k.contains("move.w d0, d0"), "{m68k}");
        assert_eq!(m68k.matches("move.w d1, d0").count(), 1, "{m68k}");
    }

    #[test]
    fn preserves_memory_copies_inline_assembly_and_dcpu_pc_writes() {
        let i8086 = remove_redundant_register_copies(
            "    mov ax, [value]\n    mov ax, [value]\n; asm volatile\n    mov ax, ax\n; end asm\n",
            CpuFamily::I8086,
        );
        assert_eq!(i8086.matches("mov ax, [value]").count(), 2, "{i8086}");
        assert!(i8086.contains("mov ax, ax"), "{i8086}");

        let dcpu = remove_redundant_register_copies("    set pc, pc\n", CpuFamily::Dcpu);
        assert!(dcpu.contains("set pc, pc"), "{dcpu}");
    }
}
