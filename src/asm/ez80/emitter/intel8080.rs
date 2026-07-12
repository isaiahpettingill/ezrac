use crate::{diagnostic::Diagnostic, target::CpuFamily};

pub(super) fn is_intel_8080_family(cpu: CpuFamily) -> bool {
    matches!(cpu, CpuFamily::I8080 | CpuFamily::I8085)
}

pub(super) fn translate_assembly_for_cpu(
    cpu: CpuFamily,
    assembly: &str,
) -> Result<String, Diagnostic> {
    if !is_intel_8080_family(cpu) {
        return Ok(assembly.to_owned());
    }
    let mut out = String::new();
    for line in assembly.lines() {
        out.push_str(&translate_8080_line(line)?);
        out.push('\n');
    }
    Ok(out)
}

fn translate_8080_line(line: &str) -> Result<String, Diagnostic> {
    let Some(column) = line.find(|ch: char| !ch.is_whitespace()) else {
        return Ok(line.to_owned());
    };
    let indent = &line[..column];
    let body = &line[column..];
    let (code, comment) = body
        .split_once(';')
        .map_or((body.trim_end(), ""), |(code, comment)| {
            (code.trim_end(), comment)
        });
    let trimmed = code.trim();
    if trimmed.is_empty()
        || trimmed.ends_with(':')
        || trimmed.starts_with("section ")
        || trimmed.starts_with("db ")
        || trimmed.starts_with("dw ")
    {
        return Ok(line.to_owned());
    }
    let translated = translate_8080_instruction(trimmed)?.unwrap_or_else(|| trimmed.to_owned());
    let mut out = indent_8080_translation(indent, &translated);
    if !comment.is_empty() {
        out.push_str(" ;");
        out.push_str(comment);
    }
    Ok(out)
}

fn indent_8080_translation(indent: &str, translated: &str) -> String {
    let mut out = String::new();
    for (index, line) in translated.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(indent);
        out.push_str(line);
    }
    out
}

fn translate_8080_instruction(text: &str) -> Result<Option<String>, Diagnostic> {
    if let Some(instruction) = translate_8080_exact(text)? {
        return Ok(Some(instruction));
    }
    if let Some((dst, src)) = parse_asm_operands(text.strip_prefix("ld ")) {
        return translate_8080_ld(dst, src).map(Some);
    }
    for (prefix, reg_mnemonic, rp_mnemonic) in [("inc ", "inr", "inx"), ("dec ", "dcr", "dcx")] {
        if let Some(register) = text.strip_prefix(prefix) {
            if matches!(register.trim(), "bc" | "de" | "hl" | "sp") {
                return Ok(Some(format!(
                    "{rp_mnemonic} {}",
                    intel_8080_rp(register.trim())?
                )));
            }
            return Ok(Some(format!(
                "{reg_mnemonic} {}",
                intel_8080_operand(register)?
            )));
        }
    }
    for (prefix, mnemonic) in [("add hl,", "dad"), ("add hl, ", "dad")] {
        if let Some(register) = text.strip_prefix(prefix) {
            return Ok(Some(format!(
                "{mnemonic} {}",
                intel_8080_rp(register.trim())?
            )));
        }
    }
    for (prefix, mnemonic) in [
        ("add a,", "add"),
        ("add a, ", "add"),
        ("adc a,", "adc"),
        ("adc a, ", "adc"),
        ("sub ", "sub"),
        ("sbc a,", "sbb"),
        ("sbc a, ", "sbb"),
        ("and ", "ana"),
        ("xor ", "xra"),
        ("or ", "ora"),
        ("cp ", "cmp"),
    ] {
        if let Some(operand) = text.strip_prefix(prefix) {
            let operand = operand.trim();
            let immediate = match mnemonic {
                "add" => "adi",
                "adc" => "aci",
                "sub" => "sui",
                "sbb" => "sbi",
                "ana" => "ani",
                "xra" => "xri",
                "ora" => "ori",
                "cmp" => "cpi",
                _ => unreachable!(),
            };
            if is_numeric_asm_operand(operand) {
                return Ok(Some(format!("{immediate} {operand}")));
            }
            return Ok(Some(format!("{mnemonic} {}", intel_8080_operand(operand)?)));
        }
    }
    if let Some(rest) = text.strip_prefix("push ") {
        return Ok(Some(format!("push {}", intel_8080_stack_rp(rest.trim())?)));
    }
    if let Some(rest) = text.strip_prefix("pop ") {
        return Ok(Some(format!("pop {}", intel_8080_stack_rp(rest.trim())?)));
    }
    if let Some(rest) = text.strip_prefix("out ") {
        let Some((port, register)) = rest.split_once(',') else {
            return Err(Diagnostic::new(format!(
                "8080 source codegen cannot translate instruction `{text}`"
            )));
        };
        if register.trim() != "a" {
            return Err(Diagnostic::new(format!(
                "8080 source codegen cannot translate instruction `{text}`"
            )));
        }
        let port = parse_8080_wrapped_port(port.trim())?;
        return Ok(Some(format!("out {port}")));
    }
    if let Some(rest) = text.strip_prefix("in a, ") {
        let port = parse_8080_wrapped_port(rest)?;
        return Ok(Some(format!("in {port}")));
    }
    for (prefix, mnemonic) in [
        ("jp nz,", "jnz"),
        ("jp z,", "jz"),
        ("jp nc,", "jnc"),
        ("jp c,", "jc"),
        ("jp po,", "jpo"),
        ("jp pe,", "jpe"),
        ("jp p,", "jp"),
        ("jp m,", "jm"),
        ("jp ", "jmp"),
        ("call nz,", "cnz"),
        ("call z,", "cz"),
        ("call nc,", "cnc"),
        ("call c,", "cc"),
        ("call po,", "cpo"),
        ("call pe,", "cpe"),
        ("call p,", "cp"),
        ("call m,", "cm"),
        ("call ", "call"),
    ] {
        if let Some(target) = text.strip_prefix(prefix) {
            return Ok(Some(format!("{mnemonic} {}", target.trim())));
        }
    }
    if let Some(target) = text.strip_prefix("rst ") {
        return Ok(Some(format!(
            "rst {}",
            parse_8080_rst_index(target.trim())?
        )));
    }
    reject_z80_only_8080_instruction(text)?;
    Ok(None)
}

fn translate_8080_exact(text: &str) -> Result<Option<String>, Diagnostic> {
    Ok(Some(match text {
        "nop" | "di" | "ei" | "ret" | "daa" => text.to_owned(),
        "halt" => "hlt".to_owned(),
        "rlca" => "rlc".to_owned(),
        "rrca" => "rrc".to_owned(),
        "rla" => "ral".to_owned(),
        "rra" => "rar".to_owned(),
        "cpl" => "cma".to_owned(),
        "scf" => "stc".to_owned(),
        "ccf" => "cmc".to_owned(),
        "ex de, hl" | "ex de,hl" => "xchg".to_owned(),
        "ex (sp), hl" | "ex (sp),hl" => "xthl".to_owned(),
        "ld sp, hl" | "ld sp,hl" => "sphl".to_owned(),
        "jp (hl)" => "pchl".to_owned(),
        "ret nz" => "rnz".to_owned(),
        "ret z" => "rz".to_owned(),
        "ret nc" => "rnc".to_owned(),
        "ret c" => "rc".to_owned(),
        "ret po" => "rpo".to_owned(),
        "ret pe" => "rpe".to_owned(),
        "ret p" => "rp".to_owned(),
        "ret m" => "rm".to_owned(),
        _ => return Ok(None),
    }))
}

fn translate_8080_ld(dst: &str, src: &str) -> Result<String, Diagnostic> {
    let dst = dst.trim();
    let src = src.trim();
    if let Some(move_pair) = translate_8080_register_pair_move(dst, src) {
        return Ok(move_pair);
    }
    if let Some(register) = intel_8080_reg(dst) {
        if let Some(src) = intel_8080_reg_or_m(src) {
            return Ok(format!("mov {register}, {src}"));
        }
        if is_numeric_asm_operand(src) {
            return Ok(format!("mvi {register}, {src}"));
        }
        if dst == "a" {
            if src == "(bc)" {
                return Ok("ldax b".to_owned());
            }
            if src == "(de)" {
                return Ok("ldax d".to_owned());
            }
            if let Some(addr) = unwrap_asm_indirect(src) {
                return Ok(format!("lda {addr}"));
            }
        }
    }
    if dst == "(hl)" {
        if let Some(src) = intel_8080_reg(src) {
            return Ok(format!("mov m, {src}"));
        }
        if is_numeric_asm_operand(src) {
            return Ok(format!("mvi m, {src}"));
        }
    }
    if let Some(dst) = unwrap_asm_indirect(dst) {
        if src == "a" {
            return Ok(format!("sta {dst}"));
        }
        if src == "hl" {
            return Ok(format!("shld {dst}"));
        }
    }
    if let Some(src) = unwrap_asm_indirect(src)
        && dst == "hl"
    {
        return Ok(format!("lhld {src}"));
    }
    if dst == "(bc)" && src == "a" {
        return Ok("stax b".to_owned());
    }
    if dst == "(de)" && src == "a" {
        return Ok("stax d".to_owned());
    }
    if is_numeric_asm_operand(src) {
        return Ok(format!("lxi {}, {src}", intel_8080_rp(dst)?));
    }
    Err(Diagnostic::new(format!(
        "8080 source codegen cannot translate instruction `ld {dst}, {src}`"
    )))
}

fn translate_8080_register_pair_move(dst: &str, src: &str) -> Option<String> {
    let (dst_hi, dst_lo) = intel_8080_rp_bytes(dst)?;
    let (src_hi, src_lo) = intel_8080_rp_bytes(src)?;
    if dst == src {
        return Some("nop".to_owned());
    }
    Some(format!("mov {dst_hi}, {src_hi}\nmov {dst_lo}, {src_lo}"))
}

fn intel_8080_rp_bytes(register: &str) -> Option<(&'static str, &'static str)> {
    match register.trim() {
        "bc" => Some(("b", "c")),
        "de" => Some(("d", "e")),
        "hl" => Some(("h", "l")),
        _ => None,
    }
}

fn reject_z80_only_8080_instruction(text: &str) -> Result<(), Diagnostic> {
    let mnemonic = text.split_whitespace().next().unwrap_or(text);
    if matches!(
        mnemonic,
        "jr" | "djnz"
            | "ldir"
            | "ldi"
            | "lddr"
            | "ldd"
            | "cpir"
            | "cpi"
            | "cpdr"
            | "cpd"
            | "ini"
            | "inir"
            | "ind"
            | "indr"
            | "outi"
            | "otir"
            | "outd"
            | "otdr"
            | "bit"
            | "set"
            | "res"
            | "srl"
            | "sra"
            | "sla"
            | "rl"
            | "rr"
            | "rlc"
            | "rrc"
            | "neg"
            | "im"
            | "reti"
            | "retn"
            | "exx"
            | "mlt"
            | "in0"
            | "out0"
    ) || text.contains("ix")
        || text.contains("iy")
        || text.starts_with("sbc hl,")
        || text.starts_with("adc hl,")
    {
        return Err(Diagnostic::new(format!(
            "8080 source codegen cannot emit Z80-only instruction `{text}`"
        )));
    }
    Ok(())
}

fn parse_asm_operands(rest: Option<&str>) -> Option<(&str, &str)> {
    let (left, right) = rest?.split_once(',')?;
    Some((left.trim(), right.trim()))
}

fn intel_8080_operand(operand: &str) -> Result<&'static str, Diagnostic> {
    intel_8080_reg_or_m(operand).ok_or_else(|| {
        Diagnostic::new(format!(
            "8080 source codegen cannot translate operand `{operand}`"
        ))
    })
}

fn intel_8080_reg_or_m(operand: &str) -> Option<&'static str> {
    if operand.trim() == "(hl)" {
        Some("m")
    } else {
        intel_8080_reg(operand)
    }
}

fn intel_8080_reg(register: &str) -> Option<&'static str> {
    match register.trim() {
        "a" => Some("a"),
        "b" => Some("b"),
        "c" => Some("c"),
        "d" => Some("d"),
        "e" => Some("e"),
        "h" => Some("h"),
        "l" => Some("l"),
        _ => None,
    }
}

fn intel_8080_rp(register: &str) -> Result<&'static str, Diagnostic> {
    match register.trim() {
        "bc" => Ok("b"),
        "de" => Ok("d"),
        "hl" => Ok("h"),
        "sp" => Ok("sp"),
        _ => Err(Diagnostic::new(format!(
            "8080 source codegen cannot translate register pair `{register}`"
        ))),
    }
}

fn intel_8080_stack_rp(register: &str) -> Result<&'static str, Diagnostic> {
    match register.trim() {
        "af" => Ok("psw"),
        other => intel_8080_rp(other),
    }
}

fn unwrap_asm_indirect(operand: &str) -> Option<&str> {
    operand.trim().strip_prefix('(')?.strip_suffix(')')
}

fn parse_8080_wrapped_port(operand: &str) -> Result<&str, Diagnostic> {
    unwrap_asm_indirect(operand).ok_or_else(|| {
        Diagnostic::new(format!(
            "8080 source codegen cannot translate port operand `{operand}`"
        ))
    })
}

fn parse_8080_rst_index(target: &str) -> Result<u8, Diagnostic> {
    let target = parse_u32_asm_number(target)?;
    if target > 0x38 || target % 8 != 0 {
        return Err(Diagnostic::new(format!(
            "restart target 0x{target:X} is not one of 0x00, 0x08, ..., 0x38"
        )));
    }
    Ok((target / 8) as u8)
}

fn is_numeric_asm_operand(text: &str) -> bool {
    let text = text.trim();
    text.strip_prefix("0x")
        .is_some_and(|hex| !hex.is_empty() && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
        || text
            .strip_suffix('h')
            .is_some_and(|hex| !hex.is_empty() && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
        || text.chars().all(|ch| ch.is_ascii_digit())
}

fn parse_u32_asm_number(text: &str) -> Result<u32, Diagnostic> {
    let text = text.trim();
    if let Some(hex) = text.strip_suffix('h') {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else {
        text.parse()
    }
    .map_err(|_| Diagnostic::new(format!("invalid numeric operand `{text}`")))
}
