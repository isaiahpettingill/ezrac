//! PIC18 source lowering.
//!
//! The source backend reuses EZRAC's AVR HIR/TBIR lowering because both
//! targets are byte-oriented Harvard microcontrollers. Its AVR register
//! stream is lowered to a classic PIC18 implementation here: AVR registers
//! become compiler-private data bytes, X/Z become FSR0/FSR1, and the compiler
//! stack becomes an FSR2 data stack. This keeps the language lowering complete
//! while keeping the emitted ABI explicitly PIC18.

use crate::{
    asm::{AssemblyOptions, emit_avr_assembly_with_options},
    ast::{Declaration, Program},
    compat::prelude::*,
    diagnostic::Diagnostic,
    target::CpuFamily,
};

const REGISTER_BASE: u32 = 0x20;
const MULTIPLY_SCRATCH: u32 = 0x70;

pub fn emit_pic18_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    if options.cpu != CpuFamily::Pic18 {
        return Err(Diagnostic::new("PIC18 emitter requires a PIC18 target"));
    }
    if program.main_function().is_none() {
        return Err(Diagnostic::new("PIC18 programs require a `main` function"));
    }

    let mut avr_options = options.clone();
    avr_options.cpu = CpuFamily::Avr;
    let avr = emit_avr_assembly_with_options(program, avr_options)?;
    translate_program(&avr, program, options.stack_top.get())
}

fn translate_program(
    program: &str,
    source: &Program,
    stack_top: u32,
) -> Result<String, Diagnostic> {
    let (high_interrupt, low_interrupt) = interrupt_vectors(source);
    let mut output = String::new();
    let mut state = TranslationState::default();
    let mut inserted_vectors = false;
    let mut inserted_unhandled = false;

    for line in program.lines() {
        let trimmed = line.trim();
        if trimmed == "section .text" && !inserted_vectors {
            output.push_str("section .text\n");
            output.push_str("org 0000h\n    goto __ezra_start\n");
            output.push_str("org 0008h\n    goto ");
            output.push_str(
                high_interrupt
                    .as_deref()
                    .unwrap_or("__ezra_unhandled_interrupt"),
            );
            output.push('\n');
            output.push_str("org 0018h\n    goto ");
            output.push_str(
                low_interrupt
                    .as_deref()
                    .unwrap_or("__ezra_unhandled_interrupt"),
            );
            output.push('\n');
            output.push_str("org 0020h\n");
            output.push_str(&format!(
                "    lfsr 2, {:04X}h\n",
                stack_top.saturating_sub(0xFF)
            ));
            inserted_vectors = true;
            continue;
        }
        if trimmed.starts_with("org ") {
            continue;
        }
        if trimmed.starts_with("section ") && trimmed != "section .text" && !inserted_unhandled {
            output.push_str("__ezra_unhandled_interrupt:\n    sleep\n");
            inserted_unhandled = true;
        }
        if trimmed == "; target: AVR register ABI" {
            output.push_str("; target: PIC18 classic data-byte ABI\n");
            output.push_str(
                "; data pointers use a 16-bit FSR value; code pointers use PIC18 byte addresses\n",
            );
            continue;
        }
        for translated in translate_line(line, &mut state)? {
            output.push_str(&translated);
            output.push('\n');
        }
    }
    if !inserted_unhandled {
        output.push_str("__ezra_unhandled_interrupt:\n    sleep\n");
    }
    Ok(output)
}

fn interrupt_vectors(program: &Program) -> (Option<String>, Option<String>) {
    let mut handlers = program
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Function(function)
                if function
                    .attrs
                    .iter()
                    .any(|attribute| attribute == "interrupt") =>
            {
                Some(function.name.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    handlers.sort();
    let high = handlers
        .iter()
        .find(|name| name.contains("high"))
        .or_else(|| handlers.first())
        .map(|name| format!("_{name}"));
    let low = handlers
        .iter()
        .find(|name| name.contains("low"))
        .or_else(|| handlers.get(1))
        .map(|name| format!("_{name}"));
    (high, low)
}

#[derive(Default)]
struct TranslationState {
    labels: usize,
}

impl TranslationState {
    fn next_label(&mut self, stem: &str) -> String {
        let label = format!(".L_pic18_{stem}_{}", self.labels);
        self.labels += 1;
        label
    }
}

fn translate_line(line: &str, state: &mut TranslationState) -> Result<Vec<String>, Diagnostic> {
    let indent = if line.starts_with("    ") { "    " } else { "" };
    let text = line.trim();
    if text.is_empty()
        || text.starts_with(';')
        || text.starts_with("section ")
        || text.starts_with("org ")
        || text.ends_with(':')
    {
        return Ok(vec![line.to_owned()]);
    }
    let (op, operands) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let op = op.to_ascii_lowercase();
    let operands = operands.trim();
    let args = split_operands(operands);

    if is_pic18_instruction(&op) {
        return Ok(vec![line.to_owned()]);
    }

    let mut out = Vec::new();
    match op.as_str() {
        "nop" => out.push(format!("{indent}nop")),
        "ldi" => {
            let [register, value] = two_args(&args, "ldi")?;
            out.push(format!("{indent}movlw {}", immediate_text(value)?));
            out.push(store_register(register)?);
        }
        "clr" => {
            let [register] = one_arg(&args, "clr")?;
            out.push(format!("{indent}clrf {}, a", register_file(register)?));
        }
        "tst" => {
            let [register] = one_arg(&args, "tst")?;
            out.push(format!("{indent}movf {}, w, a", register_file(register)?));
        }
        "lds" => {
            let [register, address] = two_args(&args, "lds")?;
            out.extend(load_memory(register, parse_address(address)?, indent)?);
        }
        "sts" => {
            let [address, register] = two_args(&args, "sts")?;
            out.extend(store_memory(parse_address(address)?, register, indent)?);
        }
        "mov" => {
            let [destination, source] = two_args(&args, "mov")?;
            out.push(format!("{indent}movf {}, w, a", register_file(source)?));
            out.push(format!("{indent}movwf {}, a", register_file(destination)?));
        }
        "add" | "adc" | "and" | "or" | "eor" | "sub" | "sbc" => {
            let [destination, source] = two_args(&args, &op)?;
            out.extend(binary_register_operation(&op, destination, source, indent)?);
        }
        "cp" | "cpc" => {
            let [left, right] = two_args(&args, &op)?;
            out.extend(compare_registers(&op, left, right, indent)?);
        }
        "subi" | "sbci" => {
            let [register, value] = two_args(&args, &op)?;
            let register = register_file(register)?;
            if op == "sbci" {
                out.push(format!("{indent}btg STATUS, 0, a"));
            }
            out.push(format!("{indent}movlw {}", immediate_text(value)?));
            out.push(format!("{indent}subwf {register}, f, a"));
            out.push(format!("{indent}btg STATUS, 0, a"));
        }
        "inc" | "dec" => {
            let [operand] = one_arg(&args, &op)?;
            if let Some(register) = parse_register(operand) {
                out.push(format!(
                    "{indent}{} {}, f, a",
                    if op == "inc" { "incf" } else { "decf" },
                    register_file_number(register)
                ));
            } else {
                let address = parse_address(operand)?;
                out.push(format!("{indent}movlb {:X}", address >> 8));
                out.push(format!(
                    "{indent}{} {:02X}h, f, b",
                    if op == "inc" { "incf" } else { "decf" },
                    address & 0xFF
                ));
            }
        }
        "lsl" | "rol" | "lsr" | "ror" => {
            let [register] = one_arg(&args, &op)?;
            let register = register_file(register)?;
            if matches!(op.as_str(), "lsl" | "lsr") {
                out.push(format!("{indent}bcf STATUS, 0, a"));
            }
            let mnemonic = match op.as_str() {
                "lsl" | "rol" => "rlcf",
                "lsr" | "ror" => "rrcf",
                _ => unreachable!(),
            };
            out.push(format!("{indent}{mnemonic} {register}, f, a"));
        }
        "asr" => {
            let [register] = one_arg(&args, "asr")?;
            let register = register_file(register)?;
            out.push(format!("{indent}btfsc {register}, 7, a"));
            out.push(format!("{indent}bsf STATUS, 0, a"));
            out.push(format!("{indent}btfss {register}, 7, a"));
            out.push(format!("{indent}bcf STATUS, 0, a"));
            out.push(format!("{indent}rrcf {register}, f, a"));
        }
        "neg" => {
            let [register] = one_arg(&args, "neg")?;
            out.push(format!("{indent}negf {}, a", register_file(register)?));
        }
        "swap" => {
            let [register] = one_arg(&args, "swap")?;
            out.push(format!("{indent}swapf {}, f, a", register_file(register)?));
        }
        "com" => {
            let [register] = one_arg(&args, "com")?;
            out.push(format!("{indent}comf {}, f, a", register_file(register)?));
        }
        "mul" | "muls" | "mulsu" => {
            let [left, right] = two_args(&args, &op)?;
            out.extend(software_multiply(left, right, indent, state));
        }
        "adiw" | "sbiw" => {
            let [pair, value] = two_args(&args, &op)?;
            let pair = pair_pair(pair)?;
            let value = immediate_text(value)?;
            if op == "adiw" {
                out.push(format!("{indent}movlw {value}"));
                out.push(format!("{indent}addwf {}, f, a", pair.0));
                out.push(format!("{indent}movlw 0"));
                out.push(format!("{indent}addwfc {}, f, a", pair.1));
            } else {
                out.push(format!("{indent}movlw {value}"));
                out.push(format!("{indent}subwf {}, f, a", pair.0));
                out.push(format!("{indent}btg STATUS, 0, a"));
                out.push(format!("{indent}movlw 0"));
                out.push(format!("{indent}subwfb {}, f, a", pair.1));
                out.push(format!("{indent}btg STATUS, 0, a"));
            }
        }
        "ld" | "st" => out.extend(indirect_operation(&op, &args, indent)?),
        "push" => {
            let [register] = one_arg(&args, "push")?;
            out.push(format!("{indent}movf {}, w, a", register_file(register)?));
            out.push(format!("{indent}movwf POSTINC2, a"));
        }
        "pop" => {
            let [register] = one_arg(&args, "pop")?;
            out.push(format!("{indent}movf POSTDEC2, w, a"));
            out.push(format!("{indent}movwf {}, a", register_file(register)?));
            out.push(format!("{indent}movf {}, w, a", register_file(register)?));
        }
        "call" | "jsr" => out.push(format!("{indent}call {}", args.join(", "))),
        "jmp" => out.push(format!("{indent}goto {operands}")),
        "rjmp" => out.push(format!("{indent}bra {operands}")),
        "ret" => out.push(format!("{indent}return")),
        "reti" => out.push(format!("{indent}retfie")),
        "brlo" | "brcs" => out.push(format!("{indent}bc {operands}")),
        "brsh" | "brcc" => out.push(format!("{indent}bnc {operands}")),
        "breq" => out.push(format!("{indent}bz {operands}")),
        "brne" => out.push(format!("{indent}bnz {operands}")),
        "brmi" => out.push(format!("{indent}bn {operands}")),
        "brpl" => out.push(format!("{indent}bnn {operands}")),
        "brvs" => out.push(format!("{indent}bov {operands}")),
        "brvc" => out.push(format!("{indent}bnov {operands}")),
        "clc" => out.push(format!("{indent}bcf STATUS, 0, a")),
        "sec" => out.push(format!("{indent}bsf STATUS, 0, a")),
        "cli" | "sei" => out.push(format!("{indent}nop")),
        "in" => {
            let [register, _port] = two_args(&args, "in")?;
            out.push(format!("{indent}movf STATUS, w, a"));
            out.push(format!("{indent}movwf {}, a", register_file(register)?));
        }
        "out" => out.push(format!("{indent}nop")),
        "icall" => {
            out.push(format!("{indent}movf {}, w, a", register_file_number(30)));
            out.push(format!("{indent}movwf PCLATH, a"));
            out.push(format!("{indent}movf {}, w, a", register_file_number(31)));
            out.push(format!("{indent}movwf PCLATU, a"));
            out.push(format!("{indent}movf {}, w, a", register_file_number(30)));
            out.push(format!("{indent}callw"));
        }
        _ => {
            return Err(Diagnostic::new(format!(
                "PIC18 source lowering cannot translate AVR instruction `{line}`"
            )));
        }
    }
    Ok(out)
}

fn binary_register_operation(
    op: &str,
    destination: &str,
    source: &str,
    indent: &str,
) -> Result<Vec<String>, Diagnostic> {
    let destination = register_file(destination)?;
    let source = register_file(source)?;
    let mut out = vec![format!("{indent}movf {source}, w, a")];
    let mnemonic = match op {
        "add" => "addwf",
        "adc" => "addwfc",
        "and" => "andwf",
        "or" => "iorwf",
        "eor" => "xorwf",
        "sub" => "subwf",
        "sbc" => "subwfb",
        _ => {
            return Err(Diagnostic::new(format!(
                "unsupported PIC18 register operation `{op}`"
            )));
        }
    };
    if matches!(op, "sub" | "sbc") {
        if op == "sbc" {
            out.push(format!("{indent}btg STATUS, 0, a"));
        }
        out.push(format!("{indent}{mnemonic} {destination}, f, a"));
        out.push(format!("{indent}btg STATUS, 0, a"));
    } else {
        out.push(format!("{indent}{mnemonic} {destination}, f, a"));
    }
    Ok(out)
}

fn compare_registers(
    op: &str,
    left: &str,
    right: &str,
    indent: &str,
) -> Result<Vec<String>, Diagnostic> {
    let left = register_file(left)?;
    let right = register_file(right)?;
    let mut out = vec![format!("{indent}movf {right}, w, a")];
    if op == "cpc" {
        out.push(format!("{indent}btg STATUS, 0, a"));
    }
    out.push(format!("{indent}subwf {left}, w, a"));
    out.push(format!("{indent}btg STATUS, 0, a"));
    Ok(out)
}

fn indirect_operation(op: &str, args: &[&str], indent: &str) -> Result<Vec<String>, Diagnostic> {
    if args.len() != 2 {
        return Err(Diagnostic::new(format!(
            "PIC18 translated {op} requires two operands"
        )));
    }
    let (register, pointer) = if op == "ld" {
        (args[0], args[1])
    } else {
        (args[1], args[0])
    };
    let register = register_file(register)?;
    let pointer = pointer.to_ascii_lowercase();
    let (fsr, indirect, post) = match pointer.as_str() {
        "x" => (0, "ef", false),
        "x+" => (0, "ee", true),
        "z" => (1, "e7", false),
        "z+" => (1, "e6", true),
        "-x" => (0, "ec", true),
        "-z" => (1, "e4", true),
        _ => {
            return Err(Diagnostic::new(format!(
                "unsupported PIC18 indirect pointer `{pointer}`"
            )));
        }
    };
    let low = if fsr == 0 { "e9" } else { "e1" };
    let high = if fsr == 0 { "ea" } else { "e2" };
    let mut out = vec![
        format!(
            "{indent}movf {}, w, a",
            register_file_number(if fsr == 0 { 26 } else { 30 })
        ),
        format!("{indent}movwf {low}h, a"),
        format!(
            "{indent}movf {}, w, a",
            register_file_number(if fsr == 0 { 27 } else { 31 })
        ),
        format!("{indent}movwf {high}h, a"),
    ];
    if op == "ld" {
        out.push(format!("{indent}movf {indirect}h, w, a"));
        out.push(format!("{indent}movwf {register}, a"));
    } else {
        out.push(format!("{indent}movf {register}, w, a"));
        out.push(format!("{indent}movwf {indirect}h, a"));
    }
    if post {
        out.extend([
            format!("{indent}movf {low}h, w, a"),
            format!(
                "{indent}movwf {}, a",
                register_file_number(if fsr == 0 { 26 } else { 30 })
            ),
            format!("{indent}movf {high}h, w, a"),
            format!(
                "{indent}movwf {}, a",
                register_file_number(if fsr == 0 { 27 } else { 31 })
            ),
        ]);
    }
    Ok(out)
}

fn software_multiply(
    left: &str,
    right: &str,
    indent: &str,
    state: &mut TranslationState,
) -> Vec<String> {
    let loop_label = state.next_label("mul_loop");
    let add_label = state.next_label("mul_add");
    let shift_label = state.next_label("mul_shift");
    let done_label = state.next_label("mul_done");
    vec![
        format!(
            "{indent}movf {}, w, a",
            register_file(left).unwrap_or_else(|_| "20h".to_owned())
        ),
        format!("{indent}movwf {:02X}h, a", MULTIPLY_SCRATCH),
        format!(
            "{indent}movf {}, w, a",
            register_file(right).unwrap_or_else(|_| "21h".to_owned())
        ),
        format!("{indent}movwf {:02X}h, a", MULTIPLY_SCRATCH + 1),
        format!("{indent}clrf {:02X}h, a", MULTIPLY_SCRATCH + 2),
        format!("{indent}clrf {:02X}h, a", MULTIPLY_SCRATCH + 3),
        format!("{indent}movlw 8"),
        format!("{indent}movwf {:02X}h, a", MULTIPLY_SCRATCH + 4),
        format!("{loop_label}:"),
        format!("{indent}btfss {:02X}h, 0, a", MULTIPLY_SCRATCH + 1),
        format!("{indent}bra {shift_label}"),
        format!("{add_label}:"),
        format!("{indent}movf {:02X}h, w, a", MULTIPLY_SCRATCH),
        format!("{indent}addwf {:02X}h, f, a", MULTIPLY_SCRATCH + 2),
        format!("{indent}movlw 0"),
        format!("{indent}addwfc {:02X}h, f, a", MULTIPLY_SCRATCH + 3),
        format!("{shift_label}:"),
        format!("{indent}bcf STATUS, 0, a"),
        format!("{indent}rlcf {:02X}h, f, a", MULTIPLY_SCRATCH),
        format!("{indent}bcf STATUS, 0, a"),
        format!("{indent}rrcf {:02X}h, f, a", MULTIPLY_SCRATCH + 1),
        format!("{indent}decfsz {:02X}h, f, a", MULTIPLY_SCRATCH + 4),
        format!("{indent}bra {loop_label}"),
        format!("{done_label}:"),
        format!("{indent}movf {:02X}h, w, a", MULTIPLY_SCRATCH + 2),
        format!("{indent}movwf 20h, a"),
        format!("{indent}movf {:02X}h, w, a", MULTIPLY_SCRATCH + 3),
        format!("{indent}movwf 21h, a"),
    ]
}

fn load_memory(register: &str, address: u32, indent: &str) -> Result<Vec<String>, Diagnostic> {
    let register = register_file(register)?;
    let (bank, low) = data_address(address)?;
    Ok(vec![
        format!("{indent}movlb {bank:X}"),
        format!("{indent}movf {low:02X}h, w, b"),
        format!("{indent}movwf {register}, a"),
    ])
}

fn store_memory(address: u32, register: &str, indent: &str) -> Result<Vec<String>, Diagnostic> {
    let register = register_file(register)?;
    let (bank, low) = data_address(address)?;
    Ok(vec![
        format!("{indent}movf {register}, w, a"),
        format!("{indent}movlb {bank:X}"),
        format!("{indent}movwf {low:02X}h, b"),
    ])
}

fn data_address(address: u32) -> Result<(u32, u32), Diagnostic> {
    if address > 0x0FFF {
        return Err(Diagnostic::new(format!(
            "PIC18 data address 0x{address:X} is outside the classic 4 KiB data bus"
        )));
    }
    Ok((address >> 8, address & 0xFF))
}

fn store_register(register: &str) -> Result<String, Diagnostic> {
    Ok(format!("    movwf {}, a", register_file(register)?))
}

fn register_file(register: &str) -> Result<String, Diagnostic> {
    let number = parse_register(register).ok_or_else(|| {
        Diagnostic::new(format!(
            "PIC18 source lowering expected an AVR register, got `{register}`"
        ))
    })?;
    Ok(register_file_number(number))
}

fn register_file_number(register: u8) -> String {
    format!("{:02X}h", REGISTER_BASE + u32::from(register))
}

fn parse_register(text: &str) -> Option<u8> {
    let text = text.trim();
    text.strip_prefix('r')?
        .parse::<u8>()
        .ok()
        .filter(|value| *value < 32)
}

fn pair_pair(text: &str) -> Result<(String, String), Diagnostic> {
    let register =
        parse_register(text).ok_or_else(|| Diagnostic::new("PIC18 register pair is invalid"))?;
    if !matches!(register, 24 | 26 | 28 | 30) {
        return Err(Diagnostic::new(
            "PIC18 translated word pair must start at r24, r26, r28, or r30",
        ));
    }
    Ok((
        register_file_number(register),
        register_file_number(register + 1),
    ))
}

fn parse_address(text: &str) -> Result<u32, Diagnostic> {
    let text = text.trim().trim_start_matches('&').trim_start_matches('$');
    let text = text
        .strip_suffix('h')
        .or_else(|| text.strip_suffix('H'))
        .unwrap_or(text);
    if let Some(value) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u32::from_str_radix(value, 16)
            .map_err(|_| Diagnostic::new(format!("invalid AVR address `{text}`")))
    } else {
        u32::from_str_radix(text, 16)
            .map_err(|_| Diagnostic::new(format!("invalid AVR address `{text}`")))
    }
}

fn immediate_text(text: &str) -> Result<String, Diagnostic> {
    let text = text.trim().trim_start_matches('#');
    if text.starts_with('$') {
        Ok(format!("{}h", text.trim_start_matches('$')))
    } else {
        Ok(text.to_owned())
    }
}

fn split_operands(text: &str) -> Vec<&str> {
    text.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

fn one_arg<'a>(args: &'a [&'a str], mnemonic: &str) -> Result<[&'a str; 1], Diagnostic> {
    if args.len() == 1 {
        Ok([args[0]])
    } else {
        Err(Diagnostic::new(format!(
            "PIC18 translated {mnemonic} expects one operand"
        )))
    }
}

fn two_args<'a>(args: &'a [&'a str], mnemonic: &str) -> Result<[&'a str; 2], Diagnostic> {
    if args.len() == 2 {
        Ok([args[0], args[1]])
    } else {
        Err(Diagnostic::new(format!(
            "PIC18 translated {mnemonic} expects two operands"
        )))
    }
}

fn is_pic18_instruction(op: &str) -> bool {
    matches!(op, |"nop"| "sleep"
        | "clrwdt"
        | "daw"
        | "callw"
        | "return"
        | "retfie"
        | "reset"
        | "movlb"
        | "tblrd*"
        | "tblrd*+"
        | "tblrd*-"
        | "tblrd+*"
        | "movff"
        | "lfsr"
        | "call"
        | "goto"
        | "bra"
        | "rcall"
        | "bz"
        | "bnz"
        | "bc"
        | "bnc"
        | "bov"
        | "bnov"
        | "bn"
        | "bnn"
        | "addwf"
        | "addwfc"
        | "andwf"
        | "comf"
        | "decf"
        | "decfsz"
        | "dcfsnz"
        | "incf"
        | "incfsz"
        | "infsnz"
        | "iorwf"
        | "movf"
        | "movwf"
        | "mulwf"
        | "negf"
        | "rlcf"
        | "rlncf"
        | "rrcf"
        | "rrncf"
        | "setf"
        | "subfwb"
        | "subwf"
        | "subwfb"
        | "swapf"
        | "tstfsz"
        | "xorwf"
        | "clrf"
        | "cpfseq"
        | "cpfslt"
        | "cpfsgt"
        | "bcf"
        | "bsf"
        | "btfsc"
        | "btfss"
        | "btg"
        | "addlw"
        | "andlw"
        | "iorlw"
        | "movlw"
        | "mullw"
        | "retlw"
        | "sublw"
        | "xorlw")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{asm::AssemblyOptions, parser::parse_program, target::Address24};
    use std::path::Path;

    fn emit(source: &str) -> String {
        let program = parse_program(Path::new("pic18.ezra"), source).unwrap();
        emit_pic18_assembly_with_options(
            &program,
            AssemblyOptions {
                cpu: CpuFamily::Pic18,
                load_addr: Address24::new(0),
                entry_addr: Address24::new(0),
                code_base: Address24::new(0),
                ram_base: Address24::new(0x0104),
                rodata_base: Address24::new(0x0200),
                asset_base: Address24::new(0x0800),
                stack_top: Address24::new(0x0DFF),
                default_sdk_symbols: false,
                ..AssemblyOptions::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn lowers_scalar_source_through_hir_and_tbir() {
        let assembly = emit(
            "global result: u16 = 0 fn add(a: u16, b: u16) -> u16 { return a + b } fn main() { result = add(20, 22) }",
        );
        assert!(assembly.contains("target: PIC18 classic"), "{assembly}");
        assert!(assembly.contains("movlw"), "{assembly}");
        assert!(assembly.contains("call _add"), "{assembly}");
    }

    #[test]
    fn lowers_typed_function_pointer_calls_to_callw() {
        let assembly = emit(
            "global callback: ptr<fn(u8, u8)u8> = &add global result: u8 = 0 fn add(a: u8, b: u8) -> u8 { return a + b } fn main() { let local: ptr<fn(u8, u8)u8> = &add; result = callback(20, 22); result = local(20, 22) }",
        );

        assert_eq!(assembly.matches("callw").count(), 2, "{assembly}");
        assert!(assembly.contains("movwf PCLATH, a"), "{assembly}");
        assert!(assembly.contains("movwf PCLATU, a"), "{assembly}");
        assert!(assembly.contains("goto _add"), "{assembly}");
        assert!(assembly.contains("_add:"), "{assembly}");
    }
}
