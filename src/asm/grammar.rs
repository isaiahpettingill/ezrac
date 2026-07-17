//! Pest-backed instruction and operand parsing for each supported CPU family.
//!
//! Every parser concatenates the shared assembly grammar with exactly one
//! family grammar. The resulting Pest pairs are the authority for delimiter
//! validation, operand boundaries, and canonical encoder spelling.

use crate::asm::frontend::AssemblyInstruction;
use crate::compat::prelude::*;
use crate::diagnostic::{Diagnostic, SourcePosition, SourceSpan};
use crate::target::AssemblerCpu;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArchitectureInstruction {
    instruction: AssemblyInstruction,
    encoder_text: String,
}

impl ArchitectureInstruction {
    pub(crate) fn instruction(&self) -> &AssemblyInstruction {
        &self.instruction
    }

    pub(crate) fn encoder_text(&self) -> &str {
        &self.encoder_text
    }

    pub(crate) fn with_mnemonic(&self, mnemonic: &str) -> Self {
        let instruction = AssemblyInstruction {
            mnemonic: mnemonic.to_owned(),
            operands: self.instruction.operands.clone(),
        };
        let encoder_text = canonical_text(&instruction.mnemonic, &instruction.operands);
        Self {
            instruction,
            encoder_text,
        }
    }
}

pub(crate) fn parse_instruction(
    cpu: AssemblerCpu,
    instruction: &AssemblyInstruction,
) -> Result<ArchitectureInstruction, Diagnostic> {
    if instruction
        .operands
        .iter()
        .any(|operand| operand.trim().is_empty())
    {
        return Err(Diagnostic::at_span(
            SourceSpan {
                file: format!("<{} instruction>", cpu.as_str()).into(),
                start: SourcePosition { line: 1, column: 1 },
                end: SourcePosition { line: 1, column: 2 },
            },
            format!("invalid {} operand syntax: empty operand", cpu.as_str()),
        ));
    }
    let source = instruction.to_text();
    match cpu {
        AssemblerCpu::I8080 | AssemblerCpu::I8085 => intel8080::parse(cpu, &source),
        AssemblerCpu::I8086 => {
            #[cfg(feature = "i8086")]
            {
                i8086::parse(cpu, &source)
            }
            #[cfg(not(feature = "i8086"))]
            {
                Err(Diagnostic::new(
                    "i8086 instruction parsing requires the `i8086` Cargo feature",
                ))
            }
        }
        AssemblerCpu::Z80 | AssemblerCpu::Z80N | AssemblerCpu::Z180 | AssemblerCpu::Ez80 => {
            z80::parse(cpu, &source)
        }
        AssemblerCpu::Lr35902 => lr35902::parse(cpu, &source),
        AssemblerCpu::Avr => avr::parse(cpu, &source),
        AssemblerCpu::Dcpu => dcpu::parse(cpu, &source),
        AssemblerCpu::M6800 => m6800::parse(cpu, &source),
        AssemblerCpu::M68k => m68k::parse(cpu, &source),
        AssemblerCpu::Mos6502
        | AssemblerCpu::Cmos65C02
        | AssemblerCpu::Wdc65C816
        | AssemblerCpu::Ricoh2A03 => mos6502::parse(cpu, &source),
        AssemblerCpu::Tms9900 => tms9900::parse(cpu, &source),
    }
}

fn canonical_text(mnemonic: &str, operands: &[String]) -> String {
    if operands.is_empty() {
        mnemonic.to_owned()
    } else {
        format!("{mnemonic} {}", operands.join(","))
    }
}

fn pest_diagnostic<R: pest::RuleType>(
    cpu: AssemblerCpu,
    error: pest::error::Error<R>,
) -> Diagnostic {
    let ((line, column), (end_line, end_column)) = match error.line_col {
        pest::error::LineColLocation::Pos((line, column)) => {
            ((line, column), (line, column.saturating_add(1)))
        }
        pest::error::LineColLocation::Span(start, end) => (start, end),
    };
    Diagnostic::at_span(
        SourceSpan {
            file: format!("<{} instruction>", cpu.as_str()).into(),
            start: SourcePosition { line, column },
            end: SourcePosition {
                line: end_line,
                column: end_column,
            },
        },
        format!("invalid {} operand syntax: {error}", cpu.as_str()),
    )
}

macro_rules! architecture_parser {
    ($module:ident, $grammar:literal, $operand:ident) => {
        mod $module {
            use pest::{Parser as _, iterators::Pair};
            use pest_derive::Parser;

            use super::{
                ArchitectureInstruction, AssemblerCpu, Diagnostic, canonical_text, pest_diagnostic,
            };
            use crate::asm::frontend::AssemblyInstruction;
            use crate::compat::prelude::*;

            #[derive(Parser)]
            #[grammar = "asm/assembly.pest"]
            #[grammar = $grammar]
            struct FamilyParser;

            pub(super) fn parse(
                cpu: AssemblerCpu,
                source: &str,
            ) -> Result<ArchitectureInstruction, Diagnostic> {
                let mut parsed = FamilyParser::parse(Rule::architecture_instruction, source)
                    .map_err(|error| pest_diagnostic(cpu, error))?;
                let root = parsed.next().ok_or_else(|| {
                    Diagnostic::new(format!(
                        "{} architecture parser produced no instruction",
                        cpu.as_str()
                    ))
                })?;
                let mut mnemonic = None;
                let mut operands = Vec::new();
                for pair in root.into_inner() {
                    match pair.as_rule() {
                        Rule::architecture_mnemonic => {
                            mnemonic = Some(pair.as_str().to_owned());
                        }
                        Rule::architecture_operands => {
                            for operand in pair.into_inner() {
                                if operand.as_rule() == Rule::$operand {
                                    operands.push(canonical_operand(operand));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let mnemonic = mnemonic.ok_or_else(|| {
                    Diagnostic::new(format!(
                        "{} architecture parser produced no mnemonic",
                        cpu.as_str()
                    ))
                })?;
                let encoder_text = canonical_text(&mnemonic, &operands);
                Ok(ArchitectureInstruction {
                    instruction: AssemblyInstruction { mnemonic, operands },
                    encoder_text,
                })
            }

            fn canonical_operand(pair: Pair<'_, Rule>) -> String {
                fn append(pair: Pair<'_, Rule>, output: &mut String, previous_atom: &mut bool) {
                    match pair.as_rule() {
                        Rule::architecture_atom
                        | Rule::architecture_double_quoted
                        | Rule::architecture_single_quoted => {
                            if *previous_atom {
                                output.push(' ');
                            }
                            output.push_str(pair.as_str());
                            *previous_atom = true;
                        }
                        Rule::architecture_operator
                        | Rule::architecture_comma
                        | Rule::architecture_lparen
                        | Rule::architecture_rparen
                        | Rule::architecture_lbracket
                        | Rule::architecture_rbracket
                        | Rule::architecture_lbrace
                        | Rule::architecture_rbrace => {
                            output.push_str(pair.as_str());
                            *previous_atom = false;
                        }
                        _ => {
                            for inner in pair.into_inner() {
                                append(inner, output, previous_atom);
                            }
                        }
                    }
                }

                let mut output = String::new();
                let mut previous_atom = false;
                append(pair, &mut output, &mut previous_atom);
                output
            }
        }
    };
}

architecture_parser!(z80, "asm/grammar/z80.pest", z80_operand);
architecture_parser!(intel8080, "asm/grammar/intel8080.pest", intel8080_operand);
#[cfg(feature = "i8086")]
architecture_parser!(i8086, "asm/grammar/i8086.pest", i8086_operand);
architecture_parser!(lr35902, "asm/grammar/lr35902.pest", lr35902_operand);
architecture_parser!(avr, "asm/grammar/avr.pest", avr_operand);
architecture_parser!(dcpu, "asm/grammar/dcpu.pest", dcpu_operand);
architecture_parser!(m6800, "asm/grammar/m6800.pest", m6800_operand);
architecture_parser!(m68k, "asm/grammar/m68k.pest", m68k_operand);
architecture_parser!(mos6502, "asm/grammar/mos6502.pest", mos6502_operand);
architecture_parser!(tms9900, "asm/grammar/tms9900.pest", tms9900_operand);

#[cfg(test)]
mod tests {
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
}
