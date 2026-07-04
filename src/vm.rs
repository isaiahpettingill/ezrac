use std::{cell::Cell, collections::HashMap};

use ez80::{Cpu, Machine};

use crate::diagnostic::Diagnostic;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestRun {
    pub halted: bool,
    pub result_code: u8,
    pub instructions: u64,
    pub debug_output: Vec<u8>,
}

pub fn run_assembly_test(assembly: &str, instruction_budget: u64) -> Result<TestRun, Diagnostic> {
    let code = assemble_subset(assembly)?;
    let mut machine = TestMachine::new();
    for (address, byte) in code.into_iter().enumerate() {
        machine.poke(address as u32, byte);
    }

    let mut cpu = Cpu::new_ez80();
    cpu.state.reg.adl = true;
    cpu.state.set_pc(0);
    if std::env::var_os("EZRA_TRACE_VM").is_some() {
        cpu.set_trace(true);
    }

    for instruction in 0..instruction_budget {
        cpu.execute_instruction(&mut machine);
        if machine.halted {
            return Ok(TestRun {
                halted: true,
                result_code: machine.result_code,
                instructions: instruction + 1,
                debug_output: machine.debug_output,
            });
        }
    }

    Ok(TestRun {
        halted: false,
        result_code: machine.result_code,
        instructions: instruction_budget,
        debug_output: machine.debug_output,
    })
}

fn assemble_subset(assembly: &str) -> Result<Vec<u8>, Diagnostic> {
    let instructions = assembly.lines().filter_map(parse_line).collect::<Vec<_>>();
    let mut labels = HashMap::new();
    let mut pc = 0u32;

    for instruction in &instructions {
        match instruction {
            AsmLine::Label(name) => {
                labels.insert(name.clone(), pc);
            }
            AsmLine::Instruction(text) => pc += instruction_len(text)? as u32,
        }
    }

    let mut bytes = Vec::new();
    for instruction in instructions {
        if let AsmLine::Instruction(text) = instruction {
            emit_instruction(&text, &labels, &mut bytes)?;
        }
    }
    Ok(bytes)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AsmLine {
    Label(String),
    Instruction(String),
}

fn parse_line(line: &str) -> Option<AsmLine> {
    let line = line.split(';').next().unwrap_or("").trim();
    if line.is_empty() || line.starts_with("section ") {
        return None;
    }
    if let Some(label) = line.strip_suffix(':') {
        return Some(AsmLine::Label(label.to_owned()));
    }
    Some(AsmLine::Instruction(line.to_owned()))
}

fn instruction_len(text: &str) -> Result<usize, Diagnostic> {
    if text.starts_with("ld sp,")
        || text.starts_with("call ")
        || text.starts_with("jp z,")
        || text.starts_with("jp nz,")
        || text.starts_with("jp c,")
        || text.starts_with("jp nc,")
        || text.starts_with("jp ")
    {
        Ok(4)
    } else if matches!(
        text,
        "ret"
            | "or a"
            | "ex de, hl"
            | "push bc"
            | "push hl"
            | "pop bc"
            | "pop hl"
            | "ld b, a"
            | "ld c, a"
            | "ld a, b"
            | "ld a, c"
            | "ld a, h"
            | "ld a, l"
            | "ld h, b"
            | "ld h, a"
            | "ld l, c"
            | "ld l, a"
            | "add hl, bc"
            | "add a, b"
            | "add a, c"
            | "sub b"
            | "sub c"
            | "and b"
            | "and c"
            | "or b"
            | "or c"
            | "xor b"
            | "xor c"
            | "xor a"
            | "cp b"
            | "cp c"
    ) {
        Ok(1)
    } else if text == "sbc hl, bc" || text == "sbc hl, de" {
        Ok(2)
    } else if text.starts_with("ld hl, (")
        || text.starts_with("ld a, (")
        || text.starts_with("ld (")
    {
        Ok(4)
    } else if text.starts_with("ld hl,") {
        Ok(4)
    } else if text.starts_with("ld h,") || text.starts_with("ld a,") || text.starts_with("in0 ") {
        Ok(2)
    } else if text.starts_with("xor ") {
        Ok(2)
    } else if text.starts_with("out0 ") {
        Ok(3)
    } else {
        Err(Diagnostic::new(format!(
            "test assembler does not support instruction `{text}`"
        )))
    }
}

fn emit_instruction(
    text: &str,
    labels: &HashMap<String, u32>,
    bytes: &mut Vec<u8>,
) -> Result<(), Diagnostic> {
    if let Some(value) = text.strip_prefix("ld sp,") {
        bytes.push(0x31);
        push24(bytes, parse_addr(value.trim(), labels)?);
    } else if let Some(target) = text.strip_prefix("call ") {
        bytes.push(0xCD);
        push24(bytes, parse_addr(target.trim(), labels)?);
    } else if let Some(target) = text.strip_prefix("jp z,") {
        bytes.push(0xCA);
        push24(bytes, parse_addr(target.trim(), labels)?);
    } else if let Some(target) = text.strip_prefix("jp nz,") {
        bytes.push(0xC2);
        push24(bytes, parse_addr(target.trim(), labels)?);
    } else if let Some(target) = text.strip_prefix("jp c,") {
        bytes.push(0xDA);
        push24(bytes, parse_addr(target.trim(), labels)?);
    } else if let Some(target) = text.strip_prefix("jp nc,") {
        bytes.push(0xD2);
        push24(bytes, parse_addr(target.trim(), labels)?);
    } else if let Some(target) = text.strip_prefix("jp ") {
        bytes.push(0xC3);
        push24(bytes, parse_addr(target.trim(), labels)?);
    } else if let Some(rest) = text.strip_prefix("ld hl, (") {
        let addr = rest
            .strip_suffix(')')
            .ok_or_else(|| Diagnostic::new(format!("invalid load syntax `{text}`")))?;
        bytes.push(0x2A);
        push24(bytes, parse_addr(addr, labels)?);
    } else if let Some(rest) = text.strip_prefix("ld a, (") {
        let addr = rest
            .strip_suffix(')')
            .ok_or_else(|| Diagnostic::new(format!("invalid load syntax `{text}`")))?;
        bytes.push(0x3A);
        push24(bytes, parse_addr(addr, labels)?);
    } else if let Some(rest) = text.strip_prefix("ld (") {
        if let Some(addr) = rest.strip_suffix("), a") {
            bytes.push(0x32);
            push24(bytes, parse_addr(addr, labels)?);
        } else if let Some(addr) = rest.strip_suffix("), hl") {
            bytes.push(0x22);
            push24(bytes, parse_addr(addr, labels)?);
        } else {
            return Err(Diagnostic::new(format!("invalid store syntax `{text}`")));
        }
    } else if let Some(value) = text.strip_prefix("ld hl,") {
        bytes.push(0x21);
        push24(bytes, parse_addr(value.trim(), labels)?);
    } else if let Some(rest) = text.strip_prefix("in0 ") {
        let port = rest
            .trim()
            .strip_prefix("a, (")
            .and_then(|rest| rest.strip_suffix(')'))
            .ok_or_else(|| Diagnostic::new(format!("invalid in0 syntax `{text}`")))?;
        bytes.extend([0xED, 0x38, parse_u8(port)?]);
    } else if let Some(rest) = text.strip_prefix("out0 ") {
        let port = rest
            .trim()
            .strip_prefix('(')
            .and_then(|rest| rest.split_once(')'))
            .ok_or_else(|| Diagnostic::new(format!("invalid out0 syntax `{text}`")))?
            .0;
        bytes.extend([0xED, 0x39, parse_u8(port)?]);
    } else if text == "ret" {
        bytes.push(0xC9);
    } else if text == "or a" {
        bytes.push(0xB7);
    } else if text == "ex de, hl" {
        bytes.push(0xEB);
    } else if text == "push bc" {
        bytes.push(0xC5);
    } else if text == "push hl" {
        bytes.push(0xE5);
    } else if text == "pop bc" {
        bytes.push(0xC1);
    } else if text == "pop hl" {
        bytes.push(0xE1);
    } else if text == "ld b, a" {
        bytes.push(0x47);
    } else if text == "ld c, a" {
        bytes.push(0x4F);
    } else if text == "ld a, b" {
        bytes.push(0x78);
    } else if text == "ld a, c" {
        bytes.push(0x79);
    } else if text == "ld a, h" {
        bytes.push(0x7C);
    } else if text == "ld a, l" {
        bytes.push(0x7D);
    } else if text == "ld h, b" {
        bytes.push(0x60);
    } else if text == "ld h, a" {
        bytes.push(0x67);
    } else if let Some(value) = text.strip_prefix("ld h,") {
        bytes.push(0x26);
        bytes.push(parse_u8(value.trim())?);
    } else if text == "ld l, a" {
        bytes.push(0x6F);
    } else if text == "ld l, c" {
        bytes.push(0x69);
    } else if text == "add hl, bc" {
        bytes.push(0x09);
    } else if text == "sbc hl, bc" {
        bytes.extend([0xED, 0x42]);
    } else if text == "sbc hl, de" {
        bytes.extend([0xED, 0x52]);
    } else if text == "add a, b" {
        bytes.push(0x80);
    } else if text == "add a, c" {
        bytes.push(0x81);
    } else if text == "sub b" {
        bytes.push(0x90);
    } else if text == "sub c" {
        bytes.push(0x91);
    } else if text == "and b" {
        bytes.push(0xA0);
    } else if text == "and c" {
        bytes.push(0xA1);
    } else if text == "or b" {
        bytes.push(0xB0);
    } else if text == "or c" {
        bytes.push(0xB1);
    } else if text == "xor b" {
        bytes.push(0xA8);
    } else if text == "xor c" {
        bytes.push(0xA9);
    } else if text == "xor a" {
        bytes.push(0xAF);
    } else if let Some(value) = text.strip_prefix("xor ") {
        bytes.push(0xEE);
        bytes.push(parse_u8(value.trim())?);
    } else if text == "cp b" {
        bytes.push(0xB8);
    } else if text == "cp c" {
        bytes.push(0xB9);
    } else if let Some(value) = text.strip_prefix("ld a,") {
        bytes.push(0x3E);
        bytes.push(parse_u8(value.trim())?);
    } else {
        return Err(Diagnostic::new(format!(
            "test assembler does not support instruction `{text}`"
        )));
    }
    Ok(())
}

fn parse_addr(text: &str, labels: &HashMap<String, u32>) -> Result<u32, Diagnostic> {
    labels
        .get(text)
        .copied()
        .map(Ok)
        .unwrap_or_else(|| parse_number(text).map(|value| value & 0xFF_FFFF))
}

fn parse_u8(text: &str) -> Result<u8, Diagnostic> {
    let value = parse_number(text)?;
    if value > 0xFF {
        return Err(Diagnostic::new(format!("value {text} is outside u8 range")));
    }
    Ok(value as u8)
}

fn parse_number(text: &str) -> Result<u32, Diagnostic> {
    let text = text.trim().trim_end_matches(',');
    if let Some(hex) = text.strip_suffix('h') {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else {
        text.parse()
    }
    .map_err(|_| Diagnostic::new(format!("invalid numeric operand `{text}`")))
}

fn push24(bytes: &mut Vec<u8>, value: u32) {
    bytes.push(value as u8);
    bytes.push((value >> 8) as u8);
    bytes.push((value >> 16) as u8);
}

struct TestMachine {
    memory: HashMap<u32, u8>,
    ports: [u8; 256],
    cycles: Cell<i64>,
    halted: bool,
    result_code: u8,
    debug_output: Vec<u8>,
}

impl TestMachine {
    fn new() -> Self {
        Self {
            memory: HashMap::new(),
            ports: [0; 256],
            cycles: Cell::new(0),
            halted: false,
            result_code: 0,
            debug_output: Vec::new(),
        }
    }
}

impl Machine for TestMachine {
    fn peek(&self, address: u32) -> u8 {
        self.memory
            .get(&(address & 0xFF_FFFF))
            .copied()
            .unwrap_or(0)
    }

    fn poke(&mut self, address: u32, value: u8) {
        self.memory.insert(address & 0xFF_FFFF, value);
    }

    fn use_cycles(&self, cycles: i32) {
        self.cycles
            .set(self.cycles.get().wrapping_add(cycles as i64));
    }

    fn port_in(&mut self, address: u16) -> u8 {
        self.ports[address as usize & 0xFF]
    }

    fn port_out(&mut self, address: u16, value: u8) {
        let port = address as usize & 0xFF;
        self.ports[port] = value;
        if port == 0x0C {
            self.debug_output.push(value);
        }
        if port == 0x0D {
            self.result_code = value;
        }
        if port == 0x0E && value == 1 {
            self.halted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{asm::emit_ez80_assembly, parser::parse_program};

    use super::*;

    #[test]
    fn runs_emitted_test_pass_on_ez80_vm() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }

    #[test]
    fn compare_carry_jump_sequence_matches_emitter_assumption() {
        let asm = r#"
            ld sp, 0F00000h
            ld a, 00h
            ld b, a
            ld a, 04h
            ld c, a
            ld a, b
            cp c
            jp c, yes
            ld a, 09h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        yes:
            ld a, 00h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }
}
