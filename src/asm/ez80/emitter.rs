use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    asm::ez80::{analyze_instruction, is_unsupported_z80_family_instruction},
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Declaration, Expr, Function, Place, Program,
        Stmt, Type, UnaryOp,
    },
    diagnostic::Diagnostic,
    hir::HirProgram,
    target::{
        Address24, AssemblerCpu, CpuFamily, EZRA_ASSET_BASE, EZRA_AUDIO_BASE, EZRA_CODE_BASE,
        EZRA_ENTRY_ADDR, EZRA_LOAD_ADDR, EZRA_RAM_BASE, EZRA_RODATA_BASE, EZRA_STACK_TOP,
        EZRA_VRAM_BASE,
    },
    tbir::TbirProgram,
};

mod intel8080;
mod symbols;

use intel8080::{is_intel_8080_family, translate_assembly_for_cpu};
use symbols::{FunctionSig, StructLayout, Symbols, ValueWidth, Variable};

pub fn emit_ez80_assembly(program: &Program) -> Result<String, Diagnostic> {
    emit_ez80_assembly_with_options(program, AssemblyOptions::default())
}

pub fn emit_ez80_assembly_with_debug_comments(
    program: &Program,
    debug_comments: bool,
) -> Result<String, Diagnostic> {
    emit_ez80_assembly_with_options(
        program,
        AssemblyOptions {
            debug_comments,
            ..AssemblyOptions::default()
        },
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssemblyOptions {
    pub cpu: CpuFamily,
    pub debug_comments: bool,
    pub default_sdk_symbols: bool,
    pub mos_executable: bool,
    pub c64_executable: bool,
    pub load_addr: Address24,
    pub entry_addr: Address24,
    pub code_base: Address24,
    pub stack_top: Address24,
    pub ram_base: Address24,
    pub vram_base: Address24,
    pub audio_base: Address24,
    pub asset_base: Address24,
    pub rodata_base: Address24,
    pub section_bases: Vec<(String, Address24)>,
}

impl Default for AssemblyOptions {
    fn default() -> Self {
        Self {
            cpu: CpuFamily::Ez80,
            debug_comments: false,
            default_sdk_symbols: true,
            mos_executable: false,
            c64_executable: false,
            load_addr: EZRA_LOAD_ADDR,
            entry_addr: EZRA_ENTRY_ADDR,
            code_base: EZRA_CODE_BASE,
            stack_top: EZRA_STACK_TOP,
            ram_base: EZRA_RAM_BASE,
            vram_base: EZRA_VRAM_BASE,
            audio_base: EZRA_AUDIO_BASE,
            asset_base: EZRA_ASSET_BASE,
            rodata_base: EZRA_RODATA_BASE,
            section_bases: vec![
                (".rodata".to_owned(), EZRA_RODATA_BASE),
                (".assets".to_owned(), EZRA_ASSET_BASE),
            ],
        }
    }
}

pub fn emit_ez80_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    let result = (|| {
        let checked = CheckedEz80Program::from_program(program, &options)?;
        emit_ez80_assembly_from_checked(program, &checked, options)
    })();
    result.map_err(|error| locate_program_diagnostic(program, error))
}

pub fn collect_ez80_semantic_diagnostics(
    program: &Program,
    options: AssemblyOptions,
) -> Vec<Diagnostic> {
    let symbols = match Symbols::from_program(program, options.clone()) {
        Ok(symbols) => symbols,
        Err(error) => return vec![error],
    };
    let mut diagnostics = Vec::new();
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        collect_stmt_call_diagnostics(
            &function.body,
            &function.body_spans,
            &symbols.functions,
            &mut diagnostics,
        );

        let mut emitter = Emitter::new(
            symbols.clone(),
            options.clone(),
            recursive_call_edges(program, &symbols.functions),
        );
        emitter.disable_dead_code_elimination();
        if let Err(error) = emitter.emit_function(function) {
            let error = locate_program_diagnostic(program, error);
            let error = if error.span.is_none() {
                function
                    .body_spans
                    .first()
                    .map(|span| error.clone().with_span_if_missing(span.span.clone()))
                    .unwrap_or(error)
            } else {
                error
            };
            if !diagnostics.iter().any(|diagnostic| {
                diagnostic.message == error.message && diagnostic.span == error.span
            }) {
                diagnostics.push(error);
            }
        }
    }
    diagnostics
}

fn locate_program_diagnostic(program: &Program, error: Diagnostic) -> Diagnostic {
    if error.location().is_some() {
        return error;
    }
    let quoted = error
        .message
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    let value = error
        .message
        .strip_prefix("value ")
        .and_then(|message| message.split_whitespace().next());
    program
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Function(function) => Some(function),
            _ => None,
        })
        .flat_map(|function| statement_references(&function.body_spans))
        .filter(|reference| {
            quoted.iter().any(|token| reference.text == *token)
                || value.is_some_and(|value| reference.text == value)
        })
        .min_by_key(|reference| {
            (
                reference
                    .span
                    .end
                    .line
                    .saturating_sub(reference.span.start.line),
                reference
                    .span
                    .end
                    .column
                    .saturating_sub(reference.span.start.column),
            )
        })
        .map(|reference| error.clone().with_span_if_missing(reference.span.clone()))
        .unwrap_or(error)
}

fn statement_references(spans: &[crate::ast::StmtSpan]) -> Vec<&crate::ast::SourceReference> {
    let mut references = Vec::new();
    for span in spans {
        references.extend(span.references.iter());
        references.extend(statement_references(&span.children));
    }
    references
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckedEz80Program {
    pub hir: HirProgram,
    pub tbir: TbirProgram,
}

impl CheckedEz80Program {
    pub fn from_program(program: &Program, options: &AssemblyOptions) -> Result<Self, Diagnostic> {
        let hir = HirProgram::from_ast(program)?;
        let tbir = TbirProgram::lower(&hir, program, options)?;
        Ok(Self { hir, tbir })
    }
}

pub fn emit_ez80_assembly_from_checked(
    _program: &Program,
    checked: &CheckedEz80Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    let program = &checked.tbir.lowered_program;
    debug_assert_eq!(checked.hir.source_path, program.source_path);
    let symbols = Symbols::from_program(program, options.clone())?;
    let main = program
        .main_function()
        .ok_or_else(|| Diagnostic::new("missing required `fn main()`"))?;
    validate_main_signature(main)?;
    validate_all_function_calls(program, &symbols.functions)?;
    let recursive_call_edges = recursive_call_edges(program, &symbols.functions);
    validate_all_function_bodies(
        program,
        symbols.clone(),
        options.clone(),
        recursive_call_edges.clone(),
    )?;
    let emitted_functions = reachable_function_names(program, &symbols);
    let cpu = options.cpu;

    let mut emitter = Emitter::new(symbols, options, recursive_call_edges);
    emitter.emit_prelude();
    emitter.emit_embed_initializers();
    emitter.emit_string_literal_initializers();
    emitter.emit_global_initializers(program)?;
    emitter.emit_start_tail();
    emitter.emit_function(main)?;
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        if function.name != "main" && emitted_functions.contains(&function.name) {
            emitter.emit_function(function)?;
        }
    }
    emitter.emit_required_sections();
    translate_assembly_for_cpu(cpu, &peephole_cleanup(&emitter.out))
}

fn is_z80_family_16bit(cpu: CpuFamily) -> bool {
    matches!(
        cpu,
        CpuFamily::Z80 | CpuFamily::Z80N | CpuFamily::Z180 | CpuFamily::I8080 | CpuFamily::I8085
    )
}

fn peephole_cleanup(assembly: &str) -> String {
    let mut out = String::new();
    let mut previous_redundant_load = None;

    for line in assembly.lines() {
        if line.trim() == "ld hl, hl" {
            continue;
        }
        let redundant_load = redundant_load_key(line);
        if redundant_load.is_some() && redundant_load == previous_redundant_load {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        previous_redundant_load = redundant_load;
    }

    out
}

fn redundant_load_key(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with("ld ") || trimmed.contains('(') {
        return None;
    }
    let (target, value) = trimmed.strip_prefix("ld ")?.split_once(',')?;
    let target = target.trim();
    if !matches!(
        target,
        "a" | "b" | "c" | "d" | "e" | "h" | "l" | "hl" | "de" | "bc" | "ix" | "iy" | "sp"
    ) {
        return None;
    }
    if value.trim().is_empty() {
        return None;
    }
    Some(trimmed)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoopLabels {
    continue_label: String,
    break_label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalConstant {
    value: i64,
    ty: Type,
}

struct Emitter {
    symbols: Symbols,
    out: String,
    label_counter: usize,
    scopes: Vec<HashMap<String, Variable>>,
    scope_types: Vec<HashMap<String, Type>>,
    local_constants: Vec<HashMap<String, LocalConstant>>,
    readonly_pointer_aliases: Vec<HashMap<String, u32>>,
    string_literals: HashMap<String, Variable>,
    loop_stack: Vec<LoopLabels>,
    return_type_stack: Vec<Option<Type>>,
    return_value_stack: Vec<bool>,
    function_name_stack: Vec<String>,
    function_frame_stack: Vec<bool>,
    function_interrupt_stack: Vec<bool>,
    function_naked_stack: Vec<bool>,
    function_storage_stack: Vec<Vec<Variable>>,
    assigned_names_stack: Vec<HashSet<String>>,
    recursive_call_edges: HashSet<(String, String)>,
    inline_expansion_stack: Vec<String>,
    debug_comments: bool,
    cpu: CpuFamily,
    mos_executable: bool,
    stack_top: Address24,
    eliminate_dead_code: bool,
}

impl Emitter {
    fn new(
        symbols: Symbols,
        options: AssemblyOptions,
        recursive_call_edges: HashSet<(String, String)>,
    ) -> Self {
        let string_literals = symbols.string_literals.clone();
        Self {
            symbols,
            out: String::new(),
            label_counter: 0,
            scopes: Vec::new(),
            scope_types: Vec::new(),
            local_constants: Vec::new(),
            readonly_pointer_aliases: Vec::new(),
            string_literals,
            loop_stack: Vec::new(),
            return_type_stack: Vec::new(),
            return_value_stack: Vec::new(),
            function_name_stack: Vec::new(),
            function_frame_stack: Vec::new(),
            function_interrupt_stack: Vec::new(),
            function_naked_stack: Vec::new(),
            function_storage_stack: Vec::new(),
            assigned_names_stack: Vec::new(),
            recursive_call_edges,
            inline_expansion_stack: Vec::new(),
            debug_comments: options.debug_comments,
            cpu: options.cpu,
            mos_executable: options.mos_executable,
            stack_top: options.stack_top,
            eliminate_dead_code: true,
        }
    }

    fn disable_dead_code_elimination(&mut self) {
        self.eliminate_dead_code = false;
    }

    fn emit_prelude(&mut self) {
        self.line("; generated by ezrac");
        match self.cpu {
            CpuFamily::Ez80 => self.line("; target: eZ80 ADL mode"),
            CpuFamily::Z80 => self.line("; target: Z80"),
            other => self.line(&format!("; target: {}", other.as_str())),
        }
        self.line("section .text");
        self.line("__ezra_start:");
        if self.mos_executable {
            self.line("    ei");
            return;
        }
        self.line("    di");
        if is_z80_family_16bit(self.cpu) {
            self.line(&format!(
                "    ld sp, {:04X}h",
                self.stack_top.get() & 0xFFFF
            ));
        } else {
            self.line(&format!("    ld sp, {:06X}h", self.stack_top.get()));
        }
    }

    fn alloc_var<S: Into<u32>>(&mut self, size: S) -> Variable {
        let variable = self.symbols.alloc_var(size);
        self.track_function_storage(variable);
        variable
    }

    fn alloc_storage(&mut self, ty: &Type) -> Result<Variable, Diagnostic> {
        let variable = self.symbols.alloc_storage(ty)?;
        self.track_function_storage(variable);
        Ok(variable)
    }

    fn track_function_storage(&mut self, variable: Variable) {
        if let Some(storage) = self.function_storage_stack.last_mut() {
            storage.push(variable);
        }
    }

    fn emit_required_sections(&mut self) {
        for section in [".header", ".rodata", ".data", ".bss", ".assets", ".scratch"] {
            self.line(&format!("section {section}"));
        }
    }

    fn emit_start_tail(&mut self) {
        self.line("    call _main");
        self.line("__ezra_exit:");
        if self.mos_executable {
            self.line("    ld hl, 000000h");
            self.line("    ret");
            self.emit_runtime_helpers();
            self.line("");
            return;
        }
        self.line("    jp __ezra_exit");
        self.emit_runtime_helpers();
        self.line("");
    }

    fn emit_runtime_helpers(&mut self) {
        self.line("__ezra_pass:");
        self.emit_out(0x0D, 0);
        self.emit_out(0x0E, 1);
        self.line("    ret");
        self.line("__ezra_fail:");
        self.emit_out_a(0x0D);
        self.emit_out(0x0E, 1);
        self.line("    ret");
        self.line("__ezra_memcpy:");
        if is_intel_8080_family(self.cpu) {
            self.line("    mov a, b");
            self.line("    ora c");
            self.line("    rz");
            self.line(".L_memcpy_loop:");
            self.line("    mov a, m");
            self.line("    stax d");
            self.line("    inx h");
            self.line("    inx d");
            self.line("    dcx b");
            self.line("    mov a, b");
            self.line("    ora c");
            self.line("    jnz .L_memcpy_loop");
            self.line("    ret");
            self.line("__ezra_memset:");
            self.line("    mov e, a");
            self.line("    mov a, b");
            self.line("    ora c");
            self.line("    rz");
            self.line("    mov a, e");
            self.line(".L_memset_loop:");
            self.line("    mov m, a");
            self.line("    inx h");
            self.line("    dcx b");
            self.line("    mov d, a");
            self.line("    mov a, b");
            self.line("    ora c");
            self.line("    mov a, d");
            self.line("    jnz .L_memset_loop");
            self.line("    ret");
            return;
        }
        self.line("    push de");
        self.line("    push hl");
        self.line("    push bc");
        self.line("    pop hl");
        self.line("    ld de, 000000h");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line("    pop hl");
        self.line("    pop de");
        self.line("    ret z");
        self.line("    ex de, hl");
        self.line("    ldir");
        self.line("    ret");
        self.line("__ezra_memset:");
        self.line("    push hl");
        self.line("    push bc");
        self.line("    pop hl");
        self.line("    ld de, 000000h");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line("    pop hl");
        self.line("    ret z");
        self.line("    ld (hl), a");
        self.line("    dec bc");
        self.line("    push hl");
        self.line("    push bc");
        self.line("    pop hl");
        self.line("    ld de, 000000h");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line("    pop hl");
        self.line("    ret z");
        self.line("    push hl");
        self.line("    inc hl");
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    ldir");
        self.line("    ret");
        self.line("__ezra_mul_u8:");
        if is_z80_family_16bit(self.cpu) {
            self.line("    ld b, a");
            self.line("    xor a");
            self.line(".L_mul_u8_loop:");
            self.line("    ld d, a");
            self.line("    ld a, b");
            self.line("    or b");
            self.line("    ld a, d");
            self.line("    ret z");
            self.line("    add a, c");
            self.line("    dec b");
            self.line("    jp .L_mul_u8_loop");
        } else {
            self.line("    ld b, a");
            self.line("    mlt bc");
            self.line("    ld a, c");
            self.line("    ret");
        }
        self.line("__ezra_mul_u16:");
        if is_z80_family_16bit(self.cpu) {
            self.line("    ld de, 0000h");
            self.line(".L_mul_u16_loop:");
            self.line("    ld a, b");
            self.line("    or c");
            self.line("    jp z, .L_mul_u16_done");
            self.line("    ex de, hl");
            self.line("    add hl, de");
            self.line("    ex de, hl");
            self.line("    dec bc");
            self.line("    jp .L_mul_u16_loop");
            self.line(".L_mul_u16_done:");
            self.line("    ex de, hl");
            self.line("    ret");
        } else {
            self.line("    ld d, h");
            self.line("    ld e, l");
            self.line("    ld h, c");
            self.line("    mlt hl");
            self.line("    push hl");
            self.line("    ld h, d");
            self.line("    ld l, c");
            self.line("    mlt hl");
            self.line("    ld a, l");
            self.line("    ld h, e");
            self.line("    ld l, b");
            self.line("    mlt hl");
            self.line("    add a, l");
            self.line("    pop de");
            self.line("    add a, d");
            self.line("    ld hl, 000000h");
            self.line("    ld h, a");
            self.line("    ld l, e");
            self.line("    ret");
        }
        self.line("__ezra_mul_u24:");
        self.line("    ex de, hl");
        self.line("    ld hl, 000000h");
        self.line(".L_mul_u24_loop:");
        self.line("    push hl");
        self.line("    ld hl, 000000h");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp z, .L_mul_u24_done");
        self.line("    pop hl");
        self.line("    add hl, de");
        self.line("    dec bc");
        self.line("    jp .L_mul_u24_loop");
        self.line(".L_mul_u24_done:");
        self.line("    pop hl");
        self.line("    ret");
        self.line("__ezra_mul_i24:");
        self.line("    jp __ezra_mul_u24");
        self.line("__ezra_div_u8:");
        self.line("    ld d, a");
        self.line("    xor a");
        self.line("    ld b, a");
        self.line("    ld a, c");
        self.line("    or a");
        self.line("    jp z, .L_div_u8_zero");
        self.line(".L_div_u8_loop:");
        self.line("    ld a, d");
        self.line("    cp c");
        self.line("    jp c, .L_div_u8_done");
        self.line("    sub c");
        self.line("    ld d, a");
        self.line("    inc b");
        self.line("    jp .L_div_u8_loop");
        self.line(".L_div_u8_zero:");
        self.line("    xor a");
        self.line("    ret");
        self.line(".L_div_u8_done:");
        self.line("    ld a, b");
        self.line("    ret");
        self.line("__ezra_div_u16:");
        self.line("    ld a, b");
        self.line("    or c");
        self.line("    jp z, .L_div_u16_zero");
        self.line("    ex de, hl");
        self.line("    ld hl, 000000h");
        self.line(".L_div_u16_loop:");
        self.line("    push hl");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp c, .L_div_u16_done");
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    inc hl");
        self.line("    jp .L_div_u16_loop");
        self.line(".L_div_u16_zero:");
        self.line("    ld hl, 000000h");
        self.line("    ret");
        self.line(".L_div_u16_done:");
        self.line("    pop hl");
        self.line("    ret");
        self.line("__ezra_div_u24:");
        self.line("    push hl");
        self.line("    ld hl, 000000h");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp z, .L_div_u24_zero");
        self.line("    pop de");
        self.line("    ld hl, 000000h");
        self.line(".L_div_u24_loop:");
        self.line("    push hl");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp c, .L_div_u24_done");
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    inc hl");
        self.line("    jp .L_div_u24_loop");
        self.line(".L_div_u24_zero:");
        self.line("    pop hl");
        self.line("    ld hl, 000000h");
        self.line("    ret");
        self.line(".L_div_u24_done:");
        self.line("    pop hl");
        self.line("    ret");
        self.line("__ezra_mod_u8:");
        self.line("    ld d, a");
        self.line("    ld a, c");
        self.line("    or a");
        self.line("    jp z, .L_mod_u8_zero");
        self.line(".L_mod_u8_loop:");
        self.line("    ld a, d");
        self.line("    cp c");
        self.line("    jp c, .L_mod_u8_done");
        self.line("    sub c");
        self.line("    ld d, a");
        self.line("    jp .L_mod_u8_loop");
        self.line(".L_mod_u8_zero:");
        self.line("    xor a");
        self.line("    ret");
        self.line(".L_mod_u8_done:");
        self.line("    ld a, d");
        self.line("    ret");
        self.line("__ezra_mod_u16:");
        self.line("    ld a, b");
        self.line("    or c");
        self.line("    jp z, .L_mod_u16_zero");
        self.line("    ex de, hl");
        self.line(".L_mod_u16_loop:");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp c, .L_mod_u16_done");
        self.line("    ex de, hl");
        self.line("    jp .L_mod_u16_loop");
        self.line(".L_mod_u16_zero:");
        self.line("    ld hl, 000000h");
        self.line("    ret");
        self.line(".L_mod_u16_done:");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    ret");
        self.line("__ezra_mod_u24:");
        self.line("    push hl");
        self.line("    ld hl, 000000h");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp z, .L_mod_u24_zero");
        self.line("    pop de");
        self.line(".L_mod_u24_loop:");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, bc");
        self.line("    jp c, .L_mod_u24_done");
        self.line("    ex de, hl");
        self.line("    jp .L_mod_u24_loop");
        self.line(".L_mod_u24_zero:");
        self.line("    pop hl");
        self.line("    ld hl, 000000h");
        self.line("    ret");
        self.line(".L_mod_u24_done:");
        self.line("    push de");
        self.line("    pop hl");
        self.line("    ret");
        self.emit_signed_i24_div_mod_helper("__ezra_div_i24", BinaryOp::Div);
        self.emit_signed_i24_div_mod_helper("__ezra_mod_i24", BinaryOp::Mod);
    }

    fn emit_signed_i24_div_mod_helper(&mut self, label: &str, op: BinaryOp) {
        let dividend = self.alloc_var(ValueWidth::U24.bytes());
        let divisor = self.alloc_var(ValueWidth::U24.bytes());
        let quotient = self.alloc_var(ValueWidth::U24.bytes());
        let quotient_negative = self.alloc_var(ValueWidth::U8.bytes());
        let remainder_negative = self.alloc_var(ValueWidth::U8.bytes());
        let loop_label = self.next_label("sdiv_i24_loop");
        let zero_label = self.next_label("sdiv_i24_zero");
        let done_label = self.next_label("sdiv_i24_done");
        let quotient_positive_label = self.next_label("sdiv_i24_q_positive");
        let remainder_positive_label = self.next_label("sdiv_i24_r_positive");
        let not_overflow_label = self.next_label("sdiv_i24_not_overflow");

        self.line(&format!("{label}:"));
        self.emit_store_width(dividend);
        self.line("    push bc");
        self.line("    pop hl");
        self.emit_store_width(divisor);
        self.emit_zero_variable(quotient);
        self.emit_zero_variable(quotient_negative);
        self.emit_zero_variable(remainder_negative);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_jump_if_memory_not_equals(
            dividend,
            signed_min_bytes(ValueWidth::U24),
            &not_overflow_label,
        );
        self.emit_jump_if_memory_not_equals(
            divisor,
            signed_negative_one_bytes(ValueWidth::U24),
            &not_overflow_label,
        );
        match op {
            BinaryOp::Div => self.emit_load_width(dividend),
            BinaryOp::Mod => {
                self.emit_zero_variable(dividend);
                self.emit_load_width(dividend);
            }
            _ => unreachable!("not a division op"),
        }
        self.line("    ret");
        self.line(&format!("{not_overflow_label}:"));

        self.emit_abs_signed_variable(dividend, Some(quotient_negative), Some(remainder_negative));
        self.emit_abs_signed_variable(divisor, Some(quotient_negative), None);

        self.line(&format!("{loop_label}:"));
        self.emit_load_width(dividend);
        self.line("    push hl");
        self.emit_load_width(divisor);
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line(&format!("    jp c, {done_label}"));
        self.emit_store_width(dividend);
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        self.emit_load_a(quotient_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {quotient_positive_label}"));
        self.emit_negate_memory(quotient);
        self.line(&format!("{quotient_positive_label}:"));
        self.emit_load_a(remainder_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {remainder_positive_label}"));
        self.emit_negate_memory(dividend);
        self.line(&format!("{remainder_positive_label}:"));

        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        self.line("    ret");
    }

    fn emit_global_initializers(&mut self, program: &Program) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            let Declaration::Global(decl) = declaration else {
                continue;
            };
            let variable = self
                .symbols
                .globals
                .get(&decl.name)
                .copied()
                .expect("global allocation exists");
            self.emit_storage_initializer(variable, &decl.ty, &decl.value)?;
        }
        Ok(())
    }

    fn emit_embed_initializers(&mut self) {
        let embeds = self.symbols.embeds.values().cloned().collect::<Vec<_>>();
        for embed in embeds {
            for (offset, byte) in embed.bytes.into_iter().enumerate() {
                self.line(&format!("    ld a, {byte:02X}h"));
                self.emit_store_a(scalar_var(
                    embed.variable.addr + offset as u32,
                    u32::from(ValueWidth::U8.bytes()),
                ));
            }
        }
    }

    fn emit_string_literal_initializers(&mut self) {
        let mut literals = self
            .string_literals
            .iter()
            .map(|(value, variable)| (variable.addr, value.clone(), *variable))
            .collect::<Vec<_>>();
        literals.sort_by_key(|(addr, _, _)| *addr);
        for (_, value, variable) in literals {
            self.emit_string_literal_initializer(&value, variable);
        }
    }

    fn emit_string_literal_initializer(&mut self, value: &str, variable: Variable) {
        for (offset, byte) in value.bytes().chain(std::iter::once(0)).enumerate() {
            self.line(&format!("    ld a, {byte:02X}h"));
            self.emit_store_a(scalar_var(
                variable.addr + offset as u32,
                u32::from(ValueWidth::U8.bytes()),
            ));
        }
    }

    fn emit_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        validate_function_attrs(function)?;
        let naked = has_attr(function, "naked");
        let interrupt = has_attr(function, "interrupt");
        if interrupt {
            if !function.params.is_empty() {
                return Err(Diagnostic::new(format!(
                    "interrupt function `{}` cannot take parameters",
                    function.name
                )));
            }
            if function.return_type.is_some() {
                return Err(Diagnostic::new(format!(
                    "interrupt function `{}` cannot return a value",
                    function.name
                )));
            }
        }
        if naked {
            for stmt in &function.body {
                let Stmt::Asm {
                    inputs, outputs, ..
                } = stmt
                else {
                    return Err(Diagnostic::new(format!(
                        "naked function `{}` may contain only asm blocks",
                        function.name
                    )));
                };
                if !inputs.is_empty() || !outputs.is_empty() {
                    return Err(Diagnostic::new(format!(
                        "naked function `{}` asm blocks cannot use operands",
                        function.name
                    )));
                }
            }
        }
        if !naked
            && function.return_type.is_some()
            && !block_guarantees_value_return(&function.body, &self.symbols)
        {
            return Err(Diagnostic::new(format!(
                "missing return value in function `{}`",
                function.name
            )));
        }
        self.line(&format!("{}:", function_label(&function.name)));
        self.scopes.push(HashMap::new());
        self.scope_types.push(HashMap::new());
        self.local_constants.push(HashMap::new());
        self.readonly_pointer_aliases.push(HashMap::new());
        self.assigned_names_stack
            .push(assigned_names_in_block(&function.body));
        if let Some(return_type) = &function.return_type {
            self.symbols.type_width(return_type)?;
        }
        let uses_stack_frame = self
            .symbols
            .functions
            .get(&function.name)
            .is_some_and(|sig| sig.stack_arg_bytes > 0);
        self.return_type_stack.push(function.return_type.clone());
        self.return_value_stack.push(function.return_type.is_some());
        self.function_name_stack.push(function.name.clone());
        self.function_frame_stack.push(uses_stack_frame);
        self.function_interrupt_stack.push(interrupt);
        self.function_naked_stack.push(naked);
        self.function_storage_stack.push(Vec::new());
        if !naked {
            if interrupt {
                self.emit_interrupt_prologue();
            }
            if uses_stack_frame {
                self.emit_frame_prologue();
            }
            self.bind_params(function)?;
        }
        self.emit_block(&function.body)?;
        self.function_naked_stack.pop();
        self.function_interrupt_stack.pop();
        self.function_frame_stack.pop();
        self.function_name_stack.pop();
        self.function_storage_stack.pop();
        self.return_value_stack.pop();
        self.return_type_stack.pop();
        self.assigned_names_stack.pop();
        self.readonly_pointer_aliases.pop();
        self.local_constants.pop();
        self.scope_types.pop();
        self.scopes.pop();
        if naked {
            return Ok(());
        }
        if interrupt {
            self.emit_interrupt_epilogue();
            return Ok(());
        }
        if function.name == "main" {
            self.line("    jp __ezra_exit");
        } else {
            if uses_stack_frame {
                self.emit_frame_epilogue();
            }
            self.line("    ret");
        }
        Ok(())
    }

    fn emit_frame_prologue(&mut self) {
        self.line("    push ix");
        self.line("    ld ix, 000000h");
        self.line("    add ix, sp");
    }

    fn emit_frame_epilogue(&mut self) {
        self.line("    pop ix");
    }

    fn emit_interrupt_prologue(&mut self) {
        self.line("    push af");
        self.line("    push bc");
        self.line("    push de");
        self.line("    push hl");
        self.line("    push ix");
        self.line("    push iy");
    }

    fn emit_interrupt_epilogue(&mut self) {
        self.line("    pop iy");
        self.line("    pop ix");
        self.line("    pop hl");
        self.line("    pop de");
        self.line("    pop bc");
        self.line("    pop af");
        self.line("    reti");
    }

    fn bind_params(&mut self, function: &Function) -> Result<(), Diagnostic> {
        let sig = self
            .symbols
            .functions
            .get(&function.name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{}`", function.name)))?;

        for (index, param) in function.params.iter().enumerate() {
            if self.name_in_current_function(&param.name) {
                return Err(Diagnostic::new(format!(
                    "parameter `{}` shadows an existing name",
                    param.name
                )));
            }
            let width = self.symbols.type_width(&param.ty)?;
            let variable = self.alloc_var(width.bytes());
            self.current_scope_mut()
                .insert(param.name.clone(), variable);
            self.current_scope_types_mut()
                .insert(param.name.clone(), param.ty.clone());
            if sig.uses_arg_slots {
                let slot = sig.arg_slots[index];
                self.emit_load_width(slot);
                self.emit_store_width(variable);
                continue;
            }
            if let Some(offset) = sig.stack_arg_offsets[index] {
                self.emit_load_ix_offset_width_into(offset, variable)?;
                continue;
            }
            match width {
                ValueWidth::U8 => {
                    match index {
                        0 => {}
                        1 => self.line("    ld a, b"),
                        2 => self.line("    ld a, c"),
                        _ => unreachable!("param count checked"),
                    }
                    self.emit_store_a(variable);
                }
                ValueWidth::U16 | ValueWidth::U24 => {
                    match index {
                        0 => {}
                        1 => self.line("    ex de, hl"),
                        2 => {
                            self.line("    push bc");
                            self.line("    pop hl");
                        }
                        _ => unreachable!("param count checked"),
                    }
                    self.emit_store_width(variable);
                }
            }
        }
        Ok(())
    }

    fn emit_block(&mut self, body: &[Stmt]) -> Result<(), Diagnostic> {
        for stmt in body {
            self.emit_stmt(stmt)?;
            if self.eliminate_dead_code && self.stmt_terminates_current_block(stmt) {
                break;
            }
        }
        Ok(())
    }

    fn block_terminates_current_block(&self, body: &[Stmt]) -> bool {
        body.iter()
            .any(|stmt| self.stmt_terminates_current_block(stmt))
    }

    fn stmt_terminates_current_block(&self, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Return(_) | Stmt::Break | Stmt::Continue => true,
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                if let Ok(value) = self.eval_i64_with_local_constants(condition) {
                    if value == 0 {
                        return self.block_terminates_current_block(else_body);
                    }
                    return self.block_terminates_current_block(then_body);
                }
                !else_body.is_empty()
                    && self.block_terminates_current_block(then_body)
                    && self.block_terminates_current_block(else_body)
            }
            Stmt::Loop { body } => {
                !block_can_break_current_loop(body) && self.block_terminates_current_block(body)
            }
            Stmt::While { condition, body } => {
                self.eval_i64_with_local_constants(condition)
                    .is_ok_and(|value| value != 0)
                    && !block_can_break_current_loop(body)
                    && self.block_terminates_current_block(body)
            }
            _ => false,
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        if self.debug_comments {
            self.line(&format!("    ; source: {}", stmt_summary(stmt)));
        }
        match stmt {
            Stmt::Let { name, ty, value } => {
                if self.name_in_current_function(name) {
                    return Err(Diagnostic::new(format!(
                        "local `{name}` shadows an existing name"
                    )));
                }
                let variable = self.alloc_storage(ty)?;
                self.current_scope_mut().insert(name.clone(), variable);
                self.current_scope_types_mut()
                    .insert(name.clone(), ty.clone());
                self.emit_storage_initializer(variable, ty, value)?;
                self.record_local_constant(name, ty, value);
                self.record_readonly_pointer_alias(name, value);
            }
            Stmt::Assign { target, op, value } => {
                self.emit_assignment(target, *op, value)?;
            }
            Stmt::Out { port, value } => {
                let port = self.port(port)?;
                self.validate_expr_assignable_to_type(value, &Type::Named("u8".to_owned()))?;
                self.emit_expr_to_a(value)?;
                self.emit_out_a(port);
            }
            Stmt::Expr(Expr::Call { path, args }) => self.emit_call(path, args)?,
            Stmt::Expr(expr) => {
                let width = self.expr_width(expr)?;
                self.emit_expr_to_width(expr, width)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.ensure_expr_is_bool(condition, "if condition")?;
                if self.eliminate_dead_code
                    && let Ok(value) = self.eval_i64_with_local_constants(condition)
                {
                    if value == 0 {
                        self.emit_block(else_body)?;
                    } else {
                        self.emit_block(then_body)?;
                    }
                    return Ok(());
                }
                let else_label = self.next_label("else");
                let end_label = self.next_label("endif");
                self.emit_expr_to_a(condition)?;
                self.line("    or a");
                self.line(&format!("    jp z, {else_label}"));
                self.emit_block(then_body)?;
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{else_label}:"));
                self.emit_block(else_body)?;
                self.line(&format!("{end_label}:"));
            }
            Stmt::While { condition, body } => {
                self.ensure_expr_is_bool(condition, "while condition")?;
                let mut condition_is_always_true = false;
                if self.eliminate_dead_code
                    && let Ok(value) = self.eval_i64_with_local_constants(condition)
                {
                    if value == 0 {
                        return Ok(());
                    }
                    condition_is_always_true = true;
                }
                let start_label = self.next_label("while");
                let end_label = self.next_label("endwhile");
                self.loop_stack.push(LoopLabels {
                    continue_label: start_label.clone(),
                    break_label: end_label.clone(),
                });
                self.line(&format!("{start_label}:"));
                if !condition_is_always_true {
                    self.emit_expr_to_a(condition)?;
                    self.line("    or a");
                    self.line(&format!("    jp z, {end_label}"));
                }
                self.emit_block(body)?;
                self.line(&format!("    jp {start_label}"));
                self.line(&format!("{end_label}:"));
                self.loop_stack.pop();
            }
            Stmt::Loop { body } => {
                let start_label = self.next_label("loop");
                let end_label = self.next_label("endloop");
                self.loop_stack.push(LoopLabels {
                    continue_label: start_label.clone(),
                    break_label: end_label.clone(),
                });
                self.line(&format!("{start_label}:"));
                self.emit_block(body)?;
                self.line(&format!("    jp {start_label}"));
                self.line(&format!("{end_label}:"));
                self.loop_stack.pop();
            }
            Stmt::Break => {
                let Some(labels) = self.loop_stack.last() else {
                    return Err(Diagnostic::new("`break` outside loop"));
                };
                self.line(&format!("    jp {}", labels.break_label));
            }
            Stmt::Continue => {
                let Some(labels) = self.loop_stack.last() else {
                    return Err(Diagnostic::new("`continue` outside loop"));
                };
                self.line(&format!("    jp {}", labels.continue_label));
            }
            Stmt::Return(None) => {
                if self.current_function_requires_return_value() {
                    return Err(Diagnostic::new(format!(
                        "missing return value in function `{}`",
                        self.current_function_name()
                    )));
                }
                if self.current_function_uses_frame() {
                    self.emit_frame_epilogue();
                }
                if self.current_function_is_interrupt() {
                    self.emit_interrupt_epilogue();
                } else {
                    self.line("    ret");
                }
            }
            Stmt::Return(Some(expr)) => {
                if !self.current_function_requires_return_value() {
                    return Err(Diagnostic::new(format!(
                        "void function `{}` cannot return a value",
                        self.current_function_name()
                    )));
                }
                let return_type = self.current_return_type().clone();
                self.emit_expr_to_type(expr, &return_type)?;
                if self.current_function_uses_frame() {
                    self.emit_frame_epilogue();
                }
                if self.current_function_is_interrupt() {
                    self.emit_interrupt_epilogue();
                } else {
                    self.line("    ret");
                }
            }
            Stmt::Asm {
                volatile,
                inputs,
                outputs,
                clobbers,
                lines,
            } => self.emit_inline_asm(*volatile, inputs, outputs, clobbers, lines)?,
        }
        Ok(())
    }

    fn emit_inline_asm(
        &mut self,
        volatile: bool,
        inputs: &[crate::ast::AsmInput],
        outputs: &[crate::ast::AsmOutput],
        clobbers: &[String],
        lines: &[String],
    ) -> Result<(), Diagnostic> {
        let mut operands = HashMap::new();

        if volatile {
            self.line("    ; asm volatile");
        } else {
            self.line("    ; asm");
        }
        for input in inputs {
            if operands.contains_key(&input.name) {
                return Err(Diagnostic::new(format!(
                    "duplicate inline asm operand `{}`",
                    input.name
                )));
            }
            self.validate_inline_asm_input_type(input)?;
            let binding = self.inline_asm_input_binding(input)?;
            self.line(&format!(
                "    ; in {}: {} as {}",
                input.name,
                type_display(&input.ty),
                input.class
            ));
            operands.insert(input.name.clone(), binding);
        }
        for output in outputs {
            if operands.contains_key(&output.name) {
                return Err(Diagnostic::new(format!(
                    "duplicate inline asm operand `{}`",
                    output.name
                )));
            }
            self.validate_inline_asm_output_type(output)?;
            let binding = self.inline_asm_output_binding(output)?;
            self.line(&format!(
                "    ; out {}: {} as {}",
                output.name,
                type_display(&output.ty),
                output.class
            ));
            operands.insert(output.name.clone(), binding);
        }
        if !clobbers.is_empty() {
            self.line(&format!("    ; clobber {}", clobbers.join(", ")));
        }
        if (inputs.iter().any(|input| input.class == "mem")
            || outputs.iter().any(|output| output.class == "mem"))
            && !asm_clobbers_include(clobbers, "memory")
        {
            return Err(Diagnostic::new(
                "inline asm uses memory without declaring clobber `memory`",
            ));
        }
        let substituted_lines = lines
            .iter()
            .map(|line| substitute_inline_asm_operands(line, &operands))
            .collect::<Result<Vec<_>, _>>()?;
        validate_inline_asm_clobbers(
            clobbers,
            &substituted_lines,
            self.current_function_is_naked(),
            self.cpu.into(),
        )?;

        for input in inputs {
            self.emit_inline_asm_input_load(input)?;
        }
        let preserve_ix = !self.current_function_is_naked() && asm_clobbers_include(clobbers, "ix");
        let preserve_iy = !self.current_function_is_naked() && asm_clobbers_include(clobbers, "iy");
        if preserve_ix {
            self.line("    push ix");
        }
        if preserve_iy {
            self.line("    push iy");
        }
        for line in &substituted_lines {
            self.line(&format!("    {line}"));
        }
        if preserve_iy {
            self.line("    pop iy");
        }
        if preserve_ix {
            self.line("    pop ix");
        }
        for output in outputs {
            self.emit_inline_asm_output_store(output)?;
            self.invalidate_local_constant(&output.name);
            self.invalidate_readonly_pointer_alias(&output.name);
        }
        if asm_clobbers_include(clobbers, "memory") {
            self.invalidate_all_local_constants();
        }
        Ok(())
    }

    fn validate_inline_asm_input_type(
        &self,
        input: &crate::ast::AsmInput,
    ) -> Result<(), Diagnostic> {
        let Some(bound) = self.named_value_type(&input.name) else {
            return Err(Diagnostic::new(format!("unknown value `{}`", input.name)));
        };
        let declared = self.symbols.resolved_type(&input.ty)?;
        let bound = self.symbols.resolved_type(bound)?;
        if declared != bound {
            return Err(Diagnostic::new(format!(
                "inline asm input `{}` declared type `{}` does not match bound type `{}`",
                input.name,
                type_display(&declared),
                type_display(&bound)
            )));
        }
        Ok(())
    }

    fn validate_inline_asm_output_type(
        &self,
        output: &crate::ast::AsmOutput,
    ) -> Result<(), Diagnostic> {
        let Some(bound) = self.variable_type(&output.name) else {
            return Err(Diagnostic::new(format!(
                "unknown variable `{}`",
                output.name
            )));
        };
        let declared = self.symbols.resolved_type(&output.ty)?;
        let bound = self.symbols.resolved_type(bound)?;
        if declared != bound {
            return Err(Diagnostic::new(format!(
                "inline asm output `{}` declared type `{}` does not match bound type `{}`",
                output.name,
                type_display(&declared),
                type_display(&bound)
            )));
        }
        Ok(())
    }

    fn inline_asm_input_binding(&self, input: &crate::ast::AsmInput) -> Result<String, Diagnostic> {
        match input.class.as_str() {
            "reg8" => Ok("a".to_owned()),
            "reg16" | "reg24" => Ok("hl".to_owned()),
            "mem" => {
                let variable = self.variable(&input.name)?;
                Ok(format!("({:06X}h)", variable.addr))
            }
            "imm" => {
                let width = self.symbols.type_width(&input.ty)?;
                let value = self.eval_i64_with_local_constants(&Expr::Ident(input.name.clone()))?;
                Ok(format_immediate(value, width))
            }
            _ => Err(Diagnostic::new(format!(
                "unsupported inline asm operand class `{}`",
                input.class
            ))),
        }
    }

    fn inline_asm_output_binding(
        &self,
        output: &crate::ast::AsmOutput,
    ) -> Result<String, Diagnostic> {
        match output.class.as_str() {
            "reg8" => Ok("a".to_owned()),
            "reg16" | "reg24" => Ok("hl".to_owned()),
            "mem" => {
                let variable = self.variable(&output.name)?;
                Ok(format!("({:06X}h)", variable.addr))
            }
            "imm" => Err(Diagnostic::new(format!(
                "inline asm output `{}` cannot use imm class",
                output.name
            ))),
            _ => Err(Diagnostic::new(format!(
                "unsupported inline asm operand class `{}`",
                output.class
            ))),
        }
    }

    fn emit_inline_asm_input_load(
        &mut self,
        input: &crate::ast::AsmInput,
    ) -> Result<(), Diagnostic> {
        match input.class.as_str() {
            "reg8" => {
                if let Some(variable) = self.variable_opt(&input.name) {
                    self.emit_load_a(variable);
                } else {
                    let value = self.u8(&Expr::Ident(input.name.clone()))?;
                    self.line(&format!("    ld a, {value:02X}h"));
                }
            }
            "reg16" | "reg24" => {
                let width = self.symbols.type_width(&input.ty)?;
                if let Some(variable) = self.variable_opt(&input.name) {
                    self.emit_load_width(variable);
                } else {
                    let value = self.symbols.eval_i64(&Expr::Ident(input.name.clone()))?;
                    self.line(&format!("    ld hl, {}", format_immediate(value, width)));
                }
            }
            "mem" | "imm" => {}
            _ => {
                return Err(Diagnostic::new(format!(
                    "unsupported inline asm operand class `{}`",
                    input.class
                )));
            }
        }
        Ok(())
    }

    fn emit_inline_asm_output_store(
        &mut self,
        output: &crate::ast::AsmOutput,
    ) -> Result<(), Diagnostic> {
        match output.class.as_str() {
            "reg8" | "reg16" | "reg24" => {
                let variable = self.variable(&output.name)?;
                self.emit_store_width(variable);
            }
            "mem" => {}
            "imm" => {
                return Err(Diagnostic::new(format!(
                    "inline asm output `{}` cannot use imm class",
                    output.name
                )));
            }
            _ => {
                return Err(Diagnostic::new(format!(
                    "unsupported inline asm operand class `{}`",
                    output.class
                )));
            }
        }
        Ok(())
    }

    fn emit_assignment_value(
        &mut self,
        variable: Variable,
        op: AssignOp,
        value: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if variable.size == 2 {
            match op {
                AssignOp::Set => self.emit_expr_to_hl(value, variable.width()?)?,
                AssignOp::Add => self.emit_wide_assignment_op(variable, BinaryOp::Add, value)?,
                AssignOp::Sub => self.emit_wide_assignment_op(variable, BinaryOp::Sub, value)?,
                AssignOp::Mul => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Mul, value, signed)?
                }
                AssignOp::Div => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Div, value, signed)?
                }
                AssignOp::Mod => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Mod, value, signed)?
                }
                AssignOp::BitAnd => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitAnd, value)?
                }
                AssignOp::BitOr => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitOr, value)?
                }
                AssignOp::BitXor => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitXor, value)?
                }
                AssignOp::Shl => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shl, value, signed)?
                }
                AssignOp::Shr => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shr, value, signed)?
                }
            }
            return Ok(());
        }
        if variable.size == 3 {
            match op {
                AssignOp::Set => self.emit_expr_to_hl(value, ValueWidth::U24)?,
                AssignOp::Add => self.emit_wide_assignment_op(variable, BinaryOp::Add, value)?,
                AssignOp::Sub => self.emit_wide_assignment_op(variable, BinaryOp::Sub, value)?,
                AssignOp::Mul => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Mul, value, signed)?
                }
                AssignOp::Div => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Div, value, signed)?
                }
                AssignOp::Mod => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Mod, value, signed)?
                }
                AssignOp::BitAnd => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitAnd, value)?
                }
                AssignOp::BitOr => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitOr, value)?
                }
                AssignOp::BitXor => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitXor, value)?
                }
                AssignOp::Shl => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shl, value, signed)?
                }
                AssignOp::Shr => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shr, value, signed)?
                }
            }
            return Ok(());
        }

        match op {
            AssignOp::Set => self.emit_expr_to_a(value)?,
            AssignOp::Add => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    add a, b");
            }
            AssignOp::Sub => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    ld c, a");
                self.line("    ld a, b");
                self.line("    sub c");
            }
            AssignOp::Mul => {
                self.emit_arithmetic_assignment_op(variable, BinaryOp::Mul, value, signed)?
            }
            AssignOp::Div => {
                self.emit_arithmetic_assignment_op(variable, BinaryOp::Div, value, signed)?
            }
            AssignOp::Mod => {
                self.emit_arithmetic_assignment_op(variable, BinaryOp::Mod, value, signed)?
            }
            AssignOp::BitAnd => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    and b");
            }
            AssignOp::BitOr => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    or b");
            }
            AssignOp::BitXor => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    xor b");
            }
            AssignOp::Shl => {
                self.ensure_shift_count_compatible(value)?;
                self.emit_load_a(variable);
                self.emit_shift_a_by_expr(BinaryOp::Shl, value, signed)?;
            }
            AssignOp::Shr => {
                self.ensure_shift_count_compatible(value)?;
                self.emit_load_a(variable);
                self.emit_shift_a_by_expr(BinaryOp::Shr, value, signed)?;
            }
        }
        Ok(())
    }

    fn emit_arithmetic_assignment_op(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        value: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        let width = variable.width()?;
        match op {
            BinaryOp::Mul => self.emit_assignment_mul(variable, value, width, signed),
            BinaryOp::Div | BinaryOp::Mod => {
                if signed {
                    self.emit_signed_assignment_div_mod(variable, value, op, width)
                } else {
                    self.emit_unsigned_assignment_div_mod(variable, value, op, width)
                }
            }
            _ => unreachable!("not an arithmetic compound assignment op"),
        }
    }

    fn emit_assignment_mul(
        &mut self,
        variable: Variable,
        value: &Expr,
        width: ValueWidth,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U8 {
            let left = self.alloc_var(width.bytes());
            self.emit_load_a(variable);
            self.emit_store_a(left);
            self.emit_expr_to_a(value)?;
            self.line("    ld c, a");
            self.emit_load_a(left);
            self.line("    call __ezra_mul_u8");
            return Ok(());
        }

        self.emit_load_width(variable);
        self.line("    push hl");
        self.emit_expr_to_hl(value, width)?;
        self.line("    push hl");
        self.line("    pop bc");
        self.line("    pop hl");
        match width {
            ValueWidth::U16 => self.line("    call __ezra_mul_u16"),
            ValueWidth::U24 if signed => self.line("    call __ezra_mul_i24"),
            ValueWidth::U24 => self.line("    call __ezra_mul_u24"),
            ValueWidth::U8 => unreachable!("u8 handled above"),
        }
        Ok(())
    }

    fn emit_unsigned_assignment_div_mod(
        &mut self,
        variable: Variable,
        value: &Expr,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U8 {
            let left = self.alloc_var(width.bytes());
            self.emit_load_a(variable);
            self.emit_store_a(left);
            self.emit_expr_to_a(value)?;
            self.line("    ld c, a");
            self.emit_load_a(left);
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_u8"),
                BinaryOp::Mod => self.line("    call __ezra_mod_u8"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }

        self.emit_load_width(variable);
        self.line("    push hl");
        self.emit_expr_to_hl(value, width)?;
        self.line("    push hl");
        self.line("    pop bc");
        self.line("    pop hl");
        match (op, width) {
            (BinaryOp::Div, ValueWidth::U16) => self.line("    call __ezra_div_u16"),
            (BinaryOp::Mod, ValueWidth::U16) => self.line("    call __ezra_mod_u16"),
            (BinaryOp::Div, ValueWidth::U24) => self.line("    call __ezra_div_u24"),
            (BinaryOp::Mod, ValueWidth::U24) => self.line("    call __ezra_mod_u24"),
            _ => unreachable!("unsupported unsigned assignment division width"),
        }
        Ok(())
    }

    fn emit_signed_assignment_div_mod(
        &mut self,
        variable: Variable,
        value: &Expr,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U24 {
            self.emit_load_width(variable);
            self.line("    push hl");
            self.emit_expr_to_hl(value, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_i24"),
                BinaryOp::Mod => self.line("    call __ezra_mod_i24"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }

        let dividend = self.alloc_var(width.bytes());
        let divisor = self.alloc_var(width.bytes());
        let quotient = self.alloc_var(width.bytes());
        let quotient_negative = self.alloc_var(ValueWidth::U8.bytes());
        let remainder_negative = self.alloc_var(ValueWidth::U8.bytes());
        let loop_label = self.next_label("sdiv_loop");
        let zero_label = self.next_label("sdiv_zero");
        let done_label = self.next_label("sdiv_done");
        let quotient_positive_label = self.next_label("sdiv_q_positive");
        let remainder_positive_label = self.next_label("sdiv_r_positive");
        let not_overflow_label = self.next_label("sdiv_not_overflow");
        let finished_label = self.next_label("sdiv_finished");

        self.emit_load_width(variable);
        self.emit_store_width(dividend);
        self.emit_expr_to_width(value, width)?;
        self.emit_store_width(divisor);
        self.emit_zero_variable(quotient);
        self.emit_zero_variable(quotient_negative);
        self.emit_zero_variable(remainder_negative);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_jump_if_memory_not_equals(dividend, signed_min_bytes(width), &not_overflow_label);
        self.emit_jump_if_memory_not_equals(
            divisor,
            signed_negative_one_bytes(width),
            &not_overflow_label,
        );
        match op {
            BinaryOp::Div => self.emit_load_width(dividend),
            BinaryOp::Mod => {
                self.emit_zero_variable(dividend);
                self.emit_load_width(dividend);
            }
            _ => unreachable!("not a division op"),
        }
        self.line(&format!("    jp {finished_label}"));
        self.line(&format!("{not_overflow_label}:"));

        self.emit_abs_signed_variable(dividend, Some(quotient_negative), Some(remainder_negative));
        self.emit_abs_signed_variable(divisor, Some(quotient_negative), None);

        self.line(&format!("{loop_label}:"));
        if width == ValueWidth::U8 {
            self.emit_load_a(dividend);
            self.line("    ld b, a");
            self.emit_load_a(divisor);
            self.line("    ld c, a");
            self.line("    ld a, b");
            self.line("    cp c");
            self.line(&format!("    jp c, {done_label}"));
            self.line("    sub c");
            self.emit_store_a(dividend);
        } else {
            self.emit_load_width(dividend);
            self.line("    push hl");
            self.emit_load_width(divisor);
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, de");
            self.line(&format!("    jp c, {done_label}"));
            self.emit_store_width(dividend);
        }
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        self.emit_load_a(quotient_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {quotient_positive_label}"));
        self.emit_negate_memory(quotient);
        self.line(&format!("{quotient_positive_label}:"));
        self.emit_load_a(remainder_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {remainder_positive_label}"));
        self.emit_negate_memory(dividend);
        self.line(&format!("{remainder_positive_label}:"));

        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        self.line(&format!("{finished_label}:"));
        Ok(())
    }

    fn emit_typed_assignment_value(
        &mut self,
        variable: Variable,
        ty: &Type,
        op: AssignOp,
        value: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if op != AssignOp::Set {
            let resolved = self.symbols.resolved_type(ty)?;
            match &resolved {
                Type::Ptr(pointee) => {
                    return self.emit_pointer_compound_assignment(variable, pointee, op, value);
                }
                Type::Array { .. } => return Err(Diagnostic::new("type mismatch")),
                Type::Named(name) if name == "bool" || self.symbols.structs.contains_key(name) => {
                    return Err(Diagnostic::new("type mismatch"));
                }
                _ => {}
            }
        }
        self.emit_assignment_value(variable, op, value, signed)
    }

    fn emit_pointer_compound_assignment(
        &mut self,
        variable: Variable,
        pointee: &Type,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let binary_op = match op {
            AssignOp::Add => BinaryOp::Add,
            AssignOp::Sub => BinaryOp::Sub,
            _ => return Err(Diagnostic::new("type mismatch")),
        };
        self.ensure_pointer_offset_expr(value)?;
        let scale = self.symbols.type_size(pointee)?;
        self.emit_load_width(variable);
        self.line("    push hl");
        self.emit_scaled_offset_to_hl(value, scale)?;
        match binary_op {
            BinaryOp::Add => {
                self.line("    pop bc");
                self.line("    add hl, bc");
            }
            BinaryOp::Sub => {
                self.line("    ex de, hl");
                self.line("    pop hl");
                self.line("    or a");
                self.line("    sbc hl, de");
            }
            _ => unreachable!("pointer compound assignment only uses add/sub"),
        }
        Ok(())
    }

    fn ensure_compound_assignment_target(&self, ty: &Type, op: AssignOp) -> Result<(), Diagnostic> {
        if op == AssignOp::Set {
            return Ok(());
        }
        match self.symbols.resolved_type(ty)? {
            Type::Ptr(_) if matches!(op, AssignOp::Add | AssignOp::Sub) => Ok(()),
            Type::Ptr(_) => Err(Diagnostic::new("type mismatch")),
            Type::Array { .. } => Err(Diagnostic::new("type mismatch")),
            Type::Named(name) if name == "bool" || self.symbols.structs.contains_key(&name) => {
                Err(Diagnostic::new("type mismatch"))
            }
            Type::Named(_) => Ok(()),
        }
    }

    fn emit_assignment(
        &mut self,
        target: &Place,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        match target {
            Place::Ident(name) => {
                self.invalidate_local_constant(name);
                self.invalidate_readonly_pointer_alias(name);
                let variable = self.variable(name)?;
                let ty = self.variable_type(name).cloned();
                if op == AssignOp::Set
                    && let Some(ty) = ty.as_ref()
                {
                    self.emit_storage_initializer(variable, ty, value)?;
                    self.record_local_constant(name, ty, value);
                    self.record_readonly_pointer_alias(name, value);
                    return Ok(());
                }
                let signed = self
                    .variable_type(name)
                    .map(|ty| self.type_is_signed(ty))
                    .transpose()?
                    .unwrap_or(false);
                if let Some(ty) = ty.as_ref() {
                    self.emit_typed_assignment_value(variable, ty, op, value, signed)?;
                } else {
                    self.emit_assignment_value(variable, op, value, signed)?;
                }
                self.emit_store_width(variable);
            }
            Place::Index { name, index } => {
                self.emit_index_assignment(name, index, op, value)?;
            }
            Place::Field { base, field } => {
                let variable = self.field_variable(base, field)?;
                if op == AssignOp::Set {
                    let ty = self.field_type(base, field)?;
                    self.validate_expr_assignable_to_type(value, &ty)?;
                    self.emit_storage_initializer(variable, &ty, value)?;
                    return Ok(());
                }
                let ty = self.field_type(base, field)?;
                self.ensure_compound_assignment_target(&ty, op)?;
                variable.width()?;
                let signed = self.type_is_signed(&ty)?;
                self.emit_typed_assignment_value(variable, &ty, op, value, signed)?;
                self.emit_store_width(variable);
            }
            Place::Access(path) => {
                self.emit_access_assignment(path, op, value)?;
            }
            Place::Deref(ptr) => {
                self.emit_deref_assignment(ptr, op, value)?;
            }
        }
        Ok(())
    }

    fn emit_array_initializer(
        &mut self,
        variable: Variable,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let Expr::Array(values) = value else {
            return Err(Diagnostic::new(
                "array initializer must be an array literal",
            ));
        };
        let element_size = variable
            .element_size
            .ok_or_else(|| Diagnostic::new("scalar variable cannot use array initializer"))?;
        let len = variable
            .len
            .ok_or_else(|| Diagnostic::new("array variable missing length"))?;
        let Type::Array {
            element: element_ty,
            ..
        } = self.symbols.resolved_type(ty)?
        else {
            return Err(Diagnostic::new("array initializer requires an array type"));
        };
        if values.len() as u32 > len {
            return Err(Diagnostic::new(format!(
                "array initializer has {} values but array length is {len}",
                values.len()
            )));
        }
        for index in 0..len {
            let element_addr = variable.addr + index * element_size;
            let element = self.symbols.storage_at(element_addr, &element_ty)?;
            if let Some(value) = values.get(index as usize) {
                self.validate_expr_assignable_to_type(value, &element_ty)?;
                match self.symbols.resolved_type(&element_ty)? {
                    Type::Array { .. } => {
                        self.emit_array_initializer(element, &element_ty, value)?
                    }
                    Type::Named(name) if self.symbols.structs.contains_key(&name) => {
                        self.emit_struct_initializer(element, &element_ty, value)?
                    }
                    _ => {
                        self.emit_expr_to_width(value, element.width()?)?;
                        self.emit_store_width(element);
                    }
                }
            } else {
                self.emit_zero_storage(element);
            }
        }
        Ok(())
    }

    fn emit_struct_initializer(
        &mut self,
        variable: Variable,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let struct_name = self.struct_type_name(ty)?;
        let Expr::StructInit { ty, fields } = value else {
            return Err(Diagnostic::new(format!(
                "struct `{struct_name}` initializer must use `{struct_name} {{ ... }}`"
            )));
        };
        if ty != &struct_name {
            return Err(Diagnostic::new(format!(
                "initializer type `{ty}` does not match `{struct_name}`"
            )));
        }

        let layout = self
            .symbols
            .structs
            .get(&struct_name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown struct `{struct_name}`")))?;
        let mut initialized = HashMap::new();
        for (field_name, field_value) in fields {
            let Some(field) = layout.fields.get(field_name) else {
                return Err(Diagnostic::new(format!(
                    "struct `{struct_name}` has no field `{field_name}`"
                )));
            };
            if initialized.insert(field_name.clone(), ()).is_some() {
                return Err(Diagnostic::new(format!(
                    "duplicate initializer for field `{field_name}`"
                )));
            }
            let field_var = self
                .symbols
                .storage_at(variable.addr + field.offset, &field.ty)?;
            self.validate_expr_assignable_to_type(field_value, &field.ty)?;
            self.emit_storage_initializer(field_var, &field.ty, field_value)?;
        }

        for (field_name, field) in &layout.fields {
            if initialized.contains_key(field_name) {
                continue;
            }
            let field_var = self
                .symbols
                .storage_at(variable.addr + field.offset, &field.ty)?;
            self.emit_zero_storage(field_var);
        }
        Ok(())
    }

    fn emit_storage_initializer(
        &mut self,
        variable: Variable,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        self.validate_expr_arithmetic_compatibility(value)?;
        self.validate_expr_assignable_to_type(value, ty)?;
        if let Expr::Deref(ptr) = value {
            return self.emit_copy_pointed_storage_into(ptr, variable);
        }
        if let Some(source) = self.expr_storage_variable(value)? {
            return self.emit_copy_storage_into(source, variable);
        }
        match self.symbols.resolved_type(ty)? {
            Type::Array { .. } => self.emit_array_initializer(variable, ty, value),
            Type::Named(name) if self.symbols.structs.contains_key(&name) => {
                self.emit_struct_initializer(variable, ty, value)
            }
            _ => {
                let width = variable.width()?;
                if width == ValueWidth::U24
                    && let Ok(value) = self.eval_i64_with_local_constants(value)
                {
                    self.validate_value_width_for_target((value as u32) & 0xFF_FFFF, width)?;
                }
                self.emit_expr_to_width(value, width)?;
                self.emit_store_width(variable);
                Ok(())
            }
        }
    }

    fn emit_wide_assignment_op(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        self.emit_load_width(variable);
        self.line("    push hl");
        self.emit_expr_to_hl(value, variable.width()?)?;
        self.line("    pop bc");
        self.emit_wide_op_with_left_in_bc(op, variable.width()?)?;
        Ok(())
    }

    fn emit_wide_assignment_shift(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        value: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        self.ensure_shift_count_compatible(value)?;
        let temp = self.alloc_var(variable.width()?.bytes());
        self.emit_load_width(variable);
        self.emit_store_width(temp);
        self.emit_shift_memory_by_expr(temp, op, value, signed)?;
        self.emit_load_width(temp);
        Ok(())
    }

    fn emit_call(&mut self, path: &[String], args: &[Expr]) -> Result<(), Diagnostic> {
        match path_text(path).as_str() {
            "test.pass" | "ezra.test.pass" => {
                self.line("    call __ezra_pass");
            }
            "test.fail" | "ezra.test.fail" => {
                let expr = args.first().cloned().unwrap_or(Expr::Int(1));
                self.validate_expr_assignable_to_type(&expr, &Type::Named("u8".to_owned()))?;
                self.emit_expr_to_a(&expr)?;
                self.emit_test_fail_call();
            }
            "test.assert_eq_u8" | "ezra.test.assert_eq_u8" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u8 requires three arguments",
                    ));
                }
                self.validate_expr_has_test_width(&args[0], ValueWidth::U8, true)?;
                self.validate_expr_has_test_width(&args[1], ValueWidth::U8, true)?;
                self.validate_expr_assignable_to_type(&args[2], &Type::Named("u8".to_owned()))?;
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_a(&args[0])?;
                self.line("    ld b, a");
                self.emit_expr_to_a(&args[1])?;
                self.line("    ld c, a");
                self.line("    ld a, b");
                self.line("    cp c");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_test_fail_call();
                self.line(&format!("{ok}:"));
            }
            "test.assert_eq_u16" | "ezra.test.assert_eq_u16" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u16 requires three arguments",
                    ));
                }
                self.validate_expr_has_test_width(&args[0], ValueWidth::U16, false)?;
                self.validate_expr_has_test_width(&args[1], ValueWidth::U16, false)?;
                self.validate_expr_assignable_to_type(&args[2], &Type::Named("u8".to_owned()))?;
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_hl(&args[0], ValueWidth::U16)?;
                self.line("    push hl");
                self.emit_expr_to_hl(&args[1], ValueWidth::U16)?;
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_test_fail_call();
                self.line(&format!("{ok}:"));
            }
            "test.assert_eq_u24" | "ezra.test.assert_eq_u24" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u24 requires three arguments",
                    ));
                }
                self.validate_expr_has_test_width(&args[0], ValueWidth::U24, false)?;
                self.validate_expr_has_test_width(&args[1], ValueWidth::U24, false)?;
                self.validate_expr_assignable_to_type(&args[2], &Type::Named("u8".to_owned()))?;
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_expr_to_hl(&args[1], ValueWidth::U24)?;
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_test_fail_call();
                self.line(&format!("{ok}:"));
            }
            "debug.char" | "ezra.debug.char" => {
                let expr = args
                    .first()
                    .ok_or_else(|| Diagnostic::new("debug.char requires one argument"))?;
                self.validate_expr_assignable_to_type(expr, &Type::Named("u8".to_owned()))?;
                self.emit_expr_to_a(expr)?;
                self.emit_out_a(0x0C);
            }
            "debug.str" | "ezra.debug.str" => {
                self.emit_debug_str(args)?;
            }
            "debug.hex_u8" | "ezra.debug.hex_u8" => {
                self.emit_debug_hex(args, ValueWidth::U8)?;
            }
            "debug.hex_u16" | "ezra.debug.hex_u16" => {
                self.emit_debug_hex(args, ValueWidth::U16)?;
            }
            "debug.hex_u24" | "ezra.debug.hex_u24" => {
                self.emit_debug_hex(args, ValueWidth::U24)?;
            }
            "mem.poke8" | "ezra.mem.poke8" => {
                self.emit_mem_poke8(args)?;
            }
            "mem.memcpy" | "ezra.mem.memcpy" => {
                self.emit_memcpy(args)?;
            }
            "mem.memset" | "ezra.mem.memset" => {
                self.emit_memset(args)?;
            }
            path => self.emit_user_call(path, args)?,
        }
        Ok(())
    }

    fn emit_test_fail_call(&mut self) {
        self.line("    call __ezra_fail");
    }

    fn emit_debug_str(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            return Err(Diagnostic::new("debug.str requires one argument"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;

        let cursor = self.alloc_var(ValueWidth::U24.bytes());
        let loop_label = self.next_label("debug_str");
        let done_label = self.next_label("debug_str_done");
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(cursor);
        self.line(&format!("{loop_label}:"));
        self.emit_load_hl(cursor);
        self.line("    ld a, (hl)");
        self.line("    or a");
        self.line(&format!("    jp z, {done_label}"));
        self.emit_out_a(0x0C);
        self.emit_load_hl(cursor);
        self.line("    inc hl");
        self.emit_store_hl(cursor);
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done_label}:"));
        Ok(())
    }

    fn emit_debug_hex(&mut self, args: &[Expr], width: ValueWidth) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            let suffix = match width {
                ValueWidth::U8 => "u8",
                ValueWidth::U16 => "u16",
                ValueWidth::U24 => "u24",
            };
            return Err(Diagnostic::new(format!(
                "debug.hex_{suffix} requires one argument"
            )));
        }
        if matches!(
            self.symbols.resolved_type(&self.expr_type(&args[0])?)?,
            Type::Array { .. }
        ) {
            return Err(Diagnostic::new("array value cannot be used as a scalar"));
        }
        self.validate_expr_assignable_to_type(&args[0], &width_unsigned_type(width))?;

        match width {
            ValueWidth::U8 => {
                self.emit_expr_to_a(&args[0])?;
                self.emit_debug_hex_byte_from_a();
            }
            ValueWidth::U16 | ValueWidth::U24 => {
                let value = self.alloc_var(width.bytes());
                self.emit_expr_to_hl(&args[0], width)?;
                self.emit_store_width(value);
                for offset in (0..width.bytes()).rev() {
                    self.line(&format!("    ld a, ({:06X}h)", value.addr + offset as u32));
                    self.emit_debug_hex_byte_from_a();
                }
            }
        }
        Ok(())
    }

    fn emit_debug_hex_byte_from_a(&mut self) {
        let byte = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_store_a(byte);
        self.emit_load_a(byte);
        for _ in 0..4 {
            self.line("    srl a");
        }
        self.emit_debug_hex_nibble_from_a();
        self.emit_load_a(byte);
        self.line("    ld bc, 00000Fh");
        self.line("    and c");
        self.emit_debug_hex_nibble_from_a();
    }

    fn emit_debug_hex_nibble_from_a(&mut self) {
        let digit_label = self.next_label("debug_hex_digit");
        let end_label = self.next_label("debug_hex_end");
        self.line("    ld bc, 00000Ah");
        self.line("    cp c");
        self.line(&format!("    jp c, {digit_label}"));
        self.line("    ld bc, 000037h");
        self.line("    add a, c");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{digit_label}:"));
        self.line("    ld bc, 000030h");
        self.line("    add a, c");
        self.line(&format!("{end_label}:"));
        self.emit_out_a(0x0C);
    }

    fn emit_user_call(&mut self, name: &str, args: &[Expr]) -> Result<(), Diagnostic> {
        let sig = self
            .symbols
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if self.current_function_is_interrupt() && !sig.is_interrupt {
            return Err(Diagnostic::new(format!(
                "interrupt function `{}` cannot call non-interrupt function `{name}`",
                self.current_function_name()
            )));
        }
        if sig.arity != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments but got {}",
                sig.arity,
                args.len()
            )));
        }

        let mut temps = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let width = sig.params[index];
            let ty = &sig.param_types[index];
            let temp = self.alloc_var(width.bytes());
            self.emit_expr_to_type(arg, ty)?;
            self.emit_store_width(temp);
            temps.push(temp);
        }

        if let Some(function) = self.symbols.inline_functions.get(name).cloned()
            && !self
                .inline_expansion_stack
                .iter()
                .any(|inline| inline == name)
        {
            self.inline_expansion_stack.push(name.to_owned());
            let inlined = (|| {
                if self.emit_inline_return_call(&function, &temps)? {
                    return Ok(true);
                }
                self.emit_inline_void_call(&function, &temps)
            })();
            self.inline_expansion_stack.pop();
            if inlined? {
                return Ok(());
            }
        }
        let saved_variables = self.recursive_call_saved_variables(name);
        let return_temp = if saved_variables.is_empty() || sig.return_type.is_none() {
            None
        } else {
            Some(self.alloc_var(sig.return_width.bytes()))
        };

        if sig.uses_arg_slots {
            for (temp, slot) in temps.iter().copied().zip(sig.arg_slots.iter().copied()) {
                self.emit_load_width(temp);
                self.emit_store_width(slot);
            }
            self.emit_save_recursive_call_variables(&saved_variables);
            self.line(&format!("    call {}", function_label(name)));
            self.emit_store_recursive_call_return(return_temp);
            self.emit_restore_recursive_call_variables(&saved_variables);
            self.emit_load_recursive_call_return(return_temp);
            return Ok(());
        }

        self.emit_save_recursive_call_variables(&saved_variables);
        if sig.stack_arg_bytes > 0 {
            for temp in temps.iter().copied().skip(3).rev() {
                self.emit_push_stack_arg_variable(temp);
            }
        }
        if let Some(temp) = temps.get(2).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld c, a");
            } else if sig.params.get(1).is_some_and(|width| width.bytes() != 1) {
                self.emit_load_width(temp);
                self.line("    push hl");
                self.line("    pop bc");
            } else {
                return Err(Diagnostic::new(
                    "current codegen supports a wide third argument only when the second argument is also wide",
                ));
            }
        }
        if let Some(temp) = temps.get(1).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld b, a");
            } else {
                self.emit_load_width(temp);
                self.line("    ex de, hl");
            }
        }
        if let Some(temp) = temps.first().copied() {
            self.emit_load_width(temp);
        }
        self.line(&format!("    call {}", function_label(name)));
        if sig.stack_arg_bytes > 0 {
            self.emit_drop_stack_arg_bytes(sig.stack_arg_bytes);
        }
        self.emit_store_recursive_call_return(return_temp);
        self.emit_restore_recursive_call_variables(&saved_variables);
        self.emit_load_recursive_call_return(return_temp);
        Ok(())
    }

    fn recursive_call_saved_variables(&self, callee: &str) -> Vec<Variable> {
        let caller = self.current_function_name();
        if !self
            .recursive_call_edges
            .contains(&(caller.to_owned(), callee.to_owned()))
        {
            return Vec::new();
        }

        let Some(storage) = self.function_storage_stack.last() else {
            return Vec::new();
        };
        let mut variables = storage.clone();
        variables.sort_by_key(|variable| variable.addr);
        variables.dedup_by_key(|variable| variable.addr);
        variables
    }

    fn emit_save_recursive_call_variables(&mut self, variables: &[Variable]) {
        for variable in variables {
            for offset in 0..variable.size {
                self.line(&format!("    ld a, ({:06X}h)", variable.addr + offset));
                self.line("    dec sp");
                self.line("    ld hl, 000000h");
                self.line("    add hl, sp");
                self.line("    ld (hl), a");
            }
        }
    }

    fn emit_restore_recursive_call_variables(&mut self, variables: &[Variable]) {
        for variable in variables.iter().rev() {
            for offset in (0..variable.size).rev() {
                self.line("    ld hl, 000000h");
                self.line("    add hl, sp");
                self.line("    ld a, (hl)");
                self.line("    inc sp");
                self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
            }
        }
    }

    fn emit_store_recursive_call_return(&mut self, return_temp: Option<Variable>) {
        if let Some(return_temp) = return_temp {
            self.emit_store_width(return_temp);
        }
    }

    fn emit_load_recursive_call_return(&mut self, return_temp: Option<Variable>) {
        if let Some(return_temp) = return_temp {
            self.emit_load_width(return_temp);
        }
    }

    fn emit_inline_return_call(
        &mut self,
        function: &Function,
        temps: &[Variable],
    ) -> Result<bool, Diagnostic> {
        let Some((prefix, expr)) = inline_return_body(function) else {
            return Ok(false);
        };
        let Some(return_type) = &function.return_type else {
            return Ok(false);
        };

        self.scopes.push(HashMap::new());
        self.scope_types.push(HashMap::new());
        self.local_constants.push(HashMap::new());
        self.readonly_pointer_aliases.push(HashMap::new());
        self.assigned_names_stack
            .push(assigned_names_in_block(prefix));
        for (param, temp) in function.params.iter().zip(temps.iter().copied()) {
            self.current_scope_mut().insert(param.name.clone(), temp);
            self.current_scope_types_mut()
                .insert(param.name.clone(), param.ty.clone());
        }
        for stmt in prefix {
            self.emit_inline_prefix_stmt(stmt)?;
        }
        let result = self.emit_expr_to_type(&expr, return_type);
        self.assigned_names_stack.pop();
        self.readonly_pointer_aliases.pop();
        self.local_constants.pop();
        self.scope_types.pop();
        self.scopes.pop();
        result?;
        Ok(true)
    }

    fn emit_inline_void_call(
        &mut self,
        function: &Function,
        temps: &[Variable],
    ) -> Result<bool, Diagnostic> {
        let Some(body) = inline_void_body(function) else {
            return Ok(false);
        };

        self.scopes.push(HashMap::new());
        self.scope_types.push(HashMap::new());
        self.local_constants.push(HashMap::new());
        self.readonly_pointer_aliases.push(HashMap::new());
        self.assigned_names_stack
            .push(assigned_names_in_block(body));
        for (param, temp) in function.params.iter().zip(temps.iter().copied()) {
            self.current_scope_mut().insert(param.name.clone(), temp);
            self.current_scope_types_mut()
                .insert(param.name.clone(), param.ty.clone());
        }
        let result = self.emit_block(body);
        self.assigned_names_stack.pop();
        self.readonly_pointer_aliases.pop();
        self.local_constants.pop();
        self.scope_types.pop();
        self.scopes.pop();
        result?;
        Ok(true)
    }

    fn emit_inline_prefix_stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        let Stmt::Let { name, ty, value } = stmt else {
            return self.emit_stmt(stmt);
        };
        if self.current_scope_types_mut().contains_key(name) {
            return Err(Diagnostic::new(format!(
                "local `{name}` shadows an existing name"
            )));
        }
        let variable = self.alloc_storage(ty)?;
        self.current_scope_mut().insert(name.clone(), variable);
        self.current_scope_types_mut()
            .insert(name.clone(), ty.clone());
        self.emit_storage_initializer(variable, ty, value)?;
        self.record_local_constant(name, ty, value);
        self.record_readonly_pointer_alias(name, value);
        Ok(())
    }

    fn emit_expr_to_width(&mut self, expr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        match width {
            ValueWidth::U8 => self.emit_expr_to_a(expr),
            ValueWidth::U16 | ValueWidth::U24 => self.emit_expr_to_hl(expr, width),
        }
    }

    fn emit_expr_to_type(&mut self, expr: &Expr, ty: &Type) -> Result<(), Diagnostic> {
        let width = self.symbols.type_width(ty)?;
        self.validate_expr_arithmetic_compatibility(expr)?;
        self.validate_expr_assignable_to_type(expr, ty)?;
        if let Expr::Cast { expr, ty } = expr {
            self.emit_cast_to_type(expr, ty)?;
            return Ok(());
        }
        if !self.is_pointer_arithmetic_expr(expr)?
            && let Ok(value) = self.eval_i64_with_local_constants(expr)
        {
            let value = self.value_for_type(value, ty, width)?;
            match width {
                ValueWidth::U8 => self.line(&format!("    ld a, {value:02X}h")),
                ValueWidth::U16 | ValueWidth::U24 => self.line(&format!("    ld hl, {value:06X}h")),
            }
            return Ok(());
        }
        self.emit_expr_to_width(expr, width)
    }

    fn is_pointer_arithmetic_expr(&self, expr: &Expr) -> Result<bool, Diagnostic> {
        if let Expr::Binary { left, op, right } = expr {
            return Ok(matches!(op, BinaryOp::Add | BinaryOp::Sub)
                && (self.pointer_pointee_size(left)?.is_some()
                    || self.pointer_pointee_size(right)?.is_some()));
        }
        Ok(false)
    }

    fn emit_cast_to_type(&mut self, expr: &Expr, ty: &Type) -> Result<(), Diagnostic> {
        self.validate_cast(expr, ty)?;
        let width = self.symbols.type_width(ty)?;
        let target_type = self.symbols.resolved_type(ty)?;
        let source_type = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if !self.is_pointer_arithmetic_expr(expr)?
            && let Ok(value) = self.eval_i64_with_local_constants(expr)
        {
            let bits = u32::from(width.bytes()) * 8;
            let mask = (1_i128 << bits) - 1;
            let value = if type_is_bool(&target_type) {
                u32::from(value != 0)
            } else {
                ((value as i128) & mask) as u32
            };
            match width {
                ValueWidth::U8 => self.line(&format!("    ld a, {value:02X}h")),
                ValueWidth::U16 | ValueWidth::U24 => self.line(&format!("    ld hl, {value:06X}h")),
            }
            return Ok(());
        }
        let source_width = self.expr_width(expr)?;
        match width {
            ValueWidth::U8 => {
                if type_is_bool(&target_type) {
                    if source_width == ValueWidth::U8 {
                        self.emit_expr_to_a(expr)?;
                        self.emit_normalize_a_to_bool();
                    } else {
                        self.emit_expr_to_hl(expr, source_width)?;
                        self.emit_normalize_hl_to_bool(source_width);
                    }
                } else if source_width == ValueWidth::U8 {
                    self.emit_expr_to_a(expr)?;
                } else {
                    self.emit_expr_to_hl(expr, source_width)?;
                    self.line("    ld a, l");
                }
            }
            ValueWidth::U16 => {
                if source_width == ValueWidth::U8 {
                    self.emit_expr_to_a(expr)?;
                    self.line("    ld hl, 000000h");
                    self.line("    ld l, a");
                    self.emit_sign_extend_widened_integer(&source_type, source_width, width)?;
                } else {
                    self.emit_expr_to_hl(expr, source_width)?;
                    self.zero_extend_hl16();
                }
            }
            ValueWidth::U24 => {
                if source_width == ValueWidth::U8 {
                    self.emit_expr_to_a(expr)?;
                    self.line("    ld hl, 000000h");
                    self.line("    ld l, a");
                    self.emit_sign_extend_widened_integer(&source_type, source_width, width)?;
                } else {
                    self.emit_expr_to_hl(expr, source_width)?;
                    self.emit_sign_extend_widened_integer(&source_type, source_width, width)?;
                }
            }
        }
        Ok(())
    }

    fn emit_normalize_a_to_bool(&mut self) {
        let true_label = self.next_label("cast_bool_true");
        let end_label = self.next_label("cast_bool_end");
        self.line("    or a");
        self.line(&format!("    jp nz, {true_label}"));
        self.line("    ld a, 00h");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{true_label}:"));
        self.line("    ld a, 01h");
        self.line(&format!("{end_label}:"));
    }

    fn emit_normalize_hl_to_bool(&mut self, width: ValueWidth) {
        let value = self.alloc_var(width.bytes());
        self.emit_store_width(value);
        self.line("    xor a");
        for offset in 0..width.bytes() {
            self.line("    ld b, a");
            self.line(&format!("    ld a, ({:06X}h)", value.addr + offset as u32));
            self.line("    or b");
        }
        self.emit_normalize_a_to_bool();
    }

    fn emit_sign_extend_widened_integer(
        &mut self,
        source_type: &Type,
        source_width: ValueWidth,
        target_width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if source_width >= target_width || !type_is_signed(source_type) {
            return Ok(());
        }
        let (sign_register, extension) = match source_width {
            ValueWidth::U8 => ("l", 0xFFFF00),
            ValueWidth::U16 => ("h", 0xFF0000),
            ValueWidth::U24 => return Ok(()),
        };
        let done = self.next_label("cast_nonnegative");
        self.line(&format!("    ld a, {sign_register}"));
        self.line("    cp 80h");
        self.line(&format!("    jp c, {done}"));
        self.emit_add_hl_const(extension);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn validate_cast(&self, expr: &Expr, target: &Type) -> Result<(), Diagnostic> {
        let source_type = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        let target_type = self.symbols.resolved_type(target)?;
        match (&source_type, &target_type) {
            (Type::Ptr(_), Type::Ptr(_)) => Ok(()),
            (Type::Ptr(_), Type::Named(name)) if name == "bool" => Ok(()),
            (Type::Ptr(_), Type::Named(name))
                if is_raw_address_type(name)
                    || (name == "u16" && self.cpu_uses_16_bit_pointers()) =>
            {
                Ok(())
            }
            (Type::Ptr(_), Type::Named(_)) => Err(Diagnostic::new(
                "pointer-to-integer casts produce u24 or ptr24",
            )),
            (Type::Named(name), Type::Ptr(_)) if is_raw_address_type(name) => Ok(()),
            (Type::Named(_), Type::Ptr(_)) => Err(Diagnostic::new(
                "integer-to-pointer casts require u24 or ptr24",
            )),
            _ => Ok(()),
        }
    }

    fn cpu_uses_16_bit_pointers(&self) -> bool {
        is_z80_family_16bit(self.cpu)
    }

    fn emit_expr_to_hl(&mut self, expr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(value) = self.local_constant_value_for_width(name, width)? {
                    self.line(&format!("    ld hl, {value:06X}h"));
                } else if let Some(variable) = self.variable_opt(name) {
                    if variable.size == 1 {
                        self.emit_load_a(variable);
                        self.line("    ld hl, 000000h");
                        self.line("    ld l, a");
                    } else if variable.size == 2 {
                        self.emit_load_hl16(variable);
                    } else {
                        self.emit_load_hl(variable);
                    }
                } else {
                    let value = self.value_for_width(expr, width)?;
                    self.line(&format!("    ld hl, {:06X}h", value));
                }
            }
            Expr::AddressOfIndex { name, index } => {
                self.emit_array_element_address(name, index)?;
            }
            Expr::AddressOfField { base, field } => {
                self.emit_field_address(base, field)?;
            }
            Expr::AddressOfAccess(path) => {
                let path = self.canonical_access_path(path);
                self.emit_access_address(&path)?;
            }
            Expr::AddressOf(name) => {
                self.emit_variable_address(name)?;
            }
            Expr::String(value) => {
                self.emit_string_literal_address(value)?;
            }
            Expr::Deref(ptr) => {
                self.emit_deref_to_hl(ptr, width)?;
            }
            Expr::Field { base, field } => {
                if self.emit_dotted_constant_to_hl(base, field, width)? {
                    return Ok(());
                }
                if let Some(variable) = self.dotted_variable(base, field) {
                    self.emit_load_width(variable);
                    return Ok(());
                }
                let variable = self.field_variable(base, field)?;
                self.emit_load_width(variable);
            }
            Expr::Index { name, index } => {
                self.emit_load_indexed_element_to_hl(name, index)?;
            }
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty()
                    && (self.named_value_type(&path.root).is_some()
                        || self.symbols.embed_property_value(&path.root).is_some())
                {
                    let value = self.value_for_width(&Expr::Ident(path.root.clone()), width)?;
                    self.line(&format!("    ld hl, {:06X}h", value));
                    return Ok(());
                }
                let ty = self.access_type(&path)?;
                let size = self.symbols.type_size(&ty)?;
                if size > 3 {
                    return Err(Diagnostic::new(format!(
                        "value `{}` is not scalar-sized",
                        access_path_summary(&path)
                    )));
                }
                if let Some(variable) = self.const_access_variable(&path)? {
                    self.emit_load_width(variable);
                    return Ok(());
                }
                self.emit_access_address(&path)?;
                let stored = self.alloc_var(size);
                self.emit_load_pointed_width_into(stored);
                self.emit_load_width(stored);
            }
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Char(_) | Expr::Bool(_) => {
                let value = self.value_for_width(expr, width)?;
                self.line(&format!("    ld hl, {:06X}h", value));
            }
            Expr::Cast { expr, ty } => self.emit_cast_to_type(expr, ty)?,
            Expr::Unary { op, expr } => {
                if *op == UnaryOp::Not {
                    self.ensure_expr_is_bool(expr, "logical operand")?;
                }
                self.emit_unary_to_hl(*op, expr, width)?
            }
            Expr::Binary { left, op, right } => match op {
                BinaryOp::Add | BinaryOp::Sub
                    if self.emit_pointer_arithmetic(left, *op, right)? =>
                {
                    return Ok(());
                }
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor => {
                    self.ensure_binary_arithmetic_operands_compatible(left, right)?;
                    if *op == BinaryOp::Mul {
                        self.emit_mul_to_width(
                            left,
                            right,
                            width,
                            self.binary_operands_are_signed(left, right)?,
                        )?;
                        return Ok(());
                    }
                    self.emit_expr_to_hl(left, width)?;
                    self.line("    push hl");
                    self.emit_expr_to_hl(right, width)?;
                    self.line("    pop bc");
                    self.emit_wide_op_with_left_in_bc(*op, width)?;
                }
                BinaryOp::Shl | BinaryOp::Shr => {
                    self.ensure_shift_operands_compatible(left, right)?;
                    let temp = self.alloc_var(width.bytes());
                    let signed = self.expr_is_signed(left)?;
                    self.emit_expr_to_hl(left, width)?;
                    self.emit_store_width(temp);
                    self.emit_shift_memory_by_expr(temp, *op, right, signed)?;
                    self.emit_load_width(temp);
                }
                BinaryOp::Div | BinaryOp::Mod => {
                    self.ensure_binary_arithmetic_operands_compatible(left, right)?;
                    if self.binary_operands_are_signed(left, right)? {
                        self.emit_signed_div_mod_to_width(left, right, *op, width)?;
                    } else {
                        self.emit_div_mod_to_width(left, right, *op, width)?;
                    }
                    return Ok(());
                }
                _ => {
                    return Err(Diagnostic::new(format!(
                        "binary operator `{op:?}` is not implemented in wide codegen yet"
                    )));
                }
            },
            Expr::Call { path, args } => {
                self.emit_user_call(&path_text(path), args)?;
            }
            Expr::Array(_) | Expr::StructInit { .. } | Expr::In(_) => {
                return Err(Diagnostic::new(format!(
                    "expression `{expr:?}` is not supported in u16 codegen"
                )));
            }
        }
        if width == ValueWidth::U16 {
            self.zero_extend_hl16();
        }
        Ok(())
    }

    fn zero_extend_hl16(&mut self) {
        let temp = self.alloc_var(ValueWidth::U16.bytes());
        self.emit_store_hl16(temp);
        self.emit_load_hl16(temp);
    }

    fn emit_pointer_arithmetic(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<bool, Diagnostic> {
        let left_scale = self.pointer_pointee_size(left)?;
        let right_scale = self.pointer_pointee_size(right)?;
        match (op, left_scale, right_scale) {
            (BinaryOp::Add, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer arithmetic requires exactly one pointer operand",
            )),
            (BinaryOp::Add, Some(scale), None) => {
                self.ensure_pointer_offset_expr(right)?;
                self.emit_expr_to_hl(left, ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(right, scale)?;
                self.line("    pop bc");
                self.line("    add hl, bc");
                Ok(true)
            }
            (BinaryOp::Add, None, Some(scale)) => {
                self.ensure_pointer_offset_expr(left)?;
                self.emit_expr_to_hl(right, ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(left, scale)?;
                self.line("    pop bc");
                self.line("    add hl, bc");
                Ok(true)
            }
            (BinaryOp::Sub, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer subtraction between two pointers is not supported",
            )),
            (BinaryOp::Sub, Some(scale), None) => {
                self.ensure_pointer_offset_expr(right)?;
                self.emit_expr_to_hl(left, ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(right, scale)?;
                self.line("    ex de, hl");
                self.line("    pop hl");
                self.line("    or a");
                self.line("    sbc hl, de");
                Ok(true)
            }
            (BinaryOp::Sub, None, Some(_)) => Err(Diagnostic::new(
                "cannot subtract a pointer from a non-pointer value",
            )),
            _ => Ok(false),
        }
    }

    fn ensure_pointer_offset_expr(&self, expr: &Expr) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if type_is_bool(&ty) || matches!(ty, Type::Ptr(_)) {
            return Err(Diagnostic::new(
                "pointer arithmetic offset must be an integer",
            ));
        }
        self.symbols.type_width(&ty)?;
        Ok(())
    }

    fn emit_scaled_offset_to_hl(&mut self, expr: &Expr, scale: u32) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(expr, ValueWidth::U24)?;
        self.emit_sign_extend_pointer_offset(expr)?;
        match scale {
            1 => {}
            _ => {
                let base = self.alloc_var(ValueWidth::U24.bytes());
                self.emit_store_width(base);
                self.line("    ld hl, 000000h");
                for _ in 0..scale {
                    self.line("    push hl");
                    self.emit_load_width(base);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                }
            }
        }
        Ok(())
    }

    fn emit_sign_extend_pointer_offset(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        if !self.expr_is_signed(expr)? {
            return Ok(());
        }
        let width = self.symbols.type_width(&self.expr_type(expr)?)?;
        let (sign_register, extension) = match width {
            ValueWidth::U8 => ("l", 0xFFFF00),
            ValueWidth::U16 => ("h", 0xFF0000),
            ValueWidth::U24 => return Ok(()),
        };
        let done = self.next_label("offset_nonnegative");
        self.line(&format!("    ld a, {sign_register}"));
        self.line("    cp 80h");
        self.line(&format!("    jp c, {done}"));
        self.emit_add_hl_const(extension);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_wide_op_with_left_in_bc(
        &mut self,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        match op {
            BinaryOp::Add => {
                self.line("    add hl, bc");
            }
            BinaryOp::Sub => {
                self.line("    ex de, hl");
                self.line("    push bc");
                self.line("    pop hl");
                self.line("    or a");
                self.line("    sbc hl, de");
            }
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
                self.emit_wide_bitwise_from_bc_hl(op, width)?;
            }
            _ => unreachable!("unsupported wide op"),
        }
        Ok(())
    }

    fn emit_wide_bitwise_from_bc_hl(
        &mut self,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        let right = self.alloc_var(width.bytes());
        self.emit_store_width(right);
        self.line("    push bc");
        self.line("    pop hl");
        let left = self.alloc_var(width.bytes());
        self.emit_store_width(left);
        let result = self.alloc_var(width.bytes());

        for offset in 0..width.bytes() {
            self.line(&format!("    ld a, ({:06X}h)", left.addr + offset as u32));
            self.line("    ld b, a");
            self.line(&format!("    ld a, ({:06X}h)", right.addr + offset as u32));
            match op {
                BinaryOp::BitAnd => self.line("    and b"),
                BinaryOp::BitOr => self.line("    or b"),
                BinaryOp::BitXor => self.line("    xor b"),
                _ => unreachable!("not a bitwise op"),
            }
            self.line(&format!("    ld ({:06X}h), a", result.addr + offset as u32));
        }

        self.emit_load_width(result);
        Ok(())
    }

    fn emit_expr_to_a(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(value) = self.local_constant_value_for_width(name, ValueWidth::U8)? {
                    self.line(&format!("    ld a, {value:02X}h"));
                } else if let Some(variable) = self.variable_opt(name) {
                    self.emit_load_a(variable);
                } else {
                    let value = self.u8(expr)?;
                    self.line(&format!("    ld a, {:02X}h", value));
                }
            }
            Expr::In(port) => {
                let port = self.port(port)?;
                self.line(&format!("    in0 a, ({port:02X}h)"));
            }
            Expr::Index { name, index } => {
                self.emit_load_indexed_element_to_a(name, index)?;
            }
            Expr::Field { base, field } => {
                if self.emit_dotted_constant_to_a(base, field)? {
                    return Ok(());
                }
                if let Some(variable) = self.dotted_variable(base, field) {
                    if variable.size != 1 {
                        return Err(Diagnostic::new(format!(
                            "value `{base}.{field}` is not u8-sized"
                        )));
                    }
                    self.emit_load_a(variable);
                    return Ok(());
                }
                let variable = self.field_variable(base, field)?;
                if variable.size != 1 {
                    return Err(Diagnostic::new(format!(
                        "field `{base}.{field}` is not u8-sized"
                    )));
                }
                self.emit_load_a(variable);
            }
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty()
                    && (self.named_value_type(&path.root).is_some()
                        || self.symbols.embed_property_value(&path.root).is_some())
                {
                    let value = self.u8(&Expr::Ident(path.root.clone()))?;
                    self.line(&format!("    ld a, {:02X}h", value));
                    return Ok(());
                }
                let ty = self.access_type(&path)?;
                let size = self.symbols.type_size(&ty)?;
                if size != 1 {
                    return Err(Diagnostic::new(format!(
                        "value `{}` is not u8-sized",
                        access_path_summary(&path)
                    )));
                }
                if let Some(variable) = self.const_access_variable(&path)? {
                    self.emit_load_a(variable);
                    return Ok(());
                }
                self.emit_access_address(&path)?;
                self.line("    ld a, (hl)");
            }
            Expr::Deref(ptr) => {
                self.emit_deref_to_a(ptr)?;
            }
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Char(_) | Expr::Bool(_) => {
                let value = self.u8(expr)?;
                self.line(&format!("    ld a, {:02X}h", value));
            }
            Expr::Cast { expr, ty } => self.emit_cast_to_type(expr, ty)?,
            Expr::Unary { op, expr } => {
                if *op == UnaryOp::Not {
                    self.ensure_expr_is_bool(expr, "logical operand")?;
                }
                self.emit_unary_to_a(*op, expr)?
            }
            Expr::Binary { left, op, right } => self.emit_binary_expr(left, *op, right)?,
            Expr::Call { path, args }
                if matches!(path_text(path).as_str(), "mem.peek8" | "ezra.mem.peek8") =>
            {
                self.emit_mem_peek8(args)?;
            }
            Expr::Call { path, args } => {
                self.emit_user_call(&path_text(path), args)?;
            }
            Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_)
            | Expr::AddressOf(_)
            | Expr::Array(_)
            | Expr::StructInit { .. }
            | Expr::String(_) => {
                return Err(Diagnostic::new(format!(
                    "expression `{expr:?}` is not supported in u8 codegen"
                )));
            }
        }
        Ok(())
    }

    fn emit_mem_peek8(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            return Err(Diagnostic::new("mem.peek8 requires one argument"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.line("    ld a, (hl)");
        Ok(())
    }

    fn emit_mem_poke8(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 2 {
            return Err(Diagnostic::new("mem.poke8 requires two arguments"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.validate_expr_assignable_to_type(&args[1], &Type::Named("u8".to_owned()))?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        let value = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(addr);
        self.emit_expr_to_a(&args[1])?;
        self.emit_store_a(value);
        self.emit_load_hl(addr);
        self.emit_load_a(value);
        self.line("    ld (hl), a");
        Ok(())
    }

    fn emit_memcpy(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 3 {
            return Err(Diagnostic::new("mem.memcpy requires three arguments"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.validate_expr_is_ptr_u8(&args[1])?;
        self.validate_expr_assignable_to_type(&args[2], &Type::Named("u24".to_owned()))?;
        let dst = self.alloc_var(ValueWidth::U24.bytes());
        let src = self.alloc_var(ValueWidth::U24.bytes());
        let len = self.alloc_var(ValueWidth::U24.bytes());

        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(dst);
        self.emit_expr_to_hl(&args[1], ValueWidth::U24)?;
        self.emit_store_hl(src);
        self.emit_expr_to_hl(&args[2], ValueWidth::U24)?;
        self.emit_store_hl(len);

        self.emit_load_hl(len);
        self.line("    push hl");
        self.line("    pop bc");
        self.emit_load_hl(src);
        self.line("    ex de, hl");
        self.emit_load_hl(dst);
        self.line("    call __ezra_memcpy");
        Ok(())
    }

    fn emit_memset(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 3 {
            return Err(Diagnostic::new("mem.memset requires three arguments"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.validate_expr_assignable_to_type(&args[1], &Type::Named("u8".to_owned()))?;
        self.validate_expr_assignable_to_type(&args[2], &Type::Named("u24".to_owned()))?;
        let dst = self.alloc_var(ValueWidth::U24.bytes());
        let value = self.alloc_var(ValueWidth::U8.bytes());
        let len = self.alloc_var(ValueWidth::U24.bytes());

        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(dst);
        self.emit_expr_to_a(&args[1])?;
        self.emit_store_a(value);
        self.emit_expr_to_hl(&args[2], ValueWidth::U24)?;
        self.emit_store_hl(len);

        self.emit_load_hl(len);
        self.line("    push hl");
        self.line("    pop bc");
        self.emit_load_a(value);
        self.emit_load_hl(dst);
        self.line("    call __ezra_memset");
        Ok(())
    }

    fn emit_dotted_constant_to_hl(
        &mut self,
        base: &str,
        field: &str,
        width: ValueWidth,
    ) -> Result<bool, Diagnostic> {
        let key = format!("{base}.{field}");
        if !self.symbols.constants.contains_key(&key) {
            return Ok(false);
        }
        let value = self.value_for_width(&Expr::Ident(key), width)?;
        self.line(&format!("    ld hl, {value:06X}h"));
        Ok(true)
    }

    fn emit_dotted_constant_to_a(&mut self, base: &str, field: &str) -> Result<bool, Diagnostic> {
        let key = format!("{base}.{field}");
        if !self.symbols.constants.contains_key(&key) {
            return Ok(false);
        }
        let value = self.u8(&Expr::Ident(key))?;
        self.line(&format!("    ld a, {value:02X}h"));
        Ok(true)
    }

    fn emit_string_literal_address(&mut self, value: &str) -> Result<(), Diagnostic> {
        if let Some(variable) = self.string_literals.get(value).copied() {
            self.line(&format!("    ld hl, {:06X}h", variable.addr));
            return Ok(());
        }

        let variable = self.symbols.intern_string_literal(value)?;
        self.emit_string_literal_initializer(value, variable);
        self.string_literals.insert(value.to_owned(), variable);
        self.line(&format!("    ld hl, {:06X}h", variable.addr));
        Ok(())
    }

    fn emit_deref_to_a(&mut self, ptr: &Expr) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        self.line("    ld a, (hl)");
        Ok(())
    }

    fn emit_deref_to_hl(&mut self, ptr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        match width {
            ValueWidth::U8 => {
                self.line("    ld a, (hl)");
                self.line("    ld hl, 000000h");
                self.line("    ld l, a");
            }
            ValueWidth::U16 | ValueWidth::U24 => {
                let result = self.alloc_var(width.bytes());
                for offset in 0..width.bytes() {
                    if offset != 0 {
                        self.line("    inc hl");
                    }
                    self.line("    ld a, (hl)");
                    self.line(&format!("    ld ({:06X}h), a", result.addr + offset as u32));
                }
                self.emit_load_width(result);
            }
        }
        Ok(())
    }

    fn emit_deref_assignment(
        &mut self,
        ptr: &Expr,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let pointee_type = match self.symbols.resolved_type(&self.expr_type(ptr)?)? {
            Type::Ptr(inner) => *inner,
            Type::Named(name) if name == "ptr24" => {
                return Err(Diagnostic::new(
                    "raw ptr24 dereference requires an explicit typed pointer cast",
                ));
            }
            other => {
                return Err(Diagnostic::new(format!(
                    "cannot assign through non-pointer expression of type `{other:?}`"
                )));
            }
        };
        self.ensure_pointer_write_target_is_mutable(ptr, &pointee_type)?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        self.emit_store_hl(addr);

        if op != AssignOp::Set {
            let width = self.symbols.type_width(&pointee_type)?;
            let current = self.alloc_var(width.bytes());
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.alloc_var(width.bytes());
            let signed = self.type_is_signed(&pointee_type)?;
            self.emit_typed_assignment_value(current, &pointee_type, op, value, signed)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        self.validate_expr_assignable_to_type(value, &pointee_type)?;
        let stored = self.alloc_storage(&pointee_type)?;
        self.emit_storage_initializer(stored, &pointee_type, value)?;
        self.emit_load_hl(addr);
        self.emit_store_var_to_pointed_width(stored);
        Ok(())
    }

    fn ensure_pointer_write_target_is_mutable(
        &mut self,
        ptr: &Expr,
        pointee_type: &Type,
    ) -> Result<(), Diagnostic> {
        let Some(addr) = self.readonly_write_addr(ptr) else {
            return Ok(());
        };
        let size = u64::from(self.symbols.type_size(pointee_type)?);
        let write_start = u64::from(addr);
        let write_end = write_start.saturating_add(size);
        for (name, embed) in &self.symbols.embeds {
            let Some(len) = embed.variable.len else {
                continue;
            };
            if len == 0 {
                continue;
            }
            let embed_start = u64::from(embed.variable.addr);
            let embed_end = embed_start + u64::from(len);
            if write_start < embed_end && write_end > embed_start {
                return Err(Diagnostic::new(format!(
                    "embedded object `{name}` is read-only"
                )));
            }
        }
        if self
            .readonly_string_literal_for_range(write_start, write_end)
            .is_some()
        {
            return Err(Diagnostic::new("string literal is read-only"));
        }
        Ok(())
    }

    fn readonly_write_addr(&mut self, ptr: &Expr) -> Option<u32> {
        if let Some(addr) = self.readonly_expr_addr(ptr) {
            return Some(addr);
        }
        let Ok(addr) = self.symbols.eval_i64(ptr) else {
            return None;
        };
        Self::addr24(addr)
    }

    fn readonly_expr_addr(&mut self, expr: &Expr) -> Option<u32> {
        match expr {
            Expr::Ident(name) => self.readonly_pointer_alias(name).or_else(|| {
                self.symbols
                    .readonly_global_pointer_aliases
                    .get(name)
                    .copied()
            }),
            Expr::String(value) => {
                if let Some(variable) = self
                    .string_literals
                    .get(value)
                    .or_else(|| self.symbols.string_literals.get(value))
                {
                    return Some(variable.addr);
                }
                let variable = self.symbols.intern_string_literal(value).ok()?;
                self.string_literals.insert(value.clone(), variable);
                Some(variable.addr)
            }
            Expr::Cast { expr, .. } => self.readonly_expr_addr(expr),
            Expr::Binary {
                left,
                op: op @ (BinaryOp::Add | BinaryOp::Sub),
                right,
            } => {
                let base = self.readonly_expr_addr(left)?;
                let Type::Ptr(inner) = self
                    .expr_type(left)
                    .ok()
                    .and_then(|ty| self.symbols.resolved_type(&ty).ok())?
                else {
                    return None;
                };
                let offset = self.eval_i64_with_local_constants(right).ok()?;
                let offset = if *op == BinaryOp::Sub {
                    offset.wrapping_neg()
                } else {
                    offset
                };
                let scale = i64::from(self.symbols.type_size(&inner).ok()?);
                Self::addr24(i64::from(base).wrapping_add(offset.wrapping_mul(scale)))
            }
            _ => None,
        }
    }

    fn addr24(addr: i64) -> Option<u32> {
        if (0..=0xFF_FFFF).contains(&addr) {
            Some(addr as u32)
        } else {
            None
        }
    }

    fn emit_binary_expr(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            self.emit_short_circuit_logical(left, op, right)?;
            return Ok(());
        }
        if is_comparison(op) {
            self.ensure_comparison_operands_compatible(left, op, right)?;
            let width = self.expr_width(left)?.max(self.expr_width(right)?);
            if self.binary_operands_are_signed(left, right)? {
                self.emit_signed_comparison(left, op, right, width)?;
                return Ok(());
            }
            if width != ValueWidth::U8 {
                self.emit_wide_comparison(left, op, right, width)?;
                return Ok(());
            }
        }
        if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
            self.ensure_shift_operands_compatible(left, right)?;
            let signed = self.expr_is_signed(left)?;
            self.emit_expr_to_a(left)?;
            self.emit_shift_a_by_expr(op, right, signed)?;
            return Ok(());
        }
        if matches!(op, BinaryOp::Div | BinaryOp::Mod) {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
            if self.binary_operands_are_signed(left, right)? {
                self.emit_signed_div_mod_to_width(left, right, op, ValueWidth::U8)?;
            } else {
                self.emit_u8_div_mod(left, right, op)?;
            }
            return Ok(());
        }
        if op == BinaryOp::Mul {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
            self.emit_mul_to_width(
                left,
                right,
                ValueWidth::U8,
                self.binary_operands_are_signed(left, right)?,
            )?;
            return Ok(());
        }

        if matches!(
            op,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor
        ) {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
        }
        let left_var = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_expr_to_a(left)?;
        self.emit_store_a(left_var);
        self.emit_expr_to_a(right)?;
        self.line("    ld c, a");
        self.emit_load_a(left_var);
        match op {
            BinaryOp::Add => self.line("    add a, c"),
            BinaryOp::Sub => self.line("    sub c"),
            BinaryOp::BitAnd => self.line("    and c"),
            BinaryOp::BitOr => self.line("    or c"),
            BinaryOp::BitXor => self.line("    xor c"),
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => self.emit_comparison(op),
            BinaryOp::And | BinaryOp::Or => unreachable!("logical ops handled before binary load"),
            BinaryOp::Div | BinaryOp::Mod | BinaryOp::Shl | BinaryOp::Shr => {
                return Err(Diagnostic::new(format!(
                    "binary operator `{op:?}` is not implemented in u8 codegen yet"
                )));
            }
            BinaryOp::Mul => unreachable!("multiplication handled before u8 binary dispatch"),
        }
        Ok(())
    }

    fn emit_short_circuit_logical(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        self.ensure_expr_is_bool(left, "logical operand")?;
        self.ensure_expr_is_bool(right, "logical operand")?;
        let short_label = self.next_label("logical_short");
        let end_label = self.next_label("logical_end");

        self.emit_expr_to_a(left)?;
        self.line("    or a");
        match op {
            BinaryOp::And => {
                self.line(&format!("    jp z, {short_label}"));
                self.emit_expr_to_a(right)?;
                self.line("    or a");
                self.line(&format!("    jp z, {short_label}"));
                self.line("    ld a, 01h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{short_label}:"));
                self.line("    ld a, 00h");
            }
            BinaryOp::Or => {
                self.line(&format!("    jp nz, {short_label}"));
                self.emit_expr_to_a(right)?;
                self.line("    or a");
                self.line(&format!("    jp nz, {short_label}"));
                self.line("    ld a, 00h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{short_label}:"));
                self.line("    ld a, 01h");
            }
            _ => unreachable!("not a logical op"),
        }
        self.line(&format!("{end_label}:"));
        Ok(())
    }

    fn emit_wide_comparison(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(left, width)?;
        self.line("    push hl");
        self.emit_expr_to_hl(right, width)?;
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.emit_comparison_from_flags(op);
        Ok(())
    }

    fn emit_signed_comparison(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            if width == ValueWidth::U8 {
                let left_var = self.alloc_var(width.bytes());
                self.emit_expr_to_width(left, width)?;
                self.emit_store_width(left_var);
                self.emit_expr_to_width(right, width)?;
                self.line("    ld c, a");
                self.emit_load_width(left_var);
                self.emit_comparison(op);
                return Ok(());
            }
            return self.emit_wide_comparison(left, op, right, width);
        }

        let left_var = self.alloc_var(width.bytes());
        let right_var = self.alloc_var(width.bytes());
        let same_sign_label = self.next_label("scmp_same_sign");
        let true_label = self.next_label("scmp_true");
        let false_label = self.next_label("scmp_false");
        let end_label = self.next_label("scmp_end");
        let sign_offset = u32::from(width.bytes() - 1);

        self.emit_expr_to_width(left, width)?;
        self.emit_store_width(left_var);
        self.emit_expr_to_width(right, width)?;
        self.emit_store_width(right_var);

        self.line("    ld a, 80h");
        self.line("    ld c, a");
        self.line(&format!("    ld a, ({:06X}h)", left_var.addr + sign_offset));
        self.line("    and c");
        self.line("    ld b, a");
        self.line(&format!(
            "    ld a, ({:06X}h)",
            right_var.addr + sign_offset
        ));
        self.line("    and c");
        self.line("    cp b");
        self.line(&format!("    jp z, {same_sign_label}"));
        self.line("    ld a, b");
        self.line("    or a");
        match op {
            BinaryOp::Lt | BinaryOp::Le => {
                self.line(&format!("    jp nz, {true_label}"));
                self.line(&format!("    jp {false_label}"));
            }
            BinaryOp::Gt | BinaryOp::Ge => {
                self.line(&format!("    jp nz, {false_label}"));
                self.line(&format!("    jp {true_label}"));
            }
            _ => unreachable!("not a signed ordering comparison"),
        }

        self.line(&format!("{same_sign_label}:"));
        if width == ValueWidth::U8 {
            self.emit_load_width(right_var);
            self.line("    ld c, a");
            self.emit_load_width(left_var);
            self.line("    cp c");
        } else {
            self.emit_load_width(left_var);
            self.line("    push hl");
            self.emit_load_width(right_var);
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, de");
        }
        match op {
            BinaryOp::Lt => self.line(&format!("    jp c, {true_label}")),
            BinaryOp::Ge => self.line(&format!("    jp nc, {true_label}")),
            BinaryOp::Le => {
                self.line(&format!("    jp c, {true_label}"));
                self.line(&format!("    jp z, {true_label}"));
            }
            BinaryOp::Gt => {
                self.line(&format!("    jp c, {false_label}"));
                self.line(&format!("    jp z, {false_label}"));
                self.line(&format!("    jp {true_label}"));
            }
            _ => unreachable!("not a signed ordering comparison"),
        }

        self.line(&format!("{false_label}:"));
        self.line("    ld a, 00h");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{true_label}:"));
        self.line("    ld a, 01h");
        self.line(&format!("{end_label}:"));
        Ok(())
    }

    fn emit_u8_div_mod(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: BinaryOp,
    ) -> Result<(), Diagnostic> {
        let left_var = self.alloc_var(1u32);
        self.emit_expr_to_a(left)?;
        self.emit_store_a(left_var);
        self.emit_expr_to_a(right)?;
        self.line("    ld c, a");
        self.emit_load_a(left_var);
        match op {
            BinaryOp::Div => self.line("    call __ezra_div_u8"),
            BinaryOp::Mod => self.line("    call __ezra_mod_u8"),
            _ => unreachable!("not a division op"),
        }
        Ok(())
    }

    fn emit_mul_to_width(
        &mut self,
        left: &Expr,
        right: &Expr,
        width: ValueWidth,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U8 {
            let left_var = self.alloc_var(1u32);
            self.emit_expr_to_a(left)?;
            self.emit_store_a(left_var);
            self.emit_expr_to_a(right)?;
            self.line("    ld c, a");
            self.emit_load_a(left_var);
            self.line("    call __ezra_mul_u8");
            return Ok(());
        }
        if width == ValueWidth::U16 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            self.line("    call __ezra_mul_u16");
            return Ok(());
        }
        if width == ValueWidth::U24 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            if signed {
                self.line("    call __ezra_mul_i24");
            } else {
                self.line("    call __ezra_mul_u24");
            }
            return Ok(());
        }

        let left_var = self.alloc_var(width.bytes());
        let counter = self.alloc_var(width.bytes());
        let result = self.alloc_var(width.bytes());
        let loop_label = self.next_label("mul_loop");
        let done_label = self.next_label("mul_done");

        self.emit_expr_to_width(left, width)?;
        self.emit_store_width(left_var);
        self.emit_expr_to_width(right, width)?;
        self.emit_store_width(counter);
        match width {
            ValueWidth::U8 => self.line("    xor a"),
            ValueWidth::U16 | ValueWidth::U24 => self.line("    ld hl, 000000h"),
        }
        self.emit_store_width(result);

        self.line(&format!("{loop_label}:"));
        self.emit_jump_if_memory_zero(counter, &done_label);
        if width == ValueWidth::U8 {
            self.emit_load_a(result);
            self.line("    ld b, a");
            self.emit_load_a(left_var);
            self.line("    add a, b");
            self.emit_store_a(result);
        } else {
            self.emit_load_width(result);
            self.line("    push hl");
            self.emit_load_width(left_var);
            self.line("    pop bc");
            self.emit_wide_op_with_left_in_bc(BinaryOp::Add, width)?;
            self.emit_store_width(result);
        }
        self.emit_decrement_memory(counter);
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done_label}:"));
        self.emit_load_width(result);
        Ok(())
    }

    fn emit_div_mod_to_width(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U16 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_u16"),
                BinaryOp::Mod => self.line("    call __ezra_mod_u16"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }
        if width == ValueWidth::U24 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_u24"),
                BinaryOp::Mod => self.line("    call __ezra_mod_u24"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }

        let dividend = self.alloc_var(width.bytes());
        let divisor = self.alloc_var(width.bytes());
        let quotient = self.alloc_var(width.bytes());
        let loop_label = self.next_label("div_loop");
        let zero_label = self.next_label("div_zero");
        let done_label = self.next_label("div_done");

        self.emit_expr_to_hl(left, width)?;
        self.emit_store_width(dividend);
        self.emit_expr_to_hl(right, width)?;
        self.emit_store_width(divisor);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_zero_variable(quotient);

        self.line(&format!("{loop_label}:"));
        self.emit_load_width(dividend);
        self.line("    push hl");
        self.emit_load_width(divisor);
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line(&format!("    jp c, {done_label}"));
        self.emit_store_width(dividend);
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        Ok(())
    }

    fn emit_signed_div_mod_to_width(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U24 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_i24"),
                BinaryOp::Mod => self.line("    call __ezra_mod_i24"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }

        let dividend = self.alloc_var(width.bytes());
        let divisor = self.alloc_var(width.bytes());
        let quotient = self.alloc_var(width.bytes());
        let quotient_negative = self.alloc_var(ValueWidth::U8.bytes());
        let remainder_negative = self.alloc_var(ValueWidth::U8.bytes());
        let loop_label = self.next_label("sdiv_loop");
        let zero_label = self.next_label("sdiv_zero");
        let done_label = self.next_label("sdiv_done");
        let quotient_positive_label = self.next_label("sdiv_q_positive");
        let remainder_positive_label = self.next_label("sdiv_r_positive");
        let not_overflow_label = self.next_label("sdiv_not_overflow");
        let finished_label = self.next_label("sdiv_finished");

        self.emit_expr_to_width(left, width)?;
        self.emit_store_width(dividend);
        self.emit_expr_to_width(right, width)?;
        self.emit_store_width(divisor);
        self.emit_zero_variable(quotient);
        self.emit_zero_variable(quotient_negative);
        self.emit_zero_variable(remainder_negative);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_jump_if_memory_not_equals(dividend, signed_min_bytes(width), &not_overflow_label);
        self.emit_jump_if_memory_not_equals(
            divisor,
            signed_negative_one_bytes(width),
            &not_overflow_label,
        );
        match op {
            BinaryOp::Div => self.emit_load_width(dividend),
            BinaryOp::Mod => {
                self.emit_zero_variable(dividend);
                self.emit_load_width(dividend);
            }
            _ => unreachable!("not a division op"),
        }
        self.line(&format!("    jp {finished_label}"));
        self.line(&format!("{not_overflow_label}:"));

        self.emit_abs_signed_variable(dividend, Some(quotient_negative), Some(remainder_negative));
        self.emit_abs_signed_variable(divisor, Some(quotient_negative), None);

        self.line(&format!("{loop_label}:"));
        if width == ValueWidth::U8 {
            self.emit_load_a(dividend);
            self.line("    ld b, a");
            self.emit_load_a(divisor);
            self.line("    ld c, a");
            self.line("    ld a, b");
            self.line("    cp c");
            self.line(&format!("    jp c, {done_label}"));
            self.line("    sub c");
            self.emit_store_a(dividend);
        } else {
            self.emit_load_width(dividend);
            self.line("    push hl");
            self.emit_load_width(divisor);
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, de");
            self.line(&format!("    jp c, {done_label}"));
            self.emit_store_width(dividend);
        }
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        self.emit_load_a(quotient_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {quotient_positive_label}"));
        self.emit_negate_memory(quotient);
        self.line(&format!("{quotient_positive_label}:"));
        self.emit_load_a(remainder_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {remainder_positive_label}"));
        self.emit_negate_memory(dividend);
        self.line(&format!("{remainder_positive_label}:"));

        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        self.line(&format!("{finished_label}:"));
        Ok(())
    }

    fn emit_abs_signed_variable(
        &mut self,
        variable: Variable,
        quotient_negative: Option<Variable>,
        remainder_negative: Option<Variable>,
    ) {
        let nonnegative_label = self.next_label("signed_nonnegative");
        let sign_addr = variable.addr + variable.size - 1;
        self.line(&format!("    ld a, ({sign_addr:06X}h)"));
        self.line("    ld b, a");
        self.line("    ld a, 7Fh");
        self.line("    cp b");
        self.line(&format!("    jp nc, {nonnegative_label}"));
        self.emit_negate_memory(variable);
        if let Some(flag) = quotient_negative {
            self.emit_toggle_u8(flag);
        }
        if let Some(flag) = remainder_negative {
            self.emit_toggle_u8(flag);
        }
        self.line(&format!("{nonnegative_label}:"));
    }

    fn emit_negate_memory(&mut self, variable: Variable) {
        for offset in 0..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    xor FFh");
            self.line(&format!("    ld ({addr:06X}h), a"));
        }
        self.emit_increment_memory(variable);
    }

    fn emit_toggle_u8(&mut self, variable: Variable) {
        self.emit_load_a(variable);
        self.line("    xor 01h");
        self.emit_store_a(variable);
    }

    fn emit_jump_if_memory_zero(&mut self, variable: Variable, zero_label: &str) {
        let nonzero_label = self.next_label("nonzero");
        for offset in 0..variable.size {
            self.line(&format!("    ld a, ({:06X}h)", variable.addr + offset));
            self.line("    or a");
            self.line(&format!("    jp nz, {nonzero_label}"));
        }
        self.line(&format!("    jp {zero_label}"));
        self.line(&format!("{nonzero_label}:"));
    }

    fn emit_jump_if_memory_not_equals(&mut self, variable: Variable, bytes: &[u8], label: &str) {
        for (offset, byte) in bytes.iter().copied().enumerate() {
            self.line(&format!(
                "    ld a, ({:06X}h)",
                variable.addr + offset as u32
            ));
            self.line("    ld b, a");
            self.line(&format!("    ld a, {byte:02X}h"));
            self.line("    cp b");
            self.line(&format!("    jp nz, {label}"));
        }
    }

    fn emit_zero_variable(&mut self, variable: Variable) {
        match variable.size {
            1 => self.line("    xor a"),
            2 | 3 => self.line("    ld hl, 000000h"),
            _ => unreachable!("unsupported variable size {}", variable.size),
        }
        self.emit_store_width(variable);
    }

    fn emit_zero_storage(&mut self, variable: Variable) {
        self.line("    xor a");
        for offset in 0..variable.size {
            self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
        }
    }

    fn emit_increment_memory(&mut self, variable: Variable) {
        let done_label = self.next_label("inc_done");
        for offset in 0..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    ld b, a");
            self.line("    ld a, 01h");
            self.line("    add a, b");
            self.line(&format!("    ld ({addr:06X}h), a"));
            self.line("    or a");
            self.line(&format!("    jp nz, {done_label}"));
        }
        self.line(&format!("{done_label}:"));
    }

    fn emit_decrement_memory(&mut self, variable: Variable) {
        let done_label = self.next_label("dec_done");
        for offset in 0..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    ld b, a");
            self.line("    ld a, 01h");
            self.line("    ld c, a");
            self.line("    ld a, b");
            self.line("    sub c");
            self.line(&format!("    ld ({addr:06X}h), a"));
            self.line("    ld a, b");
            self.line("    or a");
            self.line(&format!("    jp nz, {done_label}"));
        }
        self.line(&format!("{done_label}:"));
    }

    fn emit_shift_a(&mut self, op: BinaryOp, count: u8, signed: bool) -> Result<(), Diagnostic> {
        for _ in 0..count {
            match op {
                BinaryOp::Shl => self.line("    add a, a"),
                BinaryOp::Shr if signed => self.line("    sra a"),
                BinaryOp::Shr => self.line("    srl a"),
                _ => unreachable!("not a shift op"),
            }
        }
        Ok(())
    }

    fn emit_shift_a_by_expr(
        &mut self,
        op: BinaryOp,
        count: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if let Some(count) = self.maybe_const_shift_count(count)? {
            return self.emit_shift_a(op, count, signed);
        }
        let temp = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_store_a(temp);
        self.emit_expr_to_a(count)?;
        self.line("    ld b, a");
        self.emit_shift_memory_dynamic(temp, op, signed)?;
        self.emit_load_a(temp);
        Ok(())
    }

    fn emit_shift_memory(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        count: u8,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        for _ in 0..count {
            match op {
                BinaryOp::Shl => self.emit_shift_memory_left_once(variable),
                BinaryOp::Shr => self.emit_shift_memory_right_once(variable, signed),
                _ => unreachable!("not a shift op"),
            }
        }
        Ok(())
    }

    fn emit_shift_memory_by_expr(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        count: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if let Some(count) = self.maybe_const_shift_count(count)? {
            return self.emit_shift_memory(variable, op, count, signed);
        }
        self.emit_expr_to_a(count)?;
        self.line("    ld b, a");
        self.emit_shift_memory_dynamic(variable, op, signed)
    }

    fn emit_shift_memory_dynamic(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        let loop_label = self.next_label("shift_loop");
        let done_label = self.next_label("shift_done");
        self.line(&format!("{loop_label}:"));
        self.line("    ld a, b");
        self.line("    or a");
        self.line(&format!("    jp z, {done_label}"));
        match op {
            BinaryOp::Shl => self.emit_shift_memory_left_once(variable),
            BinaryOp::Shr => self.emit_shift_memory_right_once(variable, signed),
            _ => unreachable!("not a shift op"),
        }
        self.line("    dec b");
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done_label}:"));
        Ok(())
    }

    fn emit_shift_memory_left_once(&mut self, variable: Variable) {
        self.line(&format!("    ld a, ({:06X}h)", variable.addr));
        self.line("    add a, a");
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
        for offset in 1..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    rl a");
            self.line(&format!("    ld ({addr:06X}h), a"));
        }
    }

    fn emit_shift_memory_right_once(&mut self, variable: Variable, signed: bool) {
        for offset in (0..variable.size).rev() {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            if offset == variable.size - 1 {
                if signed {
                    self.line("    sra a");
                } else {
                    self.line("    srl a");
                }
            } else {
                self.line("    rr a");
            }
            self.line(&format!("    ld ({addr:06X}h), a"));
        }
    }

    fn emit_unary_to_a(&mut self, op: UnaryOp, expr: &Expr) -> Result<(), Diagnostic> {
        match op {
            UnaryOp::Neg => {
                self.emit_expr_to_a(expr)?;
                self.line("    ld b, a");
                self.line("    xor a");
                self.line("    sub b");
            }
            UnaryOp::BitNot => {
                self.emit_expr_to_a(expr)?;
                self.line("    xor FFh");
            }
            UnaryOp::Not => {
                let true_label = self.next_label("not_true");
                let end_label = self.next_label("not_end");
                self.emit_expr_to_a(expr)?;
                self.line("    or a");
                self.line(&format!("    jp z, {true_label}"));
                self.line("    ld a, 00h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{true_label}:"));
                self.line("    ld a, 01h");
                self.line(&format!("{end_label}:"));
            }
        }
        Ok(())
    }

    fn emit_unary_to_hl(
        &mut self,
        op: UnaryOp,
        expr: &Expr,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        match op {
            UnaryOp::Neg => {
                self.emit_expr_to_hl(expr, width)?;
                self.line("    push hl");
                self.line("    ld hl, 000000h");
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
            }
            UnaryOp::BitNot => {
                self.emit_expr_to_hl(expr, width)?;
                let value = self.alloc_var(width.bytes());
                self.emit_store_width(value);
                let result = self.alloc_var(width.bytes());
                for offset in 0..width.bytes() {
                    self.line(&format!("    ld a, ({:06X}h)", value.addr + offset as u32));
                    self.line("    xor FFh");
                    self.line(&format!("    ld ({:06X}h), a", result.addr + offset as u32));
                }
                self.emit_load_width(result);
            }
            UnaryOp::Not => {
                let true_label = self.next_label("not_true");
                let end_label = self.next_label("not_end");
                self.emit_expr_to_hl(expr, width)?;
                self.line("    push hl");
                self.line("    ld hl, 000000h");
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {true_label}"));
                self.line("    ld hl, 000000h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{true_label}:"));
                self.line("    ld hl, 000001h");
                self.line(&format!("{end_label}:"));
            }
        }
        Ok(())
    }

    fn emit_comparison(&mut self, op: BinaryOp) {
        self.line("    cp c");
        self.emit_comparison_from_flags(op);
    }

    fn emit_comparison_from_flags(&mut self, op: BinaryOp) {
        let true_label = self.next_label("cmp_true");
        let end_label = self.next_label("cmp_end");
        let false_label = self.next_label("cmp_false");
        match op {
            BinaryOp::Eq => self.line(&format!("    jp z, {true_label}")),
            BinaryOp::Ne => self.line(&format!("    jp nz, {true_label}")),
            BinaryOp::Lt => self.line(&format!("    jp c, {true_label}")),
            BinaryOp::Ge => self.line(&format!("    jp nc, {true_label}")),
            BinaryOp::Le => {
                self.line(&format!("    jp c, {true_label}"));
                self.line(&format!("    jp z, {true_label}"));
            }
            BinaryOp::Gt => {
                self.line(&format!("    jp c, {false_label}"));
                self.line(&format!("    jp z, {false_label}"));
                self.line(&format!("    jp {true_label}"));
            }
            _ => unreachable!("not a comparison"),
        }
        self.line(&format!("{false_label}:"));
        self.line("    ld a, 00h");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{true_label}:"));
        self.line("    ld a, 01h");
        self.line(&format!("{end_label}:"));
    }

    fn emit_out(&mut self, port: u8, value: u8) {
        self.line(&format!("    ld a, {:02X}h", value));
        self.emit_out_a(port);
    }

    fn emit_out_a(&mut self, port: u8) {
        if is_z80_family_16bit(self.cpu) {
            self.line(&format!("    out ({:02X}h), a", port));
        } else {
            self.line(&format!("    out0 ({:02X}h), a", port));
        }
    }

    fn emit_load_a(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 1);
        self.line(&format!("    ld a, ({:06X}h)", variable.addr));
    }

    fn emit_store_a(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 1);
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
    }

    fn emit_load_hl(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 3);
        self.line(&format!("    ld hl, ({:06X}h)", variable.addr));
    }

    fn emit_store_hl(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 3);
        self.line(&format!("    ld ({:06X}h), hl", variable.addr));
    }

    fn emit_load_hl16(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 2);
        self.line("    ld hl, 000000h");
        self.line(&format!("    ld a, ({:06X}h)", variable.addr));
        self.line("    ld l, a");
        self.line(&format!("    ld a, ({:06X}h)", variable.addr + 1));
        self.line("    ld h, a");
    }

    fn emit_store_hl16(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 2);
        self.line("    ld a, l");
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
        self.line("    ld a, h");
        self.line(&format!("    ld ({:06X}h), a", variable.addr + 1));
    }

    fn emit_load_width(&mut self, variable: Variable) {
        match variable.size {
            1 => self.emit_load_a(variable),
            2 => self.emit_load_hl16(variable),
            3 => self.emit_load_hl(variable),
            _ => unreachable!("unsupported variable size {}", variable.size),
        }
    }

    fn emit_store_width(&mut self, variable: Variable) {
        match variable.size {
            1 => self.emit_store_a(variable),
            2 => self.emit_store_hl16(variable),
            3 => self.emit_store_hl(variable),
            _ => unreachable!("unsupported variable size {}", variable.size),
        }
    }

    fn emit_load_ix_offset_width_into(
        &mut self,
        offset: u8,
        variable: Variable,
    ) -> Result<(), Diagnostic> {
        for byte_offset in 0..variable.size {
            let displacement = offset as u32 + byte_offset;
            if displacement > 0x7F {
                return Err(Diagnostic::new(format!(
                    "stack argument offset {displacement} exceeds IX displacement range"
                )));
            }
            self.line(&format!("    ld a, (ix+{displacement})"));
            self.line(&format!("    ld ({:06X}h), a", variable.addr + byte_offset));
        }
        Ok(())
    }

    fn emit_push_stack_arg_variable(&mut self, variable: Variable) {
        for byte_offset in (0..variable.size).rev() {
            self.line(&format!("    ld a, ({:06X}h)", variable.addr + byte_offset));
            self.line("    dec sp");
            self.line("    ld hl, 000000h");
            self.line("    add hl, sp");
            self.line("    ld (hl), a");
        }
    }

    fn emit_drop_stack_arg_bytes(&mut self, bytes: u8) {
        for _ in 0..bytes {
            self.line("    inc sp");
        }
    }

    fn emit_load_pointed_width_into(&mut self, variable: Variable) {
        for offset in 0..variable.size {
            if offset != 0 {
                self.line("    inc hl");
            }
            self.line("    ld a, (hl)");
            self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
        }
    }

    fn emit_store_var_to_pointed_width(&mut self, variable: Variable) {
        for offset in 0..variable.size {
            if offset != 0 {
                self.line("    inc hl");
            }
            self.line(&format!("    ld a, ({:06X}h)", variable.addr + offset));
            self.line("    ld (hl), a");
        }
    }

    fn emit_copy_storage_into(
        &mut self,
        source: Variable,
        target: Variable,
    ) -> Result<(), Diagnostic> {
        if source.size != target.size {
            return Err(Diagnostic::new("type mismatch"));
        }
        if storage_ranges_overlap(source, target) {
            let temp = self.alloc_var(source.size);
            self.emit_copy_storage_bytes(source, temp);
            self.emit_copy_storage_bytes(temp, target);
        } else {
            self.emit_copy_storage_bytes(source, target);
        }
        Ok(())
    }

    fn emit_copy_storage_bytes(&mut self, source: Variable, target: Variable) {
        for offset in 0..source.size {
            self.line(&format!("    ld a, ({:06X}h)", source.addr + offset));
            self.line(&format!("    ld ({:06X}h), a", target.addr + offset));
        }
    }

    fn emit_copy_pointed_storage_into(
        &mut self,
        ptr: &Expr,
        variable: Variable,
    ) -> Result<(), Diagnostic> {
        let temp = self.alloc_var(variable.size);
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        self.emit_load_pointed_width_into(temp);
        self.emit_copy_storage_bytes(temp, variable);
        Ok(())
    }

    fn expr_storage_variable(&self, expr: &Expr) -> Result<Option<Variable>, Diagnostic> {
        match expr {
            Expr::Ident(name) => Ok(self.variable_opt(name)),
            Expr::Field { base, field } => {
                if let Some(variable) = self.dotted_variable(base, field) {
                    return Ok(Some(variable));
                }
                if self.variable_opt(base).is_some() {
                    self.field_variable(base, field).map(Some)
                } else {
                    Ok(None)
                }
            }
            Expr::Index { name, index } => self.const_array_element_variable(name, index),
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty() {
                    return Ok(self.variable_opt(&path.root));
                }
                if self.variable_opt(&path.root).is_some() {
                    self.const_access_variable(&path)
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn const_array_element_variable(
        &self,
        name: &str,
        index: &Expr,
    ) -> Result<Option<Variable>, Diagnostic> {
        self.validate_array_index_type(index)?;
        let (array, element_size, len) = self.array_info(name)?;
        let index_value = match self.symbols.eval_i64(index) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        if index_value < 0 || index_value as u32 >= len {
            return Err(Diagnostic::new(format!(
                "array index {index_value} is out of bounds for `{name}` length {len}"
            )));
        }
        let element_type = self.array_element_type(name)?;
        self.symbols
            .storage_at(
                array.addr + index_value as u32 * element_size,
                &element_type,
            )
            .map(Some)
    }

    fn array_info(&self, name: &str) -> Result<(Variable, u32, u32), Diagnostic> {
        let array = self.variable(name)?;
        let element_size = array
            .element_size
            .ok_or_else(|| Diagnostic::new(format!("`{name}` is not an array")))?;
        let len = array
            .len
            .ok_or_else(|| Diagnostic::new(format!("array `{name}` is missing length")))?;
        Ok((array, element_size, len))
    }

    fn array_element_width(&self, name: &str) -> Result<ValueWidth, Diagnostic> {
        let (_, element_size, _) = self.array_info(name)?;
        scalar_var(0, element_size).width()
    }

    fn emit_variable_address(&mut self, name: &str) -> Result<(), Diagnostic> {
        let variable = self
            .symbols
            .embeds
            .get(name)
            .map(|embed| embed.variable)
            .or_else(|| self.variable_opt(name))
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{name}`")))?;
        self.line(&format!("    ld hl, {:06X}h", variable.addr));
        Ok(())
    }

    fn emit_field_address(&mut self, base: &str, field: &str) -> Result<(), Diagnostic> {
        let variable = self.field_variable(base, field)?;
        self.line(&format!("    ld hl, {:06X}h", variable.addr));
        Ok(())
    }

    fn struct_type_name(&self, ty: &Type) -> Result<String, Diagnostic> {
        match self.symbols.resolved_type(ty)? {
            Type::Named(name) if self.symbols.structs.contains_key(&name) => Ok(name),
            other => Err(Diagnostic::new(format!(
                "type `{other:?}` is not a struct type"
            ))),
        }
    }

    fn field_variable(&self, base: &str, field: &str) -> Result<Variable, Diagnostic> {
        if let Some(variable) = self.dotted_variable(base, field) {
            return Ok(variable);
        }
        let base_variable = self.variable(base)?;
        let base_type = self
            .variable_type(base)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{base}`")))?;
        let struct_name = self.struct_type_name(base_type)?;
        let layout = self
            .symbols
            .structs
            .get(&struct_name)
            .ok_or_else(|| Diagnostic::new(format!("unknown struct `{struct_name}`")))?;
        let field = layout.fields.get(field).ok_or_else(|| {
            Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
        })?;
        self.symbols
            .storage_at(base_variable.addr + field.offset, &field.ty)
    }

    fn field_type(&self, base: &str, field: &str) -> Result<Type, Diagnostic> {
        let key = format!("{base}.{field}");
        if let Some(ty) = self.named_value_type(&key) {
            return Ok(ty.clone());
        }
        let base_type = self
            .variable_type(base)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{base}`")))?;
        let struct_name = self.struct_type_name(base_type)?;
        let layout = self
            .symbols
            .structs
            .get(&struct_name)
            .ok_or_else(|| Diagnostic::new(format!("unknown struct `{struct_name}`")))?;
        layout
            .fields
            .get(field)
            .map(|field| field.ty.clone())
            .ok_or_else(|| {
                Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
            })
    }

    fn access_type(&self, path: &AccessPath) -> Result<Type, Diagnostic> {
        let mut ty = self
            .variable_type(&path.root)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?
            .clone();
        for segment in &path.segments {
            ty = match segment {
                AccessSegment::Field(field) => {
                    let struct_name = self.struct_type_name(&ty)?;
                    let layout = self.symbols.structs.get(&struct_name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown struct `{struct_name}`"))
                    })?;
                    layout
                        .fields
                        .get(field)
                        .map(|field| field.ty.clone())
                        .ok_or_else(|| {
                            Diagnostic::new(format!(
                                "struct `{struct_name}` has no field `{field}`"
                            ))
                        })?
                }
                AccessSegment::Index(index) => {
                    self.validate_array_index_type(index)?;
                    match self.symbols.resolved_type(&ty)? {
                        Type::Array { element, len } => {
                            self.validate_const_access_index_bounds(
                                index,
                                &len,
                                &access_path_summary(path),
                            )?;
                            *element
                        }
                        _ => {
                            return Err(Diagnostic::new(format!(
                                "value `{}` is not an array",
                                access_path_summary(path)
                            )));
                        }
                    }
                }
            };
        }
        Ok(ty)
    }

    fn const_access_variable(&self, path: &AccessPath) -> Result<Option<Variable>, Diagnostic> {
        let mut variable = self.variable(&path.root)?;
        let mut ty = self
            .variable_type(&path.root)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?
            .clone();

        for segment in &path.segments {
            match segment {
                AccessSegment::Field(field) => {
                    let struct_name = self.struct_type_name(&ty)?;
                    let layout = self.symbols.structs.get(&struct_name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown struct `{struct_name}`"))
                    })?;
                    let field_info = layout.fields.get(field).ok_or_else(|| {
                        Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
                    })?;
                    variable = self
                        .symbols
                        .storage_at(variable.addr + field_info.offset, &field_info.ty)?;
                    ty = field_info.ty.clone();
                }
                AccessSegment::Index(index) => {
                    self.validate_array_index_type(index)?;
                    let index_value = match self.symbols.eval_i64(index) {
                        Ok(value) => value,
                        Err(_) => return Ok(None),
                    };
                    let Type::Array { element, len } = self.symbols.resolved_type(&ty)? else {
                        return Err(Diagnostic::new(format!(
                            "value `{}` is not an array",
                            access_path_summary(path)
                        )));
                    };
                    let len = self.symbols.array_len(&len)?;
                    if index_value < 0 || index_value as u32 >= len {
                        return Err(Diagnostic::new(format!(
                            "array index {index_value} is out of bounds for `{}` length {len}",
                            access_path_summary(path)
                        )));
                    }
                    let element_size = self.symbols.type_size(&element)?;
                    variable = self
                        .symbols
                        .storage_at(variable.addr + index_value as u32 * element_size, &element)?;
                    ty = *element;
                }
            }
        }

        Ok(Some(variable))
    }

    fn emit_access_address(&mut self, path: &AccessPath) -> Result<(), Diagnostic> {
        let path = self.canonical_access_path(path);
        let path = &path;
        if path.segments.is_empty()
            && (self.named_value_type(&path.root).is_some()
                || self.symbols.embed_property_value(&path.root).is_some())
        {
            let value = self.value_for_width(&Expr::Ident(path.root.clone()), ValueWidth::U24)?;
            self.line(&format!("    ld hl, {value:06X}h"));
            return Ok(());
        }
        if let Some(variable) = self.const_access_variable(path)? {
            self.line(&format!("    ld hl, {:06X}h", variable.addr));
            return Ok(());
        }

        let root = self.variable(&path.root)?;
        let mut ty = self
            .variable_type(&path.root)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?
            .clone();
        self.line(&format!("    ld hl, {:06X}h", root.addr));

        for segment in &path.segments {
            match segment {
                AccessSegment::Field(field) => {
                    let struct_name = self.struct_type_name(&ty)?;
                    let layout = self.symbols.structs.get(&struct_name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown struct `{struct_name}`"))
                    })?;
                    let field_info = layout.fields.get(field).ok_or_else(|| {
                        Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
                    })?;
                    let offset = field_info.offset;
                    let field_ty = field_info.ty.clone();
                    self.emit_add_hl_const(offset);
                    ty = field_ty;
                }
                AccessSegment::Index(index) => {
                    self.validate_array_index_type(index)?;
                    let Type::Array { element, len } = self.symbols.resolved_type(&ty)? else {
                        return Err(Diagnostic::new(format!(
                            "value `{}` is not an array",
                            access_path_summary(path)
                        )));
                    };
                    self.validate_const_access_index_bounds(
                        index,
                        &len,
                        &access_path_summary(path),
                    )?;
                    let element_size = self.symbols.type_size(&element)?;
                    let base_addr = self.alloc_var(ValueWidth::U24.bytes());
                    self.emit_store_hl(base_addr);
                    self.emit_expr_to_hl(index, ValueWidth::U24)?;
                    self.emit_scale_hl_by(element_size);
                    self.line("    push hl");
                    self.emit_load_hl(base_addr);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                    ty = *element;
                }
            }
        }
        Ok(())
    }

    fn emit_add_hl_const(&mut self, value: u32) {
        if value == 0 {
            return;
        }
        self.line("    push hl");
        self.line(&format!("    ld bc, {:06X}h", value & 0xFF_FFFF));
        self.line("    pop hl");
        self.line("    add hl, bc");
    }

    fn emit_scale_hl_by(&mut self, factor: u32) {
        match factor {
            0 | 1 => {}
            2 => self.line("    add hl, hl"),
            3 => {
                self.line("    push hl");
                self.line("    add hl, hl");
                self.line("    pop bc");
                self.line("    add hl, bc");
            }
            _ => {
                let index_value = self.alloc_var(ValueWidth::U24.bytes());
                self.emit_store_hl(index_value);
                for _ in 1..factor {
                    self.line("    push hl");
                    self.emit_load_hl(index_value);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                }
            }
        }
    }

    fn array_element_type(&self, name: &str) -> Result<Type, Diagnostic> {
        let Some(ty) = self.variable_type(name) else {
            return Err(Diagnostic::new(format!("unknown array `{name}`")));
        };
        match self.symbols.resolved_type(ty)? {
            Type::Array { element, .. } => Ok(*element),
            _ => Err(Diagnostic::new(format!("`{name}` is not an array"))),
        }
    }

    fn validate_array_index_type(&self, index: &Expr) -> Result<(), Diagnostic> {
        if expr_is_untyped_literal(index) {
            let value = self.symbols.eval_i64(index)?;
            if !(0..=0xFF_FFFF).contains(&value) {
                return Err(Diagnostic::new(format!(
                    "array index value {value} is outside u24 range"
                )));
            }
            return Ok(());
        }

        let ty = self.symbols.resolved_type(&self.expr_type(index)?)?;
        if matches!(&ty, Type::Named(name) if matches!(name.as_str(), "u8" | "u16" | "u24")) {
            Ok(())
        } else {
            Err(Diagnostic::new(format!(
                "array index type `{}` is not supported; use u8, u16, or u24",
                type_display(&ty)
            )))
        }
    }

    fn validate_const_access_index_bounds(
        &self,
        index: &Expr,
        len: &Expr,
        path: &str,
    ) -> Result<(), Diagnostic> {
        let len = self.symbols.array_len(len)?;
        if let Ok(index_value) = self.symbols.eval_i64(index)
            && (index_value < 0 || index_value as u32 >= len)
        {
            return Err(Diagnostic::new(format!(
                "array index {index_value} is out of bounds for `{path}` length {len}",
            )));
        }
        Ok(())
    }

    fn pointer_pointee_size(&self, expr: &Expr) -> Result<Option<u32>, Diagnostic> {
        match self.expr_type(expr) {
            Ok(ty) => match self.symbols.resolved_type(&ty)? {
                Type::Ptr(inner) => Ok(Some(self.symbols.type_size(&inner)?)),
                _ => Ok(None),
            },
            Err(_) => Ok(None),
        }
    }

    fn emit_array_element_address(&mut self, name: &str, index: &Expr) -> Result<(), Diagnostic> {
        self.validate_array_index_type(index)?;
        if let Some(element) = self.const_array_element_variable(name, index)? {
            self.line(&format!("    ld hl, {:06X}h", element.addr));
            return Ok(());
        }

        let (array, element_size, _) = self.array_info(name)?;
        self.emit_expr_to_hl(index, ValueWidth::U24)?;
        match element_size {
            1 => {}
            2 => self.line("    add hl, hl"),
            3 => {
                self.line("    push hl");
                self.line("    add hl, hl");
                self.line("    pop bc");
                self.line("    add hl, bc");
            }
            _ => {
                let index_value = self.alloc_var(ValueWidth::U24.bytes());
                self.emit_store_hl(index_value);
                for _ in 1..element_size {
                    self.line("    push hl");
                    self.emit_load_hl(index_value);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                }
            }
        }
        self.line("    push hl");
        self.line(&format!("    ld hl, {:06X}h", array.addr));
        self.line("    pop bc");
        self.line("    add hl, bc");
        Ok(())
    }

    fn emit_load_indexed_element_to_a(
        &mut self,
        name: &str,
        index: &Expr,
    ) -> Result<(), Diagnostic> {
        let width = self.array_element_width(name)?;
        if width != ValueWidth::U8 {
            return Err(Diagnostic::new(format!(
                "array `{name}` element is not u8-sized"
            )));
        }
        self.emit_array_element_address(name, index)?;
        self.line("    ld a, (hl)");
        Ok(())
    }

    fn emit_load_indexed_element_to_hl(
        &mut self,
        name: &str,
        index: &Expr,
    ) -> Result<(), Diagnostic> {
        let (_, element_size, _) = self.array_info(name)?;
        if let Some(element) = self.const_array_element_variable(name, index)? {
            self.emit_load_width(element);
            return Ok(());
        }

        self.emit_array_element_address(name, index)?;
        match element_size {
            1 => {
                self.line("    ld a, (hl)");
                self.line("    ld hl, 000000h");
                self.line("    ld l, a");
            }
            2 | 3 => {
                let result = self.alloc_var(element_size);
                for offset in 0..element_size {
                    if offset != 0 {
                        self.line("    inc hl");
                    }
                    self.line("    ld a, (hl)");
                    self.line(&format!("    ld ({:06X}h), a", result.addr + offset));
                }
                self.emit_load_width(result);
            }
            _ => unreachable!("unsupported array element size"),
        }
        Ok(())
    }

    fn emit_index_assignment(
        &mut self,
        name: &str,
        index: &Expr,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let ty = self.array_element_type(name)?;
        if let Some(element) = self.const_array_element_variable(name, index)? {
            if op == AssignOp::Set {
                self.validate_expr_assignable_to_type(value, &ty)?;
                self.emit_storage_initializer(element, &ty, value)?;
                return Ok(());
            }
            self.ensure_compound_assignment_target(&ty, op)?;
            element.width()?;
            let signed = self.type_is_signed(&ty)?;
            self.emit_typed_assignment_value(element, &ty, op, value, signed)?;
            self.emit_store_width(element);
            return Ok(());
        }

        let (_, element_size, _) = self.array_info(name)?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        self.emit_array_element_address(name, index)?;
        self.emit_store_hl(addr);

        let element = self.symbols.storage_at(0, &ty)?;
        if op != AssignOp::Set {
            self.ensure_compound_assignment_target(&ty, op)?;
            element.width()?;
            let current = self.alloc_var(element_size);
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.alloc_var(element_size);
            let signed = self.type_is_signed(&ty)?;
            self.emit_typed_assignment_value(current, &ty, op, value, signed)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        if op == AssignOp::Set {
            self.validate_expr_assignable_to_type(value, &ty)?;
        }
        let stored = self.alloc_storage(&ty)?;
        self.emit_storage_initializer(stored, &ty, value)?;
        self.emit_load_hl(addr);
        self.emit_store_var_to_pointed_width(stored);
        Ok(())
    }

    fn emit_access_assignment(
        &mut self,
        path: &AccessPath,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let path = self.canonical_access_path(path);
        let ty = self.access_type(&path)?;
        if let Some(variable) = self.const_access_variable(&path)? {
            if op == AssignOp::Set {
                self.validate_expr_assignable_to_type(value, &ty)?;
                self.emit_storage_initializer(variable, &ty, value)?;
                return Ok(());
            }
            self.ensure_compound_assignment_target(&ty, op)?;
            variable.width()?;
            let signed = self.type_is_signed(&ty)?;
            self.emit_typed_assignment_value(variable, &ty, op, value, signed)?;
            self.emit_store_width(variable);
            return Ok(());
        }

        let size = self.symbols.type_size(&ty)?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        self.emit_access_address(&path)?;
        self.emit_store_hl(addr);

        if op != AssignOp::Set {
            self.ensure_compound_assignment_target(&ty, op)?;
            let current = self.alloc_var(size);
            current.width()?;
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.alloc_var(size);
            let signed = self.type_is_signed(&ty)?;
            self.emit_typed_assignment_value(current, &ty, op, value, signed)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        self.validate_expr_assignable_to_type(value, &ty)?;
        let stored = self.alloc_storage(&ty)?;
        self.emit_storage_initializer(stored, &ty, value)?;
        self.emit_load_hl(addr);
        self.emit_store_var_to_pointed_width(stored);
        Ok(())
    }

    fn u8(&self, expr: &Expr) -> Result<u8, Diagnostic> {
        let value = self.symbols.eval_i64(expr)?;
        if !(0..=0xFF).contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {value} is outside u8 range"
            )));
        }
        Ok(value as u8)
    }

    fn u16(&self, expr: &Expr) -> Result<u16, Diagnostic> {
        let value = self.symbols.eval_i64(expr)?;
        if !(0..=0xFFFF).contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {value} is outside u16 range"
            )));
        }
        Ok(value as u16)
    }

    fn u24(&self, expr: &Expr) -> Result<u32, Diagnostic> {
        let value = self.symbols.eval_i64(expr)?;
        if !(0..=0xFF_FFFF).contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {value} is outside u24 range"
            )));
        }
        Ok(value as u32)
    }

    fn value_for_width(&self, expr: &Expr, width: ValueWidth) -> Result<u32, Diagnostic> {
        let value = match width {
            ValueWidth::U8 => self.u8(expr).map(u32::from),
            ValueWidth::U16 => self.u16(expr).map(u32::from),
            ValueWidth::U24 => self.u24(expr),
        }?;
        self.validate_value_width_for_target(value, width)?;
        Ok(value)
    }

    fn eval_i64_with_local_constants(&self, expr: &Expr) -> Result<i64, Diagnostic> {
        match expr {
            Expr::Ident(name) => self
                .local_constant(name)
                .map(|constant| constant.value)
                .map(Ok)
                .unwrap_or_else(|| self.symbols.eval_i64(expr)),
            Expr::Unary { op, expr } => {
                let value = self.eval_i64_with_local_constants(expr)?;
                Ok(match op {
                    UnaryOp::Neg => value.wrapping_neg(),
                    UnaryOp::BitNot => !value,
                    UnaryOp::Not => i64::from(value == 0),
                })
            }
            Expr::Binary { left, op, right } => {
                let left_signed = self.expr_is_signed(left)?;
                let left_scale = self.pointer_pointee_size(left)?;
                let right_scale = self.pointer_pointee_size(right)?;
                let left = self.eval_i64_with_local_constants(left)?;
                let right = self.eval_i64_with_local_constants(right)?;
                Ok(match op {
                    BinaryOp::Mul => left.wrapping_mul(right),
                    BinaryOp::Div => trunc_div_or_zero(left, right),
                    BinaryOp::Mod => trunc_mod_or_zero(left, right),
                    BinaryOp::Add => match (left_scale, right_scale) {
                        (Some(scale), None) => left.wrapping_add(right.wrapping_mul(scale.into())),
                        (None, Some(scale)) => left.wrapping_mul(scale.into()).wrapping_add(right),
                        _ => left.wrapping_add(right),
                    },
                    BinaryOp::Sub => match (left_scale, right_scale) {
                        (Some(scale), None) => left.wrapping_sub(right.wrapping_mul(scale.into())),
                        _ => left.wrapping_sub(right),
                    },
                    BinaryOp::Shl => const_shl_or_zero(left, right),
                    BinaryOp::Shr => const_shr_or_zero(left, right, left_signed),
                    BinaryOp::Lt => i64::from(left < right),
                    BinaryOp::Le => i64::from(left <= right),
                    BinaryOp::Gt => i64::from(left > right),
                    BinaryOp::Ge => i64::from(left >= right),
                    BinaryOp::Eq => i64::from(left == right),
                    BinaryOp::Ne => i64::from(left != right),
                    BinaryOp::BitAnd => left & right,
                    BinaryOp::BitXor => left ^ right,
                    BinaryOp::BitOr => left | right,
                    BinaryOp::And => i64::from(left != 0 && right != 0),
                    BinaryOp::Or => i64::from(left != 0 || right != 0),
                })
            }
            Expr::Cast { expr, ty } => {
                let value = self.eval_i64_with_local_constants(expr)?;
                self.symbols.const_cast_value(value, ty)
            }
            _ => self.symbols.eval_i64(expr),
        }
    }

    fn local_constant_value_for_width(
        &self,
        name: &str,
        width: ValueWidth,
    ) -> Result<Option<u32>, Diagnostic> {
        let Some(constant) = self.local_constant(name) else {
            return Ok(None);
        };
        if self.symbols.type_width(&constant.ty)? != width {
            return Ok(None);
        }
        let value = self.value_for_type(constant.value, &constant.ty, width)?;
        Ok(Some(value))
    }

    fn value_for_type(&self, value: i64, ty: &Type, width: ValueWidth) -> Result<u32, Diagnostic> {
        let resolved = self.symbols.resolved_type(ty)?;
        self.symbols.validate_value_for_type(value, &resolved)?;
        let bits = u32::from(width.bytes()) * 8;
        let mask = (1_i128 << bits) - 1;
        let value = ((value as i128) & mask) as u32;
        self.validate_value_width_for_target(value, width)?;
        Ok(value)
    }

    fn validate_value_width_for_target(
        &self,
        value: u32,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if is_z80_family_16bit(self.cpu) && width == ValueWidth::U24 && value > 0xFFFF {
            return Err(Diagnostic::new(format!(
                "24-bit value 0x{value:06X} cannot be encoded for 16-bit target `{}`",
                self.cpu.as_str()
            )));
        }
        Ok(())
    }

    fn type_is_signed(&self, ty: &Type) -> Result<bool, Diagnostic> {
        Ok(type_is_signed(&self.symbols.resolved_type(ty)?))
    }

    fn expr_is_signed(&self, expr: &Expr) -> Result<bool, Diagnostic> {
        self.type_is_signed(&self.expr_type(expr)?)
    }

    fn expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Ident(name) => self
                .named_value_type(name)
                .cloned()
                .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`"))),
            Expr::Int(value) => {
                if (0..=0xFF).contains(value) {
                    Ok(Type::Named("u8".to_owned()))
                } else if (0..=0xFFFF).contains(value) {
                    Ok(Type::Named("u16".to_owned()))
                } else {
                    Ok(Type::Named("u24".to_owned()))
                }
            }
            Expr::TypedInt(_, ty) => Ok(ty.clone()),
            Expr::Char(_) | Expr::In(_) => Ok(Type::Named("u8".to_owned())),
            Expr::Bool(_) => Ok(Type::Named("bool".to_owned())),
            Expr::String(_) => Ok(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            Expr::Array(_) => Err(Diagnostic::new("array literal does not have scalar type")),
            Expr::Index { name, .. } => self.array_element_type(name),
            Expr::Field { base, field } => self
                .named_value_type(&format!("{base}.{field}"))
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| self.field_type(base, field)),
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty() {
                    if let Some(ty) = self.named_value_type(&path.root) {
                        return Ok(ty.clone());
                    }
                    if let Some(ty) = self.embed_property_type(&path.root) {
                        return Ok(ty);
                    }
                }
                self.access_type(&path)
            }
            Expr::AddressOfIndex { name, .. } => {
                Ok(Type::Ptr(Box::new(self.array_element_type(name)?)))
            }
            Expr::AddressOfField { base, field } => {
                Ok(Type::Ptr(Box::new(self.field_type(base, field)?)))
            }
            Expr::AddressOfAccess(path) => {
                let path = self.canonical_access_path(path);
                Ok(Type::Ptr(Box::new(self.access_type(&path)?)))
            }
            Expr::AddressOf(name) => {
                if self.symbols.embeds.contains_key(name) {
                    return Ok(Type::Ptr(Box::new(Type::Named("u8".to_owned()))));
                }
                let Some(ty) = self.variable_type(name) else {
                    return Err(Diagnostic::new(format!("unknown variable `{name}`")));
                };
                Ok(Type::Ptr(Box::new(self.symbols.resolved_type(ty)?)))
            }
            Expr::Deref(ptr) => match self.symbols.resolved_type(&self.expr_type(ptr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                Type::Named(name) if name == "ptr24" => Err(Diagnostic::new(
                    "raw ptr24 dereference requires an explicit typed pointer cast",
                )),
                other => Err(Diagnostic::new(format!(
                    "cannot dereference non-pointer expression of type `{other:?}`"
                ))),
            },
            Expr::StructInit { ty, .. } => Ok(Type::Named(ty.clone())),
            Expr::Cast { ty, .. } => Ok(ty.clone()),
            Expr::Call { path, .. }
                if matches!(path_text(path).as_str(), "mem.peek8" | "ezra.mem.peek8") =>
            {
                Ok(Type::Named("u8".to_owned()))
            }
            Expr::Call { path, .. } => self.call_return_type(path),
            Expr::Unary { expr, op } => match op {
                UnaryOp::Not => {
                    self.ensure_expr_is_bool(expr, "logical operand")?;
                    Ok(Type::Named("bool".to_owned()))
                }
                UnaryOp::Neg | UnaryOp::BitNot => self.expr_type(expr),
            },
            Expr::Binary { left, op, right } => {
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) {
                    Ok(Type::Named("bool".to_owned()))
                } else if matches!(op, BinaryOp::Add | BinaryOp::Sub)
                    && self.pointer_pointee_size(left)?.is_some()
                {
                    self.expr_type(left)
                } else if *op == BinaryOp::Add && self.pointer_pointee_size(right)?.is_some() {
                    self.expr_type(right)
                } else if self.expr_width(left)? >= self.expr_width(right)? {
                    self.expr_type(left)
                } else {
                    self.expr_type(right)
                }
            }
        }
    }

    fn expr_width(&self, expr: &Expr) -> Result<ValueWidth, Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(variable) = self.variable_opt(name) {
                    variable.width()
                } else if let Some(ty) = self.named_value_type(name) {
                    self.symbols.type_width(ty)
                } else {
                    let value = self.symbols.eval_i64(expr)?;
                    if (0..=0xFF).contains(&value) {
                        Ok(ValueWidth::U8)
                    } else if (0..=0xFFFF).contains(&value) {
                        Ok(ValueWidth::U16)
                    } else {
                        Ok(ValueWidth::U24)
                    }
                }
            }
            Expr::Int(value) => {
                if (0..=0xFF).contains(value) {
                    Ok(ValueWidth::U8)
                } else if (0..=0xFFFF).contains(value) {
                    Ok(ValueWidth::U16)
                } else {
                    Ok(ValueWidth::U24)
                }
            }
            Expr::TypedInt(_, ty) => self.symbols.type_width(ty),
            Expr::Char(_) | Expr::Bool(_) | Expr::In(_) => Ok(ValueWidth::U8),
            Expr::String(_) => Ok(ValueWidth::U24),
            Expr::Array(_) => Err(Diagnostic::new("array literal does not have scalar width")),
            Expr::StructInit { ty, .. } => Err(Diagnostic::new(format!(
                "struct `{ty}` literal does not have scalar width"
            ))),
            Expr::Index { name, .. } => self.array_element_width(name),
            Expr::Field { base, field } => {
                let key = format!("{base}.{field}");
                if let Some(ty) = self.named_value_type(&key) {
                    self.symbols.type_width(ty)
                } else {
                    self.field_variable(base, field)?.width()
                }
            }
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty() {
                    if let Some(ty) = self.named_value_type(&path.root) {
                        return self.symbols.type_width(ty);
                    }
                    if self.symbols.embed_property_value(&path.root).is_some() {
                        return Ok(ValueWidth::U24);
                    }
                }
                if let Some(variable) = self.const_access_variable(&path)? {
                    variable.width()
                } else {
                    self.symbols.type_width(&self.access_type(&path)?)
                }
            }
            Expr::AddressOfIndex { .. } => Ok(ValueWidth::U24),
            Expr::AddressOfField { .. } => Ok(ValueWidth::U24),
            Expr::AddressOfAccess(_) => Ok(ValueWidth::U24),
            Expr::AddressOf(_) => Ok(ValueWidth::U24),
            Expr::Deref(ptr) => match self.symbols.resolved_type(&self.expr_type(ptr)?)? {
                Type::Ptr(inner) => self.symbols.type_width(&inner),
                Type::Named(name) if name == "ptr24" => Err(Diagnostic::new(
                    "raw ptr24 dereference requires an explicit typed pointer cast",
                )),
                other => Err(Diagnostic::new(format!(
                    "cannot dereference non-pointer expression of type `{other:?}`"
                ))),
            },
            Expr::Cast { ty, .. } => self.symbols.type_width(ty),
            Expr::Call { path, .. }
                if matches!(path_text(path).as_str(), "mem.peek8" | "ezra.mem.peek8") =>
            {
                Ok(ValueWidth::U8)
            }
            Expr::Call { path, .. } => self.call_return_width(path),
            Expr::Unary { expr, op } => match op {
                UnaryOp::Not => Ok(ValueWidth::U8),
                UnaryOp::Neg | UnaryOp::BitNot => self.expr_width(expr),
            },
            Expr::Binary { left, op, right } => {
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) {
                    Ok(ValueWidth::U8)
                } else {
                    Ok(self.expr_width(left)?.max(self.expr_width(right)?))
                }
            }
        }
    }

    fn call_return_type(&self, path: &[String]) -> Result<Type, Diagnostic> {
        let name = path_text(path);
        let sig = self
            .symbols
            .functions
            .get(&name)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        sig.return_type
            .clone()
            .ok_or_else(|| Diagnostic::new(format!("function `{name}` does not return a value")))
    }

    fn call_return_width(&self, path: &[String]) -> Result<ValueWidth, Diagnostic> {
        let name = path_text(path);
        let sig = self
            .symbols
            .functions
            .get(&name)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if sig.return_type.is_none() {
            return Err(Diagnostic::new(format!(
                "function `{name}` does not return a value"
            )));
        }
        Ok(sig.return_width)
    }

    fn maybe_const_shift_count(&self, expr: &Expr) -> Result<Option<u8>, Diagnostic> {
        match self.symbols.eval_i64(expr) {
            Ok(value) => self.validate_shift_count(value).map(Some),
            Err(_) => Ok(None),
        }
    }

    fn validate_shift_count(&self, value: i64) -> Result<u8, Diagnostic> {
        if !(0..=u8::MAX as i64).contains(&value) {
            return Err(Diagnostic::new(format!(
                "shift count {value} is outside supported range 0..=255"
            )));
        }
        Ok(value as u8)
    }

    fn binary_operands_are_signed(&self, left: &Expr, right: &Expr) -> Result<bool, Diagnostic> {
        Ok(
            type_is_signed(&self.symbols.resolved_type(&self.expr_type(left)?)?)
                || type_is_signed(&self.symbols.resolved_type(&self.expr_type(right)?)?),
        )
    }

    fn ensure_binary_arithmetic_operands_compatible(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.symbols.resolved_type(&self.expr_type(left)?)?;
        let right_type = self.symbols.resolved_type(&self.expr_type(right)?)?;
        if matches!(left_type, Type::Array { .. }) || matches!(right_type, Type::Array { .. }) {
            return Err(Diagnostic::new("array value cannot be used as a scalar"));
        }
        if type_is_bool(&left_type) || type_is_bool(&right_type) {
            return Err(Diagnostic::new("type mismatch"));
        }
        let left_is_literal = expr_is_untyped_literal(left);
        let right_is_literal = expr_is_untyped_literal(right);
        if left_is_literal && right_is_literal {
            return Ok(());
        }

        if matches!(left_type, Type::Ptr(_)) || matches!(right_type, Type::Ptr(_)) {
            return Err(Diagnostic::new("type mismatch"));
        }

        if left_is_literal {
            let value = self.symbols.eval_i64(left)?;
            return self.symbols.validate_value_for_type(value, &right_type);
        }
        if right_is_literal {
            let value = self.symbols.eval_i64(right)?;
            return self.symbols.validate_value_for_type(value, &left_type);
        }

        if type_is_signed(&left_type) != type_is_signed(&right_type) {
            return Err(Diagnostic::new("signed/unsigned mix without cast"));
        }
        if self.symbols.type_width(&left_type)? != self.symbols.type_width(&right_type)? {
            return Err(Diagnostic::new(
                "arithmetic operands must have same width without cast",
            ));
        }
        Ok(())
    }

    fn validate_expr_assignable_to_type(
        &self,
        expr: &Expr,
        target: &Type,
    ) -> Result<(), Diagnostic> {
        self.validate_typed_literal_ranges(expr)?;
        if let Expr::Array(values) = expr {
            let Type::Array { element, len } = self.symbols.resolved_type(target)? else {
                return Err(Diagnostic::new("type mismatch"));
            };
            let len = self.symbols.array_len(&len)?;
            if values.len() as u32 > len {
                return Err(Diagnostic::new(format!(
                    "array initializer has {} values but array length is {len}",
                    values.len()
                )));
            }
            for value in values {
                self.validate_expr_assignable_to_type(value, &element)?;
            }
            return Ok(());
        }
        if let Expr::Cast { ty, .. } = expr {
            self.symbols.type_width(ty)?;
            return self.validate_type_assignable_to_type(ty, target);
        }
        if expr_is_untyped_literal(expr) {
            if let Ok(value) = self.symbols.eval_i64(expr) {
                self.symbols.validate_value_for_type(value, target)?;
            }
            return Ok(());
        }

        let source_type = self.expr_type(expr)?;
        self.validate_type_assignable_to_type(&source_type, target)
    }

    fn validate_expr_is_ptr_u8(&self, expr: &Expr) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if ty == ptr_u8_type() {
            Ok(())
        } else {
            Err(Diagnostic::new("type mismatch"))
        }
    }

    fn validate_expr_has_test_width(
        &self,
        expr: &Expr,
        width: ValueWidth,
        allow_bool: bool,
    ) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if allow_bool && type_is_bool(&ty) {
            return Ok(());
        }
        if type_is_bool(&ty) || matches!(ty, Type::Ptr(_)) {
            return Err(Diagnostic::new("type mismatch"));
        }
        let actual = self.symbols.type_width(&ty)?;
        if actual == width {
            return Ok(());
        }
        if let Ok(value) = self.symbols.eval_i64(expr) {
            self.symbols
                .wrap_value_for_type(value, &width_unsigned_type(width))?;
            return Ok(());
        }
        if actual < width {
            return Err(Diagnostic::new("widening without cast"));
        }
        if actual > width {
            return Err(Diagnostic::new("narrowing without cast"));
        }
        Ok(())
    }

    fn validate_type_assignable_to_type(
        &self,
        source: &Type,
        target: &Type,
    ) -> Result<(), Diagnostic> {
        let source_type = self.symbols.resolved_type(source)?;
        let target_type = self.symbols.resolved_type(target)?;
        if source_type == target_type {
            return Ok(());
        }
        if let (
            Type::Array {
                element: source_element,
                len: source_len,
            },
            Type::Array {
                element: target_element,
                len: target_len,
            },
        ) = (&source_type, &target_type)
        {
            if self.symbols.array_len(source_len)? != self.symbols.array_len(target_len)? {
                return Err(Diagnostic::new("type mismatch"));
            }
            return self.validate_type_assignable_to_type(source_element, target_element);
        }
        if matches!(source_type, Type::Array { .. }) || matches!(target_type, Type::Array { .. }) {
            return Err(Diagnostic::new("type mismatch"));
        }
        if type_is_bool(&source_type) || type_is_bool(&target_type) {
            return Err(Diagnostic::new("type mismatch"));
        }
        if matches!(source_type, Type::Ptr(_)) || matches!(target_type, Type::Ptr(_)) {
            return Err(Diagnostic::new("type mismatch"));
        }

        let source_width = self.symbols.type_width(&source_type)?;
        let target_width = self.symbols.type_width(&target_type)?;
        if source_width < target_width {
            return Err(Diagnostic::new("widening without cast"));
        }
        if source_width > target_width {
            return Err(Diagnostic::new("narrowing without cast"));
        }
        if type_is_signed(&source_type) != type_is_signed(&target_type) {
            return Err(Diagnostic::new("signed/unsigned mix without cast"));
        }
        Err(Diagnostic::new("type mismatch"))
    }

    fn validate_typed_literal_ranges(&self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::TypedInt(value, ty) => self.symbols.validate_value_for_type(*value, ty),
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
            } => {
                if let Expr::TypedInt(value, ty) = expr.as_ref() {
                    let value = value.checked_neg().ok_or_else(|| {
                        Diagnostic::new(format!("value -{value} is outside i24 range"))
                    })?;
                    self.symbols.validate_value_for_type(value, ty)
                } else {
                    self.validate_typed_literal_ranges(expr)
                }
            }
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::Deref(expr) => {
                self.validate_typed_literal_ranges(expr)
            }
            Expr::Binary { left, right, .. } => {
                self.validate_typed_literal_ranges(left)?;
                self.validate_typed_literal_ranges(right)
            }
            Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
                self.validate_typed_literal_ranges(index)
            }
            Expr::Access(path) | Expr::AddressOfAccess(path) => {
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        self.validate_typed_literal_ranges(index)?;
                    }
                }
                Ok(())
            }
            Expr::Array(values) => {
                for value in values {
                    self.validate_typed_literal_ranges(value)?;
                }
                Ok(())
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    self.validate_typed_literal_ranges(value)?;
                }
                Ok(())
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.validate_typed_literal_ranges(arg)?;
                }
                Ok(())
            }
            Expr::Int(_)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::String(_)
            | Expr::Ident(_)
            | Expr::In(_)
            | Expr::Field { .. }
            | Expr::AddressOf(_)
            | Expr::AddressOfField { .. } => Ok(()),
        }
    }

    fn validate_expr_arithmetic_compatibility(&self, expr: &Expr) -> Result<(), Diagnostic> {
        self.validate_typed_literal_ranges(expr)?;
        match expr {
            Expr::Binary { left, op, right } => {
                self.validate_expr_arithmetic_compatibility(left)?;
                self.validate_expr_arithmetic_compatibility(right)?;
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.ensure_expr_is_bool(left, "logical operand")?;
                    self.ensure_expr_is_bool(right, "logical operand")?;
                } else if is_comparison(*op) {
                    self.ensure_comparison_operands_compatible(left, *op, right)?;
                } else if matches!(op, BinaryOp::Add | BinaryOp::Sub)
                    && (self.pointer_pointee_size(left)?.is_some()
                        || self.pointer_pointee_size(right)?.is_some())
                {
                    self.ensure_pointer_arithmetic_expr_compatible(left, *op, right)?;
                } else if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
                    self.ensure_shift_operands_compatible(left, right)?;
                } else if matches!(
                    op,
                    BinaryOp::Add
                        | BinaryOp::Sub
                        | BinaryOp::Mul
                        | BinaryOp::Div
                        | BinaryOp::Mod
                        | BinaryOp::BitAnd
                        | BinaryOp::BitOr
                        | BinaryOp::BitXor
                ) {
                    self.ensure_binary_arithmetic_operands_compatible(left, right)?;
                }
            }
            Expr::Unary { expr, op } => {
                self.validate_expr_arithmetic_compatibility(expr)?;
                match op {
                    UnaryOp::Not => self.ensure_expr_is_bool(expr, "logical operand")?,
                    UnaryOp::Neg | UnaryOp::BitNot => {
                        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
                        validate_integer_unary_operand_type(&ty)?;
                    }
                }
            }
            Expr::Cast { expr, .. } | Expr::Deref(expr) => {
                self.validate_expr_arithmetic_compatibility(expr)?;
            }
            Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
                self.validate_expr_arithmetic_compatibility(index)?;
            }
            Expr::Access(path) | Expr::AddressOfAccess(path) => {
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        self.validate_expr_arithmetic_compatibility(index)?;
                    }
                }
            }
            Expr::Array(values) => {
                for value in values {
                    self.validate_expr_arithmetic_compatibility(value)?;
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    self.validate_expr_arithmetic_compatibility(value)?;
                }
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.validate_expr_arithmetic_compatibility(arg)?;
                }
            }
            Expr::Int(_)
            | Expr::TypedInt(_, _)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::String(_)
            | Expr::Ident(_)
            | Expr::In(_)
            | Expr::Field { .. }
            | Expr::AddressOf(_)
            | Expr::AddressOfField { .. } => {}
        }
        Ok(())
    }

    fn ensure_shift_operands_compatible(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.symbols.resolved_type(&self.expr_type(left)?)?;
        validate_shift_operand_type(&left_type)?;
        self.ensure_shift_count_compatible(right)
    }

    fn ensure_shift_count_compatible(&self, count: &Expr) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(count)?)?;
        validate_shift_count_integer_type(&ty)?;

        if let Ok(value) = self.eval_i64_with_local_constants(count) {
            self.validate_shift_count(value)?;
            return Ok(());
        }

        validate_runtime_shift_count_type(&ty)
    }

    fn ensure_pointer_arithmetic_expr_compatible(
        &self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_scale = self.pointer_pointee_size(left)?;
        let right_scale = self.pointer_pointee_size(right)?;
        match (op, left_scale, right_scale) {
            (BinaryOp::Add, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer arithmetic requires exactly one pointer operand",
            )),
            (BinaryOp::Add, Some(_), None) => self.ensure_pointer_offset_expr(right),
            (BinaryOp::Add, None, Some(_)) => self.ensure_pointer_offset_expr(left),
            (BinaryOp::Sub, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer subtraction between two pointers is not supported",
            )),
            (BinaryOp::Sub, Some(_), None) => self.ensure_pointer_offset_expr(right),
            (BinaryOp::Sub, None, Some(_)) => Err(Diagnostic::new(
                "cannot subtract a pointer from a non-pointer value",
            )),
            _ => Ok(()),
        }
    }

    fn ensure_comparison_operands_compatible(
        &self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.symbols.resolved_type(&self.expr_type(left)?)?;
        let right_type = self.symbols.resolved_type(&self.expr_type(right)?)?;
        if let Some(name) = struct_scalar_type(&left_type, &self.symbols.structs) {
            return Err(Diagnostic::new(format!(
                "struct `{name}` cannot be used as a scalar value"
            )));
        }
        if let Some(name) = struct_scalar_type(&right_type, &self.symbols.structs) {
            return Err(Diagnostic::new(format!(
                "struct `{name}` cannot be used as a scalar value"
            )));
        }
        if !type_is_bool(&left_type)
            && !type_is_bool(&right_type)
            && !matches!(left_type, Type::Ptr(_))
            && !matches!(right_type, Type::Ptr(_))
        {
            if expr_is_untyped_literal(left) && expr_is_untyped_literal(right) {
                return Ok(());
            }
            if expr_is_untyped_literal(left) {
                let value = self.symbols.eval_i64(left)?;
                return self.symbols.validate_value_for_type(value, &right_type);
            }
            if expr_is_untyped_literal(right) {
                let value = self.symbols.eval_i64(right)?;
                return self.symbols.validate_value_for_type(value, &left_type);
            }
        }
        validate_comparison_types(&left_type, op, &right_type, || {
            if expr_is_untyped_literal(left) || expr_is_untyped_literal(right) {
                None
            } else {
                Some((
                    self.symbols.type_width(&left_type).ok()?,
                    self.symbols.type_width(&right_type).ok()?,
                ))
            }
        })
    }

    fn ensure_expr_is_bool(&self, expr: &Expr, context: &str) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if type_is_bool(&ty) {
            Ok(())
        } else {
            Err(Diagnostic::new(format!("{context} must be bool")))
        }
    }

    fn current_return_type(&self) -> &Type {
        self.return_type_stack
            .last()
            .and_then(|ty| ty.as_ref())
            .expect("function return type exists during value return emission")
    }

    fn current_function_requires_return_value(&self) -> bool {
        *self
            .return_value_stack
            .last()
            .expect("function return kind exists during emission")
    }

    fn current_function_name(&self) -> &str {
        self.function_name_stack
            .last()
            .expect("function name exists during emission")
    }

    fn current_function_uses_frame(&self) -> bool {
        self.function_frame_stack
            .last()
            .copied()
            .expect("function frame state exists during emission")
    }

    fn current_function_is_interrupt(&self) -> bool {
        self.function_interrupt_stack
            .last()
            .copied()
            .expect("function interrupt state exists during emission")
    }

    fn current_function_is_naked(&self) -> bool {
        self.function_naked_stack
            .last()
            .copied()
            .expect("function naked state exists during emission")
    }

    fn port(&self, name: &str) -> Result<u8, Diagnostic> {
        self.symbols
            .ports
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown port `{name}`")))
    }

    fn variable(&self, name: &str) -> Result<Variable, Diagnostic> {
        self.variable_opt(name)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{name}`")))
    }

    fn dotted_variable(&self, base: &str, field: &str) -> Option<Variable> {
        self.variable_opt(&format!("{base}.{field}"))
    }

    fn variable_opt(&self, name: &str) -> Option<Variable> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .or_else(|| self.symbols.globals.get(name).copied())
    }

    fn variable_type(&self, name: &str) -> Option<&Type> {
        self.scope_types
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.symbols.global_types.get(name))
    }

    fn named_value_type(&self, name: &str) -> Option<&Type> {
        self.variable_type(name)
            .or_else(|| self.symbols.constant_types.get(name))
    }

    fn embed_property_type(&self, name: &str) -> Option<Type> {
        self.symbols.embed_property_value(name)?;
        let (_, property) = name.rsplit_once('.')?;
        match property {
            "ptr" | "end" => Some(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            "len" => Some(Type::Named("u24".to_owned())),
            _ => None,
        }
    }

    fn canonical_access_path(&self, path: &AccessPath) -> AccessPath {
        if self.named_value_type(&path.root).is_some() {
            return path.clone();
        }

        let mut candidate = path.root.clone();
        let mut best = None;
        for (index, segment) in path.segments.iter().enumerate() {
            let AccessSegment::Field(field) = segment else {
                break;
            };
            candidate.push('.');
            candidate.push_str(field);
            if self.named_value_type(&candidate).is_some()
                || self.symbols.embed_property_value(&candidate).is_some()
            {
                best = Some((candidate.clone(), index + 1));
            }
            if let Some((_, original)) = candidate.split_once('.')
                && (self.named_value_type(original).is_some()
                    || self.symbols.embed_property_value(original).is_some())
            {
                best = Some((original.to_owned(), index + 1));
            }
        }

        if let Some((root, consumed)) = best {
            AccessPath {
                root,
                segments: path.segments[consumed..].to_vec(),
            }
        } else {
            path.clone()
        }
    }

    fn name_in_current_function(&self, name: &str) -> bool {
        self.scope_types
            .iter()
            .any(|scope| scope.contains_key(name))
            || self.symbols.global_types.contains_key(name)
            || self.symbols.constant_types.contains_key(name)
            || self.symbols.functions.contains_key(name)
    }

    fn current_scope_mut(&mut self) -> &mut HashMap<String, Variable> {
        self.scopes
            .last_mut()
            .expect("function scope exists during statement emission")
    }

    fn current_scope_types_mut(&mut self) -> &mut HashMap<String, Type> {
        self.scope_types
            .last_mut()
            .expect("function type scope exists during statement emission")
    }

    fn current_local_constants_mut(&mut self) -> &mut HashMap<String, LocalConstant> {
        self.local_constants
            .last_mut()
            .expect("function local constant scope exists during statement emission")
    }

    fn current_readonly_pointer_aliases_mut(&mut self) -> &mut HashMap<String, u32> {
        self.readonly_pointer_aliases
            .last_mut()
            .expect("function read-only pointer alias scope exists during statement emission")
    }

    fn local_constant(&self, name: &str) -> Option<&LocalConstant> {
        for index in (0..self.local_constants.len()).rev() {
            if let Some(constant) = self.local_constants[index].get(name) {
                return Some(constant);
            }
            if self
                .scope_types
                .get(index)
                .is_some_and(|scope| scope.contains_key(name))
            {
                return None;
            }
        }
        None
    }

    fn readonly_pointer_alias(&self, name: &str) -> Option<u32> {
        for index in (0..self.readonly_pointer_aliases.len()).rev() {
            if let Some(addr) = self.readonly_pointer_aliases[index].get(name) {
                return Some(*addr);
            }
            if self
                .scope_types
                .get(index)
                .is_some_and(|scope| scope.contains_key(name))
            {
                return None;
            }
        }
        None
    }

    fn current_function_assigns(&self, name: &str) -> bool {
        self.assigned_names_stack
            .last()
            .is_some_and(|names| names.contains(name))
    }

    fn record_local_constant(&mut self, name: &str, ty: &Type, value: &Expr) {
        if self.current_function_assigns(name) {
            self.current_local_constants_mut().remove(name);
            return;
        }
        if !self.local_constant_supported_type(ty) {
            self.current_local_constants_mut().remove(name);
            return;
        }
        let Ok(width) = self.symbols.type_width(ty) else {
            return;
        };
        let Ok(value) = self.eval_i64_with_local_constants(value) else {
            self.current_local_constants_mut().remove(name);
            return;
        };
        if self.value_for_type(value, ty, width).is_ok() {
            self.current_local_constants_mut().insert(
                name.to_owned(),
                LocalConstant {
                    value,
                    ty: ty.clone(),
                },
            );
        }
    }

    fn local_constant_supported_type(&self, ty: &Type) -> bool {
        match self.symbols.resolved_type(ty) {
            Ok(Type::Ptr(_)) => true,
            Ok(Type::Named(name)) => matches!(
                name.as_str(),
                "u8" | "i8" | "bool" | "u16" | "i16" | "u24" | "i24" | "ptr24"
            ),
            _ => false,
        }
    }

    fn invalidate_local_constant(&mut self, name: &str) {
        for scope in self.local_constants.iter_mut().rev() {
            if scope.remove(name).is_some() {
                return;
            }
        }
    }

    fn invalidate_all_local_constants(&mut self) {
        for scope in &mut self.local_constants {
            scope.clear();
        }
    }

    fn record_readonly_pointer_alias(&mut self, name: &str, value: &Expr) {
        let Some(addr) = self.readonly_write_addr(value) else {
            self.current_readonly_pointer_aliases_mut().remove(name);
            return;
        };
        if self.readonly_embed_name_for_addr(addr).is_some()
            || self.readonly_string_literal_for_addr(addr).is_some()
        {
            self.current_readonly_pointer_aliases_mut()
                .insert(name.to_owned(), addr);
        } else {
            self.current_readonly_pointer_aliases_mut().remove(name);
        }
    }

    fn readonly_embed_name_for_addr(&self, addr: u32) -> Option<&str> {
        let addr = u64::from(addr);
        for (name, embed) in &self.symbols.embeds {
            let Some(len) = embed.variable.len else {
                continue;
            };
            let start = u64::from(embed.variable.addr);
            let end = start + u64::from(len);
            if addr >= start && addr < end {
                return Some(name.as_str());
            }
        }
        None
    }

    fn readonly_string_literal_for_addr(&self, addr: u32) -> Option<&str> {
        self.readonly_string_literal_for_range(u64::from(addr), u64::from(addr) + 1)
    }

    fn readonly_string_literal_for_range(&self, start: u64, end: u64) -> Option<&str> {
        for (value, variable) in self
            .string_literals
            .iter()
            .chain(self.symbols.string_literals.iter())
        {
            let Some(len) = variable.len else {
                continue;
            };
            if len == 0 {
                continue;
            }
            let literal_start = u64::from(variable.addr);
            let literal_end = literal_start + u64::from(len);
            if start < literal_end && end > literal_start {
                return Some(value.as_str());
            }
        }
        None
    }

    fn invalidate_readonly_pointer_alias(&mut self, name: &str) {
        for scope in self.readonly_pointer_aliases.iter_mut().rev() {
            if scope.remove(name).is_some() {
                return;
            }
        }
    }

    fn next_label(&mut self, prefix: &str) -> String {
        let label = format!(".L_{prefix}_{}", self.label_counter);
        self.label_counter += 1;
        label
    }

    fn line(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
    }
}

fn trunc_div_or_zero(left: i64, right: i64) -> i64 {
    if right == 0 {
        0
    } else {
        ((left as i128) / (right as i128)) as i64
    }
}

fn trunc_mod_or_zero(left: i64, right: i64) -> i64 {
    if right == 0 {
        0
    } else {
        ((left as i128) % (right as i128)) as i64
    }
}

fn alloc_from_cursor(cursor: &mut u32, align: u32, size: u32) -> Result<Variable, Diagnostic> {
    if align > 1 {
        let mask = align - 1;
        *cursor = cursor
            .checked_add(mask)
            .map(|addr| addr & !mask)
            .ok_or_else(|| Diagnostic::new("section alignment exceeds 24-bit address space"))?;
    }
    let variable = Variable {
        addr: *cursor,
        size,
        element_size: Some(u32::from(ValueWidth::U8.bytes())),
        len: Some(size),
    };
    *cursor = cursor
        .checked_add(size)
        .ok_or_else(|| Diagnostic::new("section allocation exceeds 24-bit address space"))?;
    if *cursor > Address24::MAX + 1 {
        return Err(Diagnostic::new(
            "section allocation exceeds 24-bit address space",
        ));
    }
    Ok(variable)
}

fn section_cursor<'a>(cursors: &'a mut [(String, u32)], section: &str) -> &'a mut u32 {
    let index = cursors
        .iter()
        .position(|(name, _)| name == section)
        .expect("section cursor exists");
    &mut cursors[index].1
}

fn recursive_call_edges(
    program: &Program,
    functions: &HashMap<String, FunctionSig>,
) -> HashSet<(String, String)> {
    let graph = function_call_graph(program, functions);
    let mut edges = HashSet::new();
    for (caller, callees) in &graph {
        for callee in callees {
            if function_reaches(callee, caller, &graph) {
                edges.insert((caller.clone(), callee.clone()));
            }
        }
    }
    edges
}

fn function_call_graph(
    program: &Program,
    functions: &HashMap<String, FunctionSig>,
) -> HashMap<String, Vec<String>> {
    let mut graph = HashMap::new();
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        let mut calls = Vec::new();
        collect_stmt_calls(&function.body, &mut calls);
        calls.retain(|name| functions.contains_key(name));
        graph.insert(function.name.clone(), calls);
    }
    graph
}

fn function_reaches(start: &str, target: &str, graph: &HashMap<String, Vec<String>>) -> bool {
    let mut stack = vec![start.to_owned()];
    let mut visited = HashSet::new();
    while let Some(function) = stack.pop() {
        if !visited.insert(function.clone()) {
            continue;
        }
        if function == target {
            return true;
        }
        if let Some(calls) = graph.get(&function) {
            stack.extend(calls.iter().cloned());
        }
    }
    false
}

fn validate_all_function_calls(
    program: &Program,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        validate_stmt_calls(&function.body, functions)?;
    }
    Ok(())
}

fn validate_all_function_bodies(
    program: &Program,
    symbols: Symbols,
    options: AssemblyOptions,
    recursive_call_edges: HashSet<(String, String)>,
) -> Result<(), Diagnostic> {
    let mut emitter = Emitter::new(symbols, options, recursive_call_edges);
    emitter.disable_dead_code_elimination();
    if let Some(main) = program.main_function() {
        emitter.emit_function(main)?;
    }
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        if function.name == "main" {
            continue;
        }
        emitter.emit_function(function)?;
    }
    Ok(())
}

fn validate_stmt_calls(
    stmts: &[Stmt],
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } => validate_expr_calls(value, functions)?,
            Stmt::Assign { target, value, .. } => {
                validate_place_calls(target, functions)?;
                validate_expr_calls(value, functions)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                validate_expr_calls(condition, functions)?;
                validate_stmt_calls(then_body, functions)?;
                validate_stmt_calls(else_body, functions)?;
            }
            Stmt::While { condition, body } => {
                validate_expr_calls(condition, functions)?;
                validate_stmt_calls(body, functions)?;
            }
            Stmt::Loop { body } => validate_stmt_calls(body, functions)?,
            Stmt::Return(Some(expr)) | Stmt::Expr(expr) => validate_expr_calls(expr, functions)?,
            Stmt::Out { value, .. } => validate_expr_calls(value, functions)?,
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => {}
        }
    }
    Ok(())
}

fn collect_stmt_call_diagnostics(
    stmts: &[Stmt],
    spans: &[crate::ast::StmtSpan],
    functions: &HashMap<String, FunctionSig>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (index, stmt) in stmts.iter().enumerate() {
        let result = match stmt {
            Stmt::Let { value, .. } => validate_expr_calls(value, functions),
            Stmt::Assign { target, value, .. } => validate_place_calls(target, functions)
                .and_then(|_| validate_expr_calls(value, functions)),
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let result = validate_expr_calls(condition, functions);
                let children = spans
                    .get(index)
                    .map_or(&[][..], |span| span.children.as_slice());
                collect_stmt_call_diagnostics(
                    then_body,
                    &children[..children.len().min(then_body.len())],
                    functions,
                    diagnostics,
                );
                collect_stmt_call_diagnostics(
                    else_body,
                    &children[children.len().min(then_body.len())..],
                    functions,
                    diagnostics,
                );
                result
            }
            Stmt::While { condition, body } => {
                let result = validate_expr_calls(condition, functions);
                let children = spans
                    .get(index)
                    .map_or(&[][..], |span| span.children.as_slice());
                collect_stmt_call_diagnostics(body, children, functions, diagnostics);
                result
            }
            Stmt::Loop { body } => {
                let children = spans
                    .get(index)
                    .map_or(&[][..], |span| span.children.as_slice());
                collect_stmt_call_diagnostics(body, children, functions, diagnostics);
                Ok(())
            }
            Stmt::Return(Some(expr)) | Stmt::Expr(expr) => validate_expr_calls(expr, functions),
            Stmt::Out { value, .. } => validate_expr_calls(value, functions),
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => Ok(()),
        };
        if let Err(error) = result {
            let error = spans
                .get(index)
                .map(|span| locate_statement_diagnostic(span, error.clone()))
                .unwrap_or(error);
            diagnostics.push(error);
        }
    }
}

fn locate_statement_diagnostic(statement: &crate::ast::StmtSpan, error: Diagnostic) -> Diagnostic {
    let quoted = error
        .message
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    statement
        .references
        .iter()
        .filter(|reference| quoted.iter().any(|token| reference.text == *token))
        .min_by_key(|reference| {
            (
                reference
                    .span
                    .end
                    .line
                    .saturating_sub(reference.span.start.line),
                reference
                    .span
                    .end
                    .column
                    .saturating_sub(reference.span.start.column),
            )
        })
        .map(|reference| error.clone().with_span_if_missing(reference.span.clone()))
        .unwrap_or_else(|| error.with_span_if_missing(statement.span.clone()))
}

fn validate_place_calls(
    place: &Place,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => validate_expr_calls(index, functions),
        Place::Access(path) => validate_access_calls(path, functions),
        Place::Ident(_) | Place::Field { .. } => Ok(()),
    }
}

fn validate_expr_calls(
    expr: &Expr,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Array(values) => {
            for value in values {
                validate_expr_calls(value, functions)?;
            }
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } | Expr::Deref(index) => {
            validate_expr_calls(index, functions)?;
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            validate_access_calls(path, functions)?;
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                validate_expr_calls(value, functions)?;
            }
        }
        Expr::Call { path, args } => {
            let name = path_text(path);
            validate_call_signature(&name, args.len(), functions)?;
            for arg in args {
                validate_expr_calls(arg, functions)?;
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => validate_expr_calls(expr, functions)?,
        Expr::Binary { left, right, .. } => {
            validate_expr_calls(left, functions)?;
            validate_expr_calls(right, functions)?;
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
    Ok(())
}

fn validate_access_calls(
    path: &AccessPath,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            validate_expr_calls(index, functions)?;
        }
    }
    Ok(())
}

fn validate_call_signature(
    name: &str,
    arity: usize,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    if let Some(expected) = builtin_function_arity(name) {
        if expected != arity {
            return Err(Diagnostic::new(builtin_arity_error(name)));
        }
        return Ok(());
    }
    let sig = functions
        .get(name)
        .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
    if sig.arity != arity {
        return Err(Diagnostic::new(format!(
            "function `{name}` expects {} arguments but got {arity}",
            sig.arity
        )));
    }
    Ok(())
}

fn builtin_function_arity(name: &str) -> Option<usize> {
    match name {
        "test.pass" | "ezra.test.pass" => Some(0),
        "test.fail" | "ezra.test.fail" => Some(1),
        "test.assert_eq_u8" | "ezra.test.assert_eq_u8" => Some(3),
        "test.assert_eq_u16" | "ezra.test.assert_eq_u16" => Some(3),
        "test.assert_eq_u24" | "ezra.test.assert_eq_u24" => Some(3),
        "debug.char" | "ezra.debug.char" => Some(1),
        "debug.str" | "ezra.debug.str" => Some(1),
        "debug.hex_u8" | "ezra.debug.hex_u8" => Some(1),
        "debug.hex_u16" | "ezra.debug.hex_u16" => Some(1),
        "debug.hex_u24" | "ezra.debug.hex_u24" => Some(1),
        "mem.poke8" | "ezra.mem.poke8" => Some(2),
        "mem.peek8" | "ezra.mem.peek8" => Some(1),
        "mem.memcpy" | "ezra.mem.memcpy" => Some(3),
        "mem.memset" | "ezra.mem.memset" => Some(3),
        _ => None,
    }
}

fn builtin_arity_error(name: &str) -> String {
    match name.strip_prefix("ezra.").unwrap_or(name) {
        "test.pass" => "test.pass requires no arguments".to_owned(),
        "test.fail" => "test.fail requires one argument".to_owned(),
        "test.assert_eq_u8" => "test.assert_eq_u8 requires three arguments".to_owned(),
        "test.assert_eq_u16" => "test.assert_eq_u16 requires three arguments".to_owned(),
        "test.assert_eq_u24" => "test.assert_eq_u24 requires three arguments".to_owned(),
        "debug.char" => "debug.char requires one argument".to_owned(),
        "debug.str" => "debug.str requires one argument".to_owned(),
        "debug.hex_u8" => "debug.hex_u8 requires one argument".to_owned(),
        "debug.hex_u16" => "debug.hex_u16 requires one argument".to_owned(),
        "debug.hex_u24" => "debug.hex_u24 requires one argument".to_owned(),
        "mem.poke8" => "mem.poke8 requires two arguments".to_owned(),
        "mem.peek8" => "mem.peek8 requires one argument".to_owned(),
        "mem.memcpy" => "mem.memcpy requires three arguments".to_owned(),
        "mem.memset" => "mem.memset requires three arguments".to_owned(),
        builtin => format!("{builtin} has invalid argument count"),
    }
}

fn inline_return_body(function: &Function) -> Option<(&[Stmt], Expr)> {
    let (last, prefix) = function.body.split_last()?;
    if prefix.iter().any(stmt_contains_return) {
        return None;
    }
    match last {
        Stmt::Return(Some(expr)) => Some((prefix, expr.clone())),
        _ => None,
    }
}

fn inline_void_body(function: &Function) -> Option<&[Stmt]> {
    if function.return_type.is_some() {
        return None;
    }
    match function.body.split_last() {
        Some((Stmt::Return(None), prefix)) if !prefix.iter().any(stmt_contains_return) => {
            Some(prefix)
        }
        _ if !function.body.iter().any(stmt_contains_return) => Some(&function.body),
        _ => None,
    }
}

fn is_inlinable_function(function: &Function) -> bool {
    has_attr(function, "inline")
        && (inline_return_body(function).is_some() || inline_void_body(function).is_some())
}

fn stmt_contains_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            then_body.iter().any(stmt_contains_return) || else_body.iter().any(stmt_contains_return)
        }
        Stmt::While { body, .. } | Stmt::Loop { body } => body.iter().any(stmt_contains_return),
        Stmt::Let { .. }
        | Stmt::Assign { .. }
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Asm { .. }
        | Stmt::Out { .. }
        | Stmt::Expr(_) => false,
    }
}

fn reachable_function_names(program: &Program, symbols: &Symbols) -> HashSet<String> {
    let mut graph = HashMap::new();
    let mut seeds = Vec::new();
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        let calls = reachable_calls_for_body(&function.body, symbols);
        graph.insert(function.name.clone(), calls);
        if function.name == "main" || has_attr(function, "naked") || has_attr(function, "interrupt")
        {
            seeds.push(function.name.clone());
        }
    }

    let mut reachable = HashSet::new();
    let mut stack = seeds;
    while let Some(name) = stack.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        if let Some(calls) = graph.get(&name) {
            stack.extend(calls.iter().cloned());
        }
    }
    reachable
}

fn reachable_calls_for_body(stmts: &[Stmt], symbols: &Symbols) -> Vec<String> {
    reachable_calls_for_body_with_inline_stack(stmts, symbols, &mut Vec::new())
}

fn reachable_calls_for_body_with_inline_stack(
    stmts: &[Stmt],
    symbols: &Symbols,
    inline_stack: &mut Vec<String>,
) -> Vec<String> {
    let mut raw_calls = Vec::new();
    collect_reachable_stmt_calls(stmts, &mut raw_calls, symbols);
    let mut calls = Vec::new();
    for name in raw_calls {
        if !symbols.functions.contains_key(&name) {
            continue;
        }
        if let Some(inline) = symbols.inline_functions.get(&name) {
            if inline_stack.iter().any(|inline_name| inline_name == &name) {
                calls.push(name);
            } else {
                inline_stack.push(name);
                calls.extend(reachable_calls_for_body_with_inline_stack(
                    &inline.body,
                    symbols,
                    inline_stack,
                ));
                inline_stack.pop();
            }
        } else {
            calls.push(name);
        }
    }
    calls
}

fn collect_stmt_calls(stmts: &[Stmt], calls: &mut Vec<String>) {
    collect_stmt_calls_with_symbols(stmts, calls, None)
}

fn collect_reachable_stmt_calls(stmts: &[Stmt], calls: &mut Vec<String>, symbols: &Symbols) {
    collect_stmt_calls_with_symbols(stmts, calls, Some(symbols))
}

fn collect_stmt_calls_with_symbols(
    stmts: &[Stmt],
    calls: &mut Vec<String>,
    symbols: Option<&Symbols>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } => collect_expr_calls(value, calls),
            Stmt::Assign { target, value, .. } => {
                collect_place_calls(target, calls);
                collect_expr_calls(value, calls);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_calls(condition, calls);
                if let Some(symbols) = symbols
                    && let Ok(value) = symbols.eval_i64(condition)
                {
                    if value == 0 {
                        collect_stmt_calls_with_symbols(else_body, calls, Some(symbols));
                    } else {
                        collect_stmt_calls_with_symbols(then_body, calls, Some(symbols));
                    }
                    if stmt_terminates_current_block(stmt) {
                        break;
                    }
                    continue;
                }
                collect_stmt_calls_with_symbols(then_body, calls, symbols);
                collect_stmt_calls_with_symbols(else_body, calls, symbols);
            }
            Stmt::While { condition, body } => {
                collect_expr_calls(condition, calls);
                if let Some(symbols) = symbols
                    && symbols.eval_i64(condition).is_ok_and(|value| value == 0)
                {
                    if stmt_terminates_current_block(stmt) {
                        break;
                    }
                    continue;
                }
                collect_stmt_calls_with_symbols(body, calls, symbols);
            }
            Stmt::Loop { body } => collect_stmt_calls_with_symbols(body, calls, symbols),
            Stmt::Return(Some(expr)) | Stmt::Expr(expr) => collect_expr_calls(expr, calls),
            Stmt::Out { value, .. } => collect_expr_calls(value, calls),
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => {}
        }
        if stmt_terminates_current_block(stmt) {
            break;
        }
    }
}

fn collect_place_calls(place: &Place, calls: &mut Vec<String>) {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => collect_expr_calls(index, calls),
        Place::Access(path) => collect_access_calls(path, calls),
        Place::Ident(_) | Place::Field { .. } => {}
    }
}

fn collect_expr_calls(expr: &Expr, calls: &mut Vec<String>) {
    match expr {
        Expr::Array(values) => {
            for value in values {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } | Expr::Deref(index) => {
            collect_expr_calls(index, calls);
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => collect_access_calls(path, calls),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Call { path, args } => {
            calls.push(path_text(path));
            for arg in args {
                collect_expr_calls(arg, calls);
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => collect_expr_calls(expr, calls),
        Expr::Binary { left, right, .. } => {
            collect_expr_calls(left, calls);
            collect_expr_calls(right, calls);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
}

fn addr24(addr: i64) -> Option<u32> {
    if (0..=0xFF_FFFF).contains(&addr) {
        Some(addr as u32)
    } else {
        None
    }
}

fn collect_access_calls(path: &AccessPath, calls: &mut Vec<String>) {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            collect_expr_calls(index, calls);
        }
    }
}

fn const_shl_or_zero(left: i64, right: i64) -> i64 {
    if !(0..64).contains(&right) {
        0
    } else {
        left.wrapping_shl(right as u32)
    }
}

fn const_shr_or_zero(left: i64, right: i64, signed: bool) -> i64 {
    if right < 0 {
        return 0;
    }
    if signed {
        if right >= 64 {
            if left < 0 { -1 } else { 0 }
        } else {
            left >> right as u32
        }
    } else if right >= 64 {
        0
    } else {
        left.wrapping_shr(right as u32)
    }
}

fn path_text(path: &[String]) -> String {
    path.join(".")
}

fn module_alias_original_name(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(_, original)| original)
}

fn function_label(name: &str) -> String {
    let mut label = String::from("_");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            label.push(ch);
        } else {
            label.push('_');
        }
    }
    label
}

fn reserved_function_label(label: &str) -> bool {
    matches!(
        label,
        "__ezra_start"
            | "__ezra_exit"
            | "__ezra_pass"
            | "__ezra_fail"
            | "__ezra_memcpy"
            | "__ezra_memset"
            | "__ezra_mul_u8"
            | "__ezra_mul_u16"
            | "__ezra_mul_u24"
            | "__ezra_mul_i24"
            | "__ezra_div_u8"
            | "__ezra_div_u16"
            | "__ezra_div_u24"
            | "__ezra_div_i24"
            | "__ezra_mod_u8"
            | "__ezra_mod_u16"
            | "__ezra_mod_u24"
            | "__ezra_mod_i24"
    )
}

fn scalar_var(addr: u32, size: u32) -> Variable {
    Variable {
        addr,
        size,
        element_size: None,
        len: None,
    }
}

fn storage_ranges_overlap(left: Variable, right: Variable) -> bool {
    let left_start = u64::from(left.addr);
    let left_end = left_start + u64::from(left.size);
    let right_start = u64::from(right.addr);
    let right_end = right_start + u64::from(right.size);
    left_start < right_end && right_start < left_end
}

fn declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Cfg { declaration, .. } => declaration_name(declaration),
        Declaration::Import(_) => None,
        Declaration::Const(decl) => Some(&decl.name),
        Declaration::Alias(decl) => Some(&decl.name),
        Declaration::Port(decl) => Some(&decl.name),
        Declaration::Mmio(decl) => Some(&decl.name),
        Declaration::Embed(decl) => Some(&decl.name),
        Declaration::Global(decl) => Some(&decl.name),
        Declaration::Struct(decl) => Some(&decl.name),
        Declaration::ExternAsmFunction(decl) => Some(&decl.name),
        Declaration::Function(decl) => Some(&decl.name),
    }
}

fn function_declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::ExternAsmFunction(decl) => Some(&decl.name),
        Declaration::Function(decl) => Some(&decl.name),
        _ => None,
    }
}

fn find_const_declaration<'a>(
    program: &'a Program,
    name: &str,
) -> Option<&'a crate::ast::ConstDecl> {
    program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Const(decl) if decl.name == name => Some(decl),
            _ => None,
        })
}

fn collect_const_dependency_names(expr: &Expr, names: &mut Vec<String>) {
    match expr {
        Expr::Ident(name) => names.push(name.clone()),
        Expr::Field { base, field } => names.push(format!("{base}.{field}")),
        Expr::Access(path) => {
            if let Ok(name) = const_access_name(path) {
                names.push(name);
            }
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_dependency_names(index, names);
                }
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::Deref(expr) => {
            collect_const_dependency_names(expr, names)
        }
        Expr::Binary { left, right, .. } => {
            collect_const_dependency_names(left, names);
            collect_const_dependency_names(right, names);
        }
        Expr::Array(values) => {
            for value in values {
                collect_const_dependency_names(value, names);
            }
        }
        Expr::Index { index, .. } => collect_const_dependency_names(index, names),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_const_dependency_names(value, names);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_const_dependency_names(arg, names);
            }
        }
        Expr::AddressOfIndex { index, .. } => collect_const_dependency_names(index, names),
        Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_dependency_names(index, names);
                }
            }
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::In(_)
        | Expr::AddressOf(_)
        | Expr::AddressOfField { .. } => {}
    }
}

fn collect_const_address_roots(expr: &Expr, roots: &mut Vec<String>) {
    match expr {
        Expr::AddressOf(name) => roots.push(name.clone()),
        Expr::AddressOfIndex { name, index } => {
            roots.push(name.clone());
            collect_const_address_roots(index, roots);
        }
        Expr::AddressOfField { base, .. } => roots.push(base.clone()),
        Expr::AddressOfAccess(path) => {
            roots.push(path.root.clone());
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_address_roots(index, roots);
                }
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::Deref(expr) => {
            collect_const_address_roots(expr, roots)
        }
        Expr::Binary { left, right, .. } => {
            collect_const_address_roots(left, roots);
            collect_const_address_roots(right, roots);
        }
        Expr::Array(values) => {
            for value in values {
                collect_const_address_roots(value, roots);
            }
        }
        Expr::Index { index, .. } => {
            collect_const_address_roots(index, roots);
        }
        Expr::Access(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_address_roots(index, roots);
                }
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_const_address_roots(value, roots);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_const_address_roots(arg, roots);
            }
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. } => {}
    }
}

fn has_attr(function: &Function, attr: &str) -> bool {
    function.attrs.iter().any(|candidate| candidate == attr)
}

fn validate_function_attrs(function: &Function) -> Result<(), Diagnostic> {
    let mut seen = HashSet::new();
    for attr in &function.attrs {
        if !seen.insert(attr.as_str()) {
            return Err(Diagnostic::new(format!(
                "duplicate attribute `{attr}` on function `{}`",
                function.name
            )));
        }
    }
    Ok(())
}

fn block_guarantees_value_return(stmts: &[Stmt], symbols: &Symbols) -> bool {
    stmts
        .iter()
        .any(|stmt| stmt_guarantees_value_return(stmt, symbols))
}

fn stmt_guarantees_value_return(stmt: &Stmt, symbols: &Symbols) -> bool {
    match stmt {
        Stmt::Return(Some(_)) => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } if !else_body.is_empty() => {
            block_guarantees_value_return(then_body, symbols)
                && block_guarantees_value_return(else_body, symbols)
        }
        Stmt::Loop { body } => {
            !block_can_break_current_loop(body) && block_guarantees_value_return(body, symbols)
        }
        Stmt::While { condition, body } if condition_is_const_true(condition, symbols) => {
            !block_can_break_current_loop(body) && block_guarantees_value_return(body, symbols)
        }
        _ => false,
    }
}

fn condition_is_const_true(condition: &Expr, symbols: &Symbols) -> bool {
    matches!(condition, Expr::Bool(true))
        || symbols.eval_i64(condition).is_ok_and(|value| value != 0)
}

fn block_terminates_current_block(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_terminates_current_block)
}

fn stmt_terminates_current_block(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::Break | Stmt::Continue => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } if !else_body.is_empty() => {
            block_terminates_current_block(then_body) && block_terminates_current_block(else_body)
        }
        Stmt::Loop { body } => {
            !block_can_break_current_loop(body) && block_terminates_current_block(body)
        }
        Stmt::While {
            condition: Expr::Bool(true),
            body,
        } => !block_can_break_current_loop(body) && block_terminates_current_block(body),
        _ => false,
    }
}

fn assigned_names_in_block(stmts: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_assigned_names(stmts, &mut names);
    names
}

fn assigned_names_in_program(program: &Program) -> HashSet<String> {
    let mut names = HashSet::new();
    for declaration in &program.declarations {
        if let Declaration::Function(function) = declaration {
            collect_assigned_names(&function.body, &mut names);
        }
    }
    names
}

fn collect_assigned_names(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { target, .. } => collect_assigned_place(target, names),
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_assigned_names(then_body, names);
                collect_assigned_names(else_body, names);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => collect_assigned_names(body, names),
            _ => {}
        }
    }
}

fn collect_assigned_place(place: &Place, names: &mut HashSet<String>) {
    if let Place::Ident(name) = place {
        names.insert(name.clone());
    }
}

fn block_can_break_current_loop(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_can_break_current_loop)
}

fn stmt_can_break_current_loop(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Break => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => block_can_break_current_loop(then_body) || block_can_break_current_loop(else_body),
        Stmt::While { .. } | Stmt::Loop { .. } => false,
        _ => false,
    }
}

fn stmt_summary(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Let { name, ty, value } => {
            format!("let {name}: {} = {}", type_display(ty), expr_summary(value))
        }
        Stmt::Assign { target, op, value } => {
            format!(
                "{} {} {}",
                place_summary(target),
                assign_op_summary(*op),
                expr_summary(value)
            )
        }
        Stmt::If { condition, .. } => format!("if {}", expr_summary(condition)),
        Stmt::While { condition, .. } => format!("while {}", expr_summary(condition)),
        Stmt::Loop { .. } => "loop".to_owned(),
        Stmt::Break => "break".to_owned(),
        Stmt::Continue => "continue".to_owned(),
        Stmt::Return(Some(expr)) => format!("return {}", expr_summary(expr)),
        Stmt::Return(None) => "return".to_owned(),
        Stmt::Asm { volatile, .. } => {
            if *volatile {
                "asm volatile".to_owned()
            } else {
                "asm".to_owned()
            }
        }
        Stmt::Out { port, value } => format!("out {port}, {}", expr_summary(value)),
        Stmt::Expr(expr) => expr_summary(expr),
    }
}

fn place_summary(place: &Place) -> String {
    match place {
        Place::Ident(name) => name.clone(),
        Place::Index { name, index } => format!("{name}[{}]", expr_summary(index)),
        Place::Field { base, field } => format!("{base}.{field}"),
        Place::Access(path) => access_path_summary(path),
        Place::Deref(expr) => format!("*{}", expr_summary(expr)),
    }
}

fn expr_summary(expr: &Expr) -> String {
    match expr {
        Expr::Int(value) => value.to_string(),
        Expr::TypedInt(value, ty) => format!("{value}{}", type_display(ty)),
        Expr::Bool(value) => value.to_string(),
        Expr::Char(value) => format!("'{}'", char::from(*value).escape_default()),
        Expr::String(value) => format!("{value:?}"),
        Expr::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(expr_summary)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Ident(name) => name.clone(),
        Expr::In(port) => format!("in {port}"),
        Expr::Index { name, index } => format!("{name}[{}]", expr_summary(index)),
        Expr::Field { base, field } => format!("{base}.{field}"),
        Expr::AddressOfIndex { name, index } => format!("&{name}[{}]", expr_summary(index)),
        Expr::AddressOfField { base, field } => format!("&{base}.{field}"),
        Expr::Access(path) => access_path_summary(path),
        Expr::AddressOfAccess(path) => format!("&{}", access_path_summary(path)),
        Expr::AddressOf(name) => format!("&{name}"),
        Expr::StructInit { ty, fields } => format!(
            "{ty} {{ {} }}",
            fields
                .iter()
                .map(|(name, value)| format!("{name}: {}", expr_summary(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Deref(expr) => format!("*{}", expr_summary(expr)),
        Expr::Call { path, args } => format!(
            "{}({})",
            path_text(path),
            args.iter().map(expr_summary).collect::<Vec<_>>().join(", ")
        ),
        Expr::Unary { op, expr } => format!("{}{}", unary_op_summary(*op), expr_summary(expr)),
        Expr::Binary { left, op, right } => format!(
            "{} {} {}",
            expr_summary(left),
            binary_op_summary(*op),
            expr_summary(right)
        ),
        Expr::Cast { ty, expr } => format!("cast<{}>({})", type_display(ty), expr_summary(expr)),
    }
}

fn access_path_summary(path: &AccessPath) -> String {
    let mut out = path.root.clone();
    for segment in &path.segments {
        match segment {
            AccessSegment::Field(field) => {
                out.push('.');
                out.push_str(field);
            }
            AccessSegment::Index(index) => {
                out.push('[');
                out.push_str(&expr_summary(index));
                out.push(']');
            }
        }
    }
    out
}

fn const_access_name(path: &AccessPath) -> Result<String, Diagnostic> {
    if path
        .segments
        .iter()
        .all(|segment| matches!(segment, AccessSegment::Field(_)))
    {
        Ok(access_path_summary(path))
    } else {
        Err(Diagnostic::new(
            "expression is not supported in a constant declaration",
        ))
    }
}

fn assign_op_summary(op: AssignOp) -> &'static str {
    match op {
        AssignOp::Set => "=",
        AssignOp::Add => "+=",
        AssignOp::Sub => "-=",
        AssignOp::Mul => "*=",
        AssignOp::Div => "/=",
        AssignOp::Mod => "%=",
        AssignOp::BitAnd => "&=",
        AssignOp::BitOr => "|=",
        AssignOp::BitXor => "^=",
        AssignOp::Shl => "<<=",
        AssignOp::Shr => ">>=",
    }
}

fn unary_op_summary(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::BitNot => "~",
        UnaryOp::Not => "!",
    }
}

fn binary_op_summary(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitXor => "^",
        BinaryOp::BitOr => "|",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

fn type_display(ty: &Type) -> String {
    match ty {
        Type::Named(name) => name.clone(),
        Type::Ptr(inner) => format!("ptr<{}>", type_display(inner)),
        Type::Array { element, len } => {
            format!("[{}; {}]", type_display(element), expr_summary(len))
        }
    }
}

fn type_is_signed(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if matches!(name.as_str(), "i8" | "i16" | "i24"))
}

fn struct_scalar_type<'a>(
    ty: &'a Type,
    structs: &HashMap<String, StructLayout>,
) -> Option<&'a str> {
    match ty {
        Type::Named(name) if structs.contains_key(name) => Some(name.as_str()),
        _ => None,
    }
}

fn signed_min_bytes(width: ValueWidth) -> &'static [u8] {
    match width {
        ValueWidth::U8 => &[0x80],
        ValueWidth::U16 => &[0x00, 0x80],
        ValueWidth::U24 => &[0x00, 0x00, 0x80],
    }
}

fn signed_negative_one_bytes(width: ValueWidth) -> &'static [u8] {
    match width {
        ValueWidth::U8 => &[0xFF],
        ValueWidth::U16 => &[0xFF, 0xFF],
        ValueWidth::U24 => &[0xFF, 0xFF, 0xFF],
    }
}

fn type_is_bool(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if name == "bool")
}

fn validate_integer_unary_operand_type(ty: &Type) -> Result<(), Diagnostic> {
    if type_is_bool(ty) || matches!(ty, Type::Ptr(_)) {
        return Err(Diagnostic::new("unary operand must be an integer"));
    }
    match ty {
        Type::Named(name)
            if matches!(name.as_str(), "u8" | "i8" | "u16" | "i16" | "u24" | "i24") =>
        {
            Ok(())
        }
        Type::Named(name) if name == "ptr24" => {
            Err(Diagnostic::new("unary operand must be an integer"))
        }
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
            Err(Diagnostic::new(format!(
                "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
            )))
        }
        Type::Named(name) => Err(Diagnostic::new(format!("unknown type `{name}`"))),
        Type::Array { .. } => Err(Diagnostic::new("unary operand must be an integer")),
        Type::Ptr(_) => Err(Diagnostic::new("unary operand must be an integer")),
    }
}

fn validate_shift_operand_type(ty: &Type) -> Result<(), Diagnostic> {
    if type_is_bool(ty) || matches!(ty, Type::Ptr(_)) {
        return Err(Diagnostic::new("shift operand must be an integer"));
    }
    match ty {
        Type::Named(name)
            if matches!(name.as_str(), "u8" | "i8" | "u16" | "i16" | "u24" | "i24") =>
        {
            Ok(())
        }
        Type::Named(name) if name == "ptr24" => {
            Err(Diagnostic::new("shift operand must be an integer"))
        }
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
            Err(Diagnostic::new(format!(
                "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
            )))
        }
        Type::Named(name) => Err(Diagnostic::new(format!("unknown type `{name}`"))),
        Type::Array { .. } => Err(Diagnostic::new("shift operand must be an integer")),
        Type::Ptr(_) => Err(Diagnostic::new("shift operand must be an integer")),
    }
}

fn validate_shift_count_integer_type(ty: &Type) -> Result<(), Diagnostic> {
    if type_is_bool(ty) || matches!(ty, Type::Ptr(_)) {
        return Err(Diagnostic::new("shift count must be an integer"));
    }
    match ty {
        Type::Named(name)
            if matches!(name.as_str(), "u8" | "i8" | "u16" | "i16" | "u24" | "i24") =>
        {
            Ok(())
        }
        Type::Named(name) if name == "ptr24" => {
            Err(Diagnostic::new("shift count must be an integer"))
        }
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
            Err(Diagnostic::new(format!(
                "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
            )))
        }
        Type::Named(name) => Err(Diagnostic::new(format!("unknown type `{name}`"))),
        Type::Array { .. } => Err(Diagnostic::new("shift count must be an integer")),
        Type::Ptr(_) => Err(Diagnostic::new("shift count must be an integer")),
    }
}

fn validate_runtime_shift_count_type(ty: &Type) -> Result<(), Diagnostic> {
    match ty {
        Type::Named(name) if name == "u8" => Ok(()),
        _ => Err(Diagnostic::new("runtime shift count must be u8")),
    }
}

fn ptr_u8_type() -> Type {
    Type::Ptr(Box::new(Type::Named("u8".to_owned())))
}

fn width_unsigned_type(width: ValueWidth) -> Type {
    let name = match width {
        ValueWidth::U8 => "u8",
        ValueWidth::U16 => "u16",
        ValueWidth::U24 => "u24",
    };
    Type::Named(name.to_owned())
}

fn is_raw_address_type(name: &str) -> bool {
    matches!(name, "u24" | "ptr24")
}

fn validate_comparison_types<F>(
    left_type: &Type,
    op: BinaryOp,
    right_type: &Type,
    widths: F,
) -> Result<(), Diagnostic>
where
    F: FnOnce() -> Option<(ValueWidth, ValueWidth)>,
{
    if matches!(left_type, Type::Array { .. }) || matches!(right_type, Type::Array { .. }) {
        return Err(Diagnostic::new("array value cannot be used as a scalar"));
    }
    if type_is_bool(left_type) || type_is_bool(right_type) {
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && left_type == right_type {
            return Ok(());
        }
        return Err(Diagnostic::new("type mismatch"));
    }

    let left_is_ptr = matches!(left_type, Type::Ptr(_));
    let right_is_ptr = matches!(right_type, Type::Ptr(_));
    if left_is_ptr || right_is_ptr {
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && left_type == right_type {
            return Ok(());
        }
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            return Err(Diagnostic::new("type mismatch"));
        }
        if left_is_ptr && right_is_ptr {
            return Err(Diagnostic::new(
                "pointer comparisons support only == and !=",
            ));
        }
        return Err(Diagnostic::new("type mismatch"));
    }

    if type_is_signed(left_type) != type_is_signed(right_type) {
        return Err(Diagnostic::new("signed/unsigned mix without cast"));
    }
    if let Some((left_width, right_width)) = widths()
        && left_width != right_width
    {
        return Err(Diagnostic::new(
            "comparison operands must have same width without cast",
        ));
    }
    Ok(())
}

fn int_value_type(value: i64) -> Type {
    if (0..=0xFF).contains(&value) {
        Type::Named("u8".to_owned())
    } else if (0..=0xFFFF).contains(&value) {
        Type::Named("u16".to_owned())
    } else {
        Type::Named("u24".to_owned())
    }
}

fn expr_is_untyped_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_) | Expr::Char(_) => true,
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => matches!(expr.as_ref(), Expr::Int(_)),
        _ => false,
    }
}

fn format_immediate(value: i64, width: ValueWidth) -> String {
    match width {
        ValueWidth::U8 => format!("{:02X}h", (value as u64) & 0xFF),
        ValueWidth::U16 => format!("{:04X}h", (value as u64) & 0xFFFF),
        ValueWidth::U24 => format!("{:06X}h", (value as u64) & 0xFF_FFFF),
    }
}

fn validate_main_signature(main: &Function) -> Result<(), Diagnostic> {
    if !main.params.is_empty() {
        return Err(Diagnostic::new("main function cannot take parameters"));
    }
    if main.return_type.is_some() {
        return Err(Diagnostic::new("main function cannot return a value"));
    }
    Ok(())
}

fn validate_inline_asm_clobbers(
    clobbers: &[String],
    lines: &[String],
    allow_sp_clobber: bool,
    cpu: AssemblerCpu,
) -> Result<(), Diagnostic> {
    let mut seen = HashSet::new();
    for clobber in clobbers {
        if !is_allowed_inline_asm_clobber(clobber) {
            return Err(Diagnostic::new(format!(
                "unknown inline asm clobber `{clobber}`"
            )));
        }
        if !seen.insert(clobber.as_str()) {
            return Err(Diagnostic::new(format!(
                "duplicate inline asm clobber `{clobber}`"
            )));
        }
    }
    if asm_clobbers_include(clobbers, "sp") && !allow_sp_clobber {
        return Err(Diagnostic::new(
            "inline asm clobber `sp` is only allowed in naked functions",
        ));
    }
    for line in lines {
        if is_unsupported_z80_family_instruction(cpu, line)? {
            return Err(Diagnostic::new(format!(
                "test assembler does not support instruction `{}`",
                line.trim()
            )));
        }
        let effects = analyze_instruction(cpu, line)?.effects;
        for register in effects.referenced_special_registers {
            if !asm_clobbers_include(clobbers, register) {
                return Err(Diagnostic::new(format!(
                    "inline asm uses `{register}` without declaring clobber `{register}`"
                )));
            }
        }
        if effects.uses_ports && !asm_clobbers_include(clobbers, "ports") {
            return Err(Diagnostic::new(
                "inline asm uses ports without declaring clobber `ports`",
            ));
        }
        if effects.changes_flags && !asm_clobbers_include_flags(clobbers) {
            return Err(Diagnostic::new(
                "inline asm changes flags without declaring clobber `flags`",
            ));
        }
        if effects.uses_memory && !asm_clobbers_include(clobbers, "memory") {
            return Err(Diagnostic::new(
                "inline asm uses memory without declaring clobber `memory`",
            ));
        }
        for register in effects.modified_registers {
            if !asm_clobbers_include_register(clobbers, register) {
                return Err(Diagnostic::new(format!(
                    "inline asm modifies `{register}` without declaring clobber `{register}`"
                )));
            }
        }
    }
    Ok(())
}

fn is_allowed_inline_asm_clobber(clobber: &str) -> bool {
    matches!(
        clobber,
        "a" | "f"
            | "af"
            | "b"
            | "c"
            | "bc"
            | "d"
            | "e"
            | "de"
            | "h"
            | "l"
            | "hl"
            | "ix"
            | "iy"
            | "sp"
            | "memory"
            | "ports"
            | "flags"
    )
}

fn asm_clobbers_include(clobbers: &[String], name: &str) -> bool {
    clobbers.iter().any(|clobber| clobber == name)
}

fn asm_clobbers_include_flags(clobbers: &[String]) -> bool {
    asm_clobbers_include(clobbers, "flags")
        || asm_clobbers_include(clobbers, "f")
        || asm_clobbers_include(clobbers, "af")
}

fn asm_clobbers_include_register(clobbers: &[String], register: &str) -> bool {
    if asm_clobbers_include(clobbers, register) {
        return true;
    }
    match register {
        "a" | "f" => asm_clobbers_include(clobbers, "af"),
        "b" | "c" => asm_clobbers_include(clobbers, "bc"),
        "d" | "e" => asm_clobbers_include(clobbers, "de"),
        "h" | "l" => asm_clobbers_include(clobbers, "hl"),
        "af" => {
            asm_clobbers_include(clobbers, "a")
                && (asm_clobbers_include(clobbers, "f") || asm_clobbers_include(clobbers, "flags"))
        }
        "bc" => asm_clobbers_include(clobbers, "b") && asm_clobbers_include(clobbers, "c"),
        "de" => asm_clobbers_include(clobbers, "d") && asm_clobbers_include(clobbers, "e"),
        "hl" => asm_clobbers_include(clobbers, "h") && asm_clobbers_include(clobbers, "l"),
        _ => false,
    }
}

fn substitute_inline_asm_operands(
    line: &str,
    operands: &HashMap<String, String>,
) -> Result<String, Diagnostic> {
    let mut output = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('}') else {
            return Err(Diagnostic::new(format!(
                "unterminated inline asm operand placeholder in `{line}`"
            )));
        };
        let name = &after_start[..end];
        let Some(binding) = operands.get(name) else {
            return Err(Diagnostic::new(format!(
                "unknown inline asm operand placeholder `{name}`"
            )));
        };
        output.push_str(binding);
        rest = &after_start[end + 1..];
    }
    if rest.contains('}') {
        return Err(Diagnostic::new(format!(
            "unmatched inline asm operand placeholder in `{line}`"
        )));
    }
    output.push_str(rest);
    Ok(output)
}

fn is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    )
}

fn sdk_constants(options: &AssemblyOptions) -> HashMap<String, i64> {
    let mut constants = HashMap::from([
        ("EZRA_LOAD_ADDR".to_owned(), options.load_addr.get() as i64),
        (
            "EZRA_ENTRY_ADDR".to_owned(),
            options.entry_addr.get() as i64,
        ),
        ("EZRA_CODE_BASE".to_owned(), options.code_base.get() as i64),
        ("EZRA_STACK_TOP".to_owned(), options.stack_top.get() as i64),
        ("EZRA_RAM_BASE".to_owned(), options.ram_base.get() as i64),
        ("EZRA_VRAM_BASE".to_owned(), options.vram_base.get() as i64),
        (
            "EZRA_AUDIO_BASE".to_owned(),
            options.audio_base.get() as i64,
        ),
        (
            "EZRA_ASSET_BASE".to_owned(),
            options.asset_base.get() as i64,
        ),
        (
            "EZRA_RODATA_BASE".to_owned(),
            options.rodata_base.get() as i64,
        ),
    ]);
    if options.default_sdk_symbols {
        constants.extend([
            ("VRAM_BASE".to_owned(), options.vram_base.get() as i64),
            ("AUDIO_BASE".to_owned(), options.audio_base.get() as i64),
            ("BTN_B".to_owned(), 0x0001),
            ("BTN_Y".to_owned(), 0x0002),
            ("BTN_SELECT".to_owned(), 0x0004),
            ("BTN_START".to_owned(), 0x0008),
            ("BTN_UP".to_owned(), 0x0010),
            ("BTN_DOWN".to_owned(), 0x0020),
            ("BTN_LEFT".to_owned(), 0x0040),
            ("BTN_RIGHT".to_owned(), 0x0080),
            ("BTN_A".to_owned(), 0x0100),
            ("BTN_X".to_owned(), 0x0200),
            ("BTN_L".to_owned(), 0x0400),
            ("BTN_R".to_owned(), 0x0800),
            ("VIDEO_PRESENT".to_owned(), 1),
            ("VIDEO_CLEAR".to_owned(), 2),
            ("VIDEO_SET_MODE".to_owned(), 3),
            ("AUDIO_SUBMIT_BUFFER".to_owned(), 1),
            ("AUDIO_STOP".to_owned(), 2),
        ]);
    }
    constants
}

fn sdk_constant_types(options: &AssemblyOptions) -> HashMap<String, Type> {
    let mut types = HashMap::new();
    for name in [
        "EZRA_LOAD_ADDR",
        "EZRA_ENTRY_ADDR",
        "EZRA_CODE_BASE",
        "EZRA_STACK_TOP",
        "EZRA_RAM_BASE",
        "EZRA_VRAM_BASE",
        "EZRA_AUDIO_BASE",
        "EZRA_ASSET_BASE",
        "EZRA_RODATA_BASE",
    ] {
        types.insert(name.to_owned(), Type::Named("u24".to_owned()));
    }
    if !options.default_sdk_symbols {
        return types;
    }
    for name in ["VRAM_BASE", "AUDIO_BASE"] {
        types.insert(
            name.to_owned(),
            Type::Ptr(Box::new(Type::Named("u8".to_owned()))),
        );
    }
    for name in [
        "BTN_B",
        "BTN_Y",
        "BTN_SELECT",
        "BTN_START",
        "BTN_UP",
        "BTN_DOWN",
        "BTN_LEFT",
        "BTN_RIGHT",
        "BTN_A",
        "BTN_X",
        "BTN_L",
        "BTN_R",
    ] {
        types.insert(name.to_owned(), Type::Named("u16".to_owned()));
    }
    for name in [
        "VIDEO_PRESENT",
        "VIDEO_CLEAR",
        "VIDEO_SET_MODE",
        "AUDIO_SUBMIT_BUFFER",
        "AUDIO_STOP",
    ] {
        types.insert(name.to_owned(), Type::Named("u8".to_owned()));
    }
    types
}

fn sdk_ports(options: &AssemblyOptions) -> HashMap<String, u8> {
    if !options.default_sdk_symbols {
        return HashMap::new();
    }
    HashMap::from([
        ("PAD1_LO".to_owned(), 0x01),
        ("PAD1_HI".to_owned(), 0x02),
        ("PAD2_LO".to_owned(), 0x03),
        ("PAD2_HI".to_owned(), 0x04),
        ("PAD3_LO".to_owned(), 0x05),
        ("PAD3_HI".to_owned(), 0x06),
        ("PAD4_LO".to_owned(), 0x07),
        ("PAD4_HI".to_owned(), 0x08),
        ("VIDEO_CMD".to_owned(), 0x09),
        ("AUDIO_CMD".to_owned(), 0x0A),
        ("SYS_STATUS".to_owned(), 0x0B),
        ("DEBUG_CHAR".to_owned(), 0x0C),
        ("TEST_RESULT".to_owned(), 0x0D),
        ("TEST_HALT".to_owned(), 0x0E),
        ("EXT_ADDR0".to_owned(), 0x10),
        ("EXT_ADDR1".to_owned(), 0x11),
        ("EXT_ADDR2".to_owned(), 0x12),
        ("EXT_LEN0".to_owned(), 0x13),
        ("EXT_LEN1".to_owned(), 0x14),
        ("EXT_MODE".to_owned(), 0x15),
        ("EXT_COMMAND".to_owned(), 0x16),
        ("EXT_STATUS".to_owned(), 0x17),
    ])
}

fn read_embed_file(path: &str, source_path: &Path) -> Result<Vec<u8>, Diagnostic> {
    let path = Path::new(path);
    if path.is_absolute() {
        return read_embed_file_candidate(path);
    }

    let candidates = embed_file_candidates(path, source_path);
    let missing_path = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| path.to_path_buf());
    for candidate in candidates {
        match fs::read(&candidate) {
            Ok(bytes) => return Ok(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Diagnostic::new(format!(
                    "failed to read embedded file `{}`: {error}",
                    candidate.display()
                )));
            }
        }
    }
    Err(Diagnostic::new(format!(
        "embedded file `{}` not found",
        missing_path.display()
    )))
}

fn read_embed_file_candidate(path: &Path) -> Result<Vec<u8>, Diagnostic> {
    fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Diagnostic::new(format!("embedded file `{}` not found", path.display()))
        } else {
            Diagnostic::new(format!(
                "failed to read embedded file `{}`: {error}",
                path.display()
            ))
        }
    })
}

fn embed_file_candidates(path: &Path, source_path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![
        source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path),
    ];
    if let Ok(project_root) = std::env::current_dir() {
        let project_relative = project_root.join(path);
        if !candidates
            .iter()
            .any(|candidate| candidate == &project_relative)
        {
            candidates.push(project_relative);
        }
    }
    candidates
}

#[cfg(test)]
mod tests;
