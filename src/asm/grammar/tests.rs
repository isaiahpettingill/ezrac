
use super::*;

fn parse(cpu: AssemblerCpu, mnemonic: &str, operands: &[&str]) -> ArchitectureInstruction {
    parse_instruction(
        cpu,
        &AssemblyInstruction {
            mnemonic: mnemonic.to_owned(),
            operands: operands
                .iter()
                .map(|operand| (*operand).to_owned())
                .collect(),
        },
    )
    .unwrap()
}

#[test]
fn z80_family_normalizes_indexed_addressing() {
    for cpu in [
        AssemblerCpu::Z80,
        AssemblerCpu::Z80N,
        AssemblerCpu::Z180,
        AssemblerCpu::Ez80,
    ] {
        assert_eq!(
            parse(cpu, "ld", &["a", "( ix    + 1 )"]).encoder_text(),
            "ld a,(ix+1)"
        );
    }
}

#[test]
fn intel8080_normalizes_operand_commas() {
    for cpu in [AssemblerCpu::I8080, AssemblerCpu::I8085] {
        assert_eq!(
            parse(cpu, "lxi", &["h", "1234h"]).encoder_text(),
            "lxi h,1234h"
        );
    }
}

#[cfg(feature = "i8086")]
#[test]
fn i8086_normalizes_memory_addressing() {
    assert_eq!(
        parse(AssemblerCpu::I8086, "mov", &["ax", "[ bx    + si + 4 ]"]).encoder_text(),
        "mov ax,[bx+si+4]"
    );
}

#[test]
fn lr35902_normalizes_sp_offsets() {
    assert_eq!(
        parse(AssemblerCpu::Lr35902, "ld", &["hl", "sp    + 1"]).encoder_text(),
        "ld hl,sp+1"
    );
}

#[test]
fn avr_normalizes_displacement_addressing() {
    assert_eq!(
        parse(AssemblerCpu::Avr, "ldd", &["r1", "y    + 1"]).encoder_text(),
        "ldd r1,y+1"
    );
}

#[test]
fn dcpu_normalizes_brackets_but_preserves_pick_separator() {
    assert_eq!(
        parse(AssemblerCpu::Dcpu, "set", &["a", "[ sp + 1 ]"]).encoder_text(),
        "set a,[sp+1]"
    );
    assert_eq!(
        parse(AssemblerCpu::Dcpu, "set", &["a", "pick    1"]).encoder_text(),
        "set a,pick 1"
    );
}

#[test]
fn m6800_normalizes_expressions_and_indexing() {
    assert_eq!(
        parse(AssemblerCpu::M6800, "ldaa", &["$    + 2"]).encoder_text(),
        "ldaa $+2"
    );
    assert_eq!(
        parse(AssemblerCpu::M6800, "ldaa", &["1", "x"]).encoder_text(),
        "ldaa 1,x"
    );
}

#[test]
fn m6809_accepts_zero_offset_indexing_and_indirection() {
    assert_eq!(
        parse(AssemblerCpu::M6809, "lda", &["", "x"]).encoder_text(),
        "lda ,x"
    );
    assert_eq!(
        parse(AssemblerCpu::M6809, "lda", &["[8", "s]"]).encoder_text(),
        "lda [8,s]"
    );
}

#[test]
fn m68k_normalizes_nested_effective_addresses() {
    assert_eq!(
        parse(
            AssemblerCpu::M68k,
            "move.w",
            &["( 4 , a0 , d0.w * 2 )", "( 8 , a1 )"],
        )
        .encoder_text(),
        "move.w (4,a0,d0.w*2),(8,a1)"
    );
}

#[test]
fn mos6502_normalizes_indirect_indexing() {
    assert_eq!(
        parse(AssemblerCpu::Mos6502, "lda", &["( $20 )", "y"]).encoder_text(),
        "lda ($20),y"
    );
}

#[test]
fn tms9900_normalizes_symbolic_indexing() {
    assert_eq!(
        parse(AssemblerCpu::Tms9900, "a", &["@addr ( r1 )", "r2"]).encoder_text(),
        "a @addr(r1),r2"
    );
}

fn assert_rejected(cpu: AssemblerCpu, operand: &str) {
    let error = parse_instruction(
        cpu,
        &AssemblyInstruction {
            mnemonic: "op".to_owned(),
            operands: vec![operand.to_owned()],
        },
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&format!("invalid {} operand syntax", cpu.as_str())),
        "{cpu:?} unexpectedly accepted `{operand}`"
    );
    assert!(error.location().is_some());
}

#[test]
fn every_architecture_parser_rejects_unbalanced_and_empty_groups() {
    for (cpu, unbalanced) in [
        (AssemblerCpu::I8080, "(1"),
        #[cfg(feature = "i8086")]
        (AssemblerCpu::I8086, "[bx+si"),
        (AssemblerCpu::Z80, "(ix+1"),
        (AssemblerCpu::Lr35902, "[hl"),
        (AssemblerCpu::Avr, "(1"),
        (AssemblerCpu::Dcpu, "[sp+1"),
        (AssemblerCpu::M6800, "(1"),
        (AssemblerCpu::M68k, "(4,a0"),
        (AssemblerCpu::Mos6502, "($20"),
        (AssemblerCpu::Tms9900, "@addr(r1"),
    ] {
        assert_rejected(cpu, unbalanced);
        assert_rejected(cpu, "()");
    }
}

#[test]
fn family_parsers_enforce_architecture_delimiter_shapes() {
    for cpu in [
        AssemblerCpu::I8080,
        AssemblerCpu::Avr,
        AssemblerCpu::M6800,
        AssemblerCpu::Tms9900,
    ] {
        assert_rejected(cpu, "[value]");
    }
    assert_rejected(AssemblerCpu::Dcpu, "[[sp+1]]");
}

#[test]
fn architecture_parsers_reject_empty_top_level_operands() {
    assert_rejected(AssemblerCpu::Z80, "");
}
