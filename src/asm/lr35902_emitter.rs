use std::collections::{HashMap, HashSet};

use crate::{
    asm::{
        AssemblyOptions, GameBoyBankingMapper, GameBoyBankingOptions,
        comments::{stmt_summary, with_readability_comments},
        data::{ByteLiteralStyle, byte_data_lines},
        reachability::{RoutineProfile, strip_unreachable_generated_routines},
    },
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Declaration, Expr, Function, Place, Program,
        Stmt, Type, UnaryOp,
    },
    declaration::unwrapped_declaration,
    diagnostic::Diagnostic,
    hir::HirProgram,
    regalloc::{
        Location, PhysReg, PhysicalRegister, RegClass, RegUnit, RegisterClass, RegisterUnit,
        SpillClass, SpillClassId, Target,
        source::{SourceLocal, allocate_source_locals},
    },
    target::CpuFamily,
    tbir::{
        TbirProgram,
        model::{SemanticModel, Storage},
    },
};

// HRAM scratch avoids clobbering Game Boy I/O registers and is reachable through a16 loads.
const POINTER_ZP: u32 = 0xFF80;

pub fn emit_lr35902_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    if options.cpu != CpuFamily::Lr35902 {
        return Err(Diagnostic::new(
            "LR35902 emitter requires an LR35902 target",
        ));
    }
    if program.main_function().is_none() {
        return Err(Diagnostic::new(
            "LR35902 programs require a `main` function",
        ));
    }
    let hir = HirProgram::from_ast(program)?;
    let tbir = TbirProgram::lower(&hir, program, &options)?;
    let mut model = SemanticModel::from_program(
        &tbir.lowered_program,
        16,
        options.ram_base.get(),
        options.rodata_base.get(),
        options.asset_base.get(),
    )?;
    let banked_layout =
        configure_gameboy_banking(&tbir.lowered_program, &mut model, options.gameboy_banking)?;
    Emitter::new(model, options.gameboy_banking, banked_layout)
        .emit(&tbir.lowered_program)
        .map(|asm| {
            let asm = strip_unreachable_generated_routines(&asm, RoutineProfile::Lr35902);
            with_readability_comments(asm, program, &options, "lr35902", &tbir.source_comments)
        })
}

#[derive(Clone, Default)]
struct BankedLayout {
    data_addresses: HashMap<String, u32>,
    globals: HashSet<String>,
    functions: HashMap<String, u32>,
    data: HashMap<u32, Vec<(String, Vec<u8>)>>,
}

fn configure_gameboy_banking(
    program: &Program,
    model: &mut SemanticModel,
    banking: Option<GameBoyBankingOptions>,
) -> Result<BankedLayout, Diagnostic> {
    let mut declarations = Vec::new();
    collect_banked_declarations(&program.declarations, None, &mut declarations);
    let has_banked_declarations = declarations.iter().any(|(bank, _)| bank.is_some());
    if has_banked_declarations && banking.is_none() {
        return Err(Diagnostic::new(
            "Game Boy `@cfg(bank(N))` declarations require `[banking] enabled = true`",
        ));
    }

    let Some(banking) = banking else {
        validate_banked_pointer_contexts(&declarations, None)?;
        return Ok(BankedLayout::default());
    };
    validate_banked_pointer_contexts(&declarations, Some(banking))?;
    let maximum_bank = match banking.mapper {
        GameBoyBankingMapper::Mbc1 => 127,
        GameBoyBankingMapper::Mbc5 => 511,
    };
    let mut layout = BankedLayout::default();
    let mut cursors = HashMap::<u32, u32>::new();
    for (bank, declaration) in declarations {
        let Some(bank) = bank else {
            continue;
        };
        if bank == 0
            || bank > maximum_bank
            || (banking.mapper == GameBoyBankingMapper::Mbc1 && bank & 0x1F == 0)
        {
            return Err(Diagnostic::new(format!(
                "Game Boy mapper cannot place a declaration in ROM bank {bank}"
            )));
        }
        match declaration {
            Declaration::Embed(embed) => {
                if embed.align.is_some() {
                    return Err(Diagnostic::new(format!(
                        "banked Game Boy embed `{}` cannot specify `align`",
                        embed.name
                    )));
                }
                let bytes = model
                    .embeds
                    .get(&embed.name)
                    .ok_or_else(|| {
                        Diagnostic::new(format!(
                            "missing banked embed `{}` in semantic model",
                            embed.name
                        ))
                    })?
                    .bytes
                    .clone();
                let address =
                    place_banked_data(&mut layout, &mut cursors, bank, &embed.name, bytes)?;
                let object = model
                    .embeds
                    .get_mut(&embed.name)
                    .expect("banked embed exists");
                object.storage.address = address;
                model
                    .constants
                    .insert(format!("{}.ptr", embed.name), i64::from(address));
                model.constants.insert(
                    format!("{}.end", embed.name),
                    i64::from(address + object.bytes.len() as u32),
                );
            }
            Declaration::Global(global) => {
                let bytes = banked_global_bytes(model, &global.ty, &global.value)?;
                let address =
                    place_banked_data(&mut layout, &mut cursors, bank, &global.name, bytes)?;
                let storage = model.globals.get_mut(&global.name).ok_or_else(|| {
                    Diagnostic::new(format!(
                        "missing banked global `{}` in semantic model",
                        global.name
                    ))
                })?;
                storage.address = address;
                layout.globals.insert(global.name.clone());
            }
            Declaration::Function(function) => {
                if function.name == "main" {
                    return Err(Diagnostic::new(
                        "Game Boy `main` must remain resident in ROM bank 0",
                    ));
                }
                layout.functions.insert(function.name.clone(), bank);
            }
            _ => {
                return Err(Diagnostic::new(
                    "Game Boy `@cfg(bank(N))` supports only `embed`, `global`, and `fn` declarations",
                ));
            }
        }
    }
    Ok(layout)
}

fn place_banked_data(
    layout: &mut BankedLayout,
    cursors: &mut HashMap<u32, u32>,
    bank: u32,
    name: &str,
    bytes: Vec<u8>,
) -> Result<u32, Diagnostic> {
    let cursor = cursors.entry(bank).or_insert(0);
    let size = u32::try_from(bytes.len())
        .map_err(|_| Diagnostic::new(format!("banked declaration `{name}` is too large")))?;
    let end = cursor.checked_add(size).ok_or_else(|| {
        Diagnostic::new(format!(
            "banked declaration `{name}` exceeds Game Boy ROM bank size"
        ))
    })?;
    if end > 0x4000 {
        return Err(Diagnostic::new(format!(
            "banked declarations in Game Boy ROM bank {bank} exceed its 16 KiB window"
        )));
    }
    let address = 0x4000 + *cursor;
    layout.data_addresses.insert(name.to_owned(), address);
    layout
        .data
        .entry(bank)
        .or_default()
        .push((name.to_owned(), bytes));
    *cursor = end;
    Ok(address)
}

fn banked_global_bytes(
    model: &SemanticModel,
    ty: &Type,
    value: &Expr,
) -> Result<Vec<u8>, Diagnostic> {
    let resolved = model.resolved_type(ty)?;
    match (resolved, value) {
        (Type::Array { element, len }, Expr::Array(values)) => {
            let len = usize::try_from(model.const_value(&len)?)
                .map_err(|_| Diagnostic::new("banked global array length must be non-negative"))?;
            if values.len() > len {
                return Err(Diagnostic::new(
                    "banked global initializer has too many array values",
                ));
            }
            let mut bytes = Vec::new();
            for index in 0..len {
                if let Some(value) = values.get(index) {
                    bytes.extend(banked_global_bytes(model, &element, value)?);
                } else {
                    bytes.resize(bytes.len() + model.type_size(&element)? as usize, 0);
                }
            }
            Ok(bytes)
        }
        (Type::Named(name), Expr::StructInit { fields, .. })
            if model.structs.contains_key(&name) =>
        {
            let layout = &model.structs[&name];
            let mut bytes = vec![0; layout.size as usize];
            for (field_name, value) in fields {
                let field = layout.fields.get(field_name).ok_or_else(|| {
                    Diagnostic::new(format!(
                        "unknown field `{field_name}` on banked global `{name}`"
                    ))
                })?;
                let field_bytes = banked_global_bytes(model, &field.ty, value)?;
                bytes[field.offset as usize..field.offset as usize + field_bytes.len()]
                    .copy_from_slice(&field_bytes);
            }
            Ok(bytes)
        }
        (resolved, value) => {
            let width = model.type_width(&resolved)? as usize;
            let raw = model
                .const_value(value)
                .map_err(|_| Diagnostic::new("banked globals require compile-time initializers"))?;
            let mut bytes = raw.to_le_bytes()[..width].to_vec();
            bytes.resize(width, if raw < 0 { 0xFF } else { 0 });
            Ok(bytes)
        }
    }
}

fn collect_banked_declarations<'a>(
    declarations: &'a [Declaration],
    enclosing_bank: Option<u32>,
    output: &mut Vec<(Option<u32>, &'a Declaration)>,
) {
    for declaration in declarations {
        match declaration {
            Declaration::Cfg { declaration, .. } => {
                collect_banked_declarations(
                    core::slice::from_ref(declaration.as_ref()),
                    enclosing_bank,
                    output,
                );
            }
            Declaration::Bank { bank, declaration } => {
                collect_banked_declarations(
                    core::slice::from_ref(declaration.as_ref()),
                    Some(*bank),
                    output,
                );
            }
            declaration => output.push((enclosing_bank, declaration)),
        }
    }
}

fn validate_banked_pointer_contexts(
    declarations: &[(Option<u32>, &Declaration)],
    banking: Option<GameBoyBankingOptions>,
) -> Result<(), Diagnostic> {
    for (bank, declaration) in declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        for statement in &function.body {
            validate_banked_pointer_stmt(statement, bank.unwrap_or(0), &function.name, banking)?;
        }
    }
    Ok(())
}

fn validate_banked_pointer_stmt(
    stmt: &Stmt,
    function_bank: u32,
    function: &str,
    banking: Option<GameBoyBankingOptions>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Out { value, .. } | Stmt::Expr(value) => {
            validate_banked_pointer_expr(value, function_bank, function, banking)
        }
        Stmt::Assign { target, value, .. } => {
            validate_banked_pointer_place(target, function_bank, function, banking)?;
            validate_banked_pointer_expr(value, function_bank, function, banking)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            validate_banked_pointer_expr(condition, function_bank, function, banking)?;
            for statement in then_body.iter().chain(else_body) {
                validate_banked_pointer_stmt(statement, function_bank, function, banking)?;
            }
            Ok(())
        }
        Stmt::While { condition, body } => {
            validate_banked_pointer_expr(condition, function_bank, function, banking)?;
            for statement in body {
                validate_banked_pointer_stmt(statement, function_bank, function, banking)?;
            }
            Ok(())
        }
        Stmt::Loop { body } => {
            for statement in body {
                validate_banked_pointer_stmt(statement, function_bank, function, banking)?;
            }
            Ok(())
        }
        Stmt::Return(value) => value
            .as_ref()
            .map(|value| validate_banked_pointer_expr(value, function_bank, function, banking))
            .unwrap_or(Ok(())),
        Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => Ok(()),
    }
}

fn validate_banked_pointer_place(
    place: &Place,
    function_bank: u32,
    function: &str,
    banking: Option<GameBoyBankingOptions>,
) -> Result<(), Diagnostic> {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => {
            validate_banked_pointer_expr(index, function_bank, function, banking)
        }
        Place::Access(path) => {
            validate_banked_pointer_access(path, function_bank, function, banking)
        }
        Place::Ident(_) | Place::Field { .. } => Ok(()),
    }
}

fn validate_banked_pointer_access(
    path: &AccessPath,
    function_bank: u32,
    function: &str,
    banking: Option<GameBoyBankingOptions>,
) -> Result<(), Diagnostic> {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            validate_banked_pointer_expr(index, function_bank, function, banking)?;
        }
    }
    Ok(())
}

fn validate_banked_pointer_expr(
    expr: &Expr,
    function_bank: u32,
    function: &str,
    banking: Option<GameBoyBankingOptions>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::BankedPointer { pointer, bank } => {
            let Some(banking) = banking else {
                return Err(Diagnostic::new(format!(
                    "banked pointer qualifier `@{bank}` in function `{function}` requires Game Boy `[banking] enabled = true`"
                )));
            };
            let maximum = match banking.mapper {
                GameBoyBankingMapper::Mbc1 => 127,
                GameBoyBankingMapper::Mbc5 => 511,
            };
            if *bank == 0
                || *bank > maximum
                || (banking.mapper == GameBoyBankingMapper::Mbc1 && *bank & 0x1F == 0)
            {
                return Err(Diagnostic::new(format!(
                    "banked pointer qualifier `@{bank}` in function `{function}` is not selectable by this Game Boy mapper"
                )));
            }
            if function_bank != 0 && function_bank != *bank {
                return Err(Diagnostic::new(format!(
                    "banked pointer qualifier `@{bank}` in function `{function}` does not match enclosing bank {function_bank}; use it from a bank-0 helper or the matching banked function"
                )));
            }
            validate_banked_pointer_expr(pointer, function_bank, function, Some(banking))
        }
        Expr::Array(values) => {
            for value in values {
                validate_banked_pointer_expr(value, function_bank, function, banking)?;
            }
            Ok(())
        }
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => {
            validate_banked_pointer_expr(index, function_bank, function, banking)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            validate_banked_pointer_access(path, function_bank, function, banking)
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                validate_banked_pointer_expr(value, function_bank, function, banking)?;
            }
            Ok(())
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_banked_pointer_expr(arg, function_bank, function, banking)?;
            }
            Ok(())
        }
        Expr::Binary { left, right, .. } => {
            validate_banked_pointer_expr(left, function_bank, function, banking)?;
            validate_banked_pointer_expr(right, function_bank, function, banking)
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
        | Expr::AddressOfField { .. } => Ok(()),
    }
}

#[derive(Clone)]
struct Binding {
    storage: Storage,
    ty: Type,
}

#[derive(Clone)]
struct LoopLabels {
    continue_label: String,
    break_label: String,
}

struct Emitter {
    model: SemanticModel,
    out: String,
    labels: usize,
    scopes: Vec<HashMap<String, Binding>>,
    planned_locals: Vec<HashMap<String, Binding>>,
    loops: Vec<LoopLabels>,
    return_labels: Vec<String>,
    return_types: Vec<Option<Type>>,
    function_ram_bases: Vec<u32>,
    r0: Storage,
    r1: Storage,
    r2: Storage,
    indirect_offset: u8,
    banked_layout: BankedLayout,
    gameboy_banking: Option<GameBoyBankingOptions>,
}

impl Emitter {
    fn new(
        mut model: SemanticModel,
        gameboy_banking: Option<GameBoyBankingOptions>,
        banked_layout: BankedLayout,
    ) -> Self {
        let r0 = model
            .allocate(4)
            .expect("LR35902 result scratch allocation");
        let r1 = model.allocate(4).expect("LR35902 rhs scratch allocation");
        let r2 = model.allocate(4).expect("LR35902 work scratch allocation");
        Self {
            model,
            out: String::new(),
            labels: 0,
            scopes: Vec::new(),
            planned_locals: Vec::new(),
            loops: Vec::new(),
            return_labels: Vec::new(),
            return_types: Vec::new(),
            function_ram_bases: Vec::new(),
            r0,
            r1,
            r2,
            indirect_offset: 0,
            banked_layout,
            gameboy_banking,
        }
    }

    fn emit(mut self, program: &Program) -> Result<String, Diagnostic> {
        self.line("; generated by ezrac");
        self.line("; target: LR35902 RAM ABI");
        self.line("section .text");
        self.line("__ezra_start:");
        self.line("    di");
        self.line("    ld sp, FFFEh");
        self.emit_storage_symbols();
        self.emit_static_initializers(program)?;
        if self.gameboy_banking.is_some() {
            self.line("    ld a, 1");
            self.line("    ld (FFB0h), a");
            self.line("    xor a");
            self.line("    ld (FFB1h), a");
        }
        self.line("    call _main");
        self.line("__ezra_exit:");
        self.line("    halt");
        self.line("    jmp __ezra_exit");
        self.emit_gameboy_banking_runtime();

        let mut emitted_functions = reachable_function_names(program, &self.model);
        // Inline assembly can reference function symbols that are intentionally opaque to
        // source-level call analysis, so retain every declared function for link correctness.
        emitted_functions.extend(program.declarations.iter().filter_map(|declaration| {
            if let Declaration::Function(function) = unwrapped_declaration(declaration) {
                Some(function.name.clone())
            } else {
                None
            }
        }));
        for declaration in &program.declarations {
            if let Declaration::Function(function) = unwrapped_declaration(declaration)
                && emitted_functions.contains(&function.name)
                && !self.banked_layout.functions.contains_key(&function.name)
            {
                self.emit_function(function)?;
            }
        }
        self.emit_banked_payloads(program, &emitted_functions)?;
        for section in [".header", ".rodata", ".data", ".bss", ".assets", ".scratch"] {
            self.line(&format!("section {section}"));
        }
        Ok(self.out)
    }

    fn emit_gameboy_banking_runtime(&mut self) {
        let Some(banking) = self.gameboy_banking else {
            return;
        };
        self.line("; Game Boy mapper and far-call runtime; always resident in ROM bank 0.");
        self.line("__ezra_gb_select_bank:");
        self.line("    ld b, 0");
        self.line("__ezra_gb_select_bank_9:");
        self.line("    ld (FFB0h), a");
        self.line("    ld c, a");
        self.line("    ld a, b");
        self.line("    ld (FFB1h), a");
        match banking.mapper {
            GameBoyBankingMapper::Mbc1 => {
                self.line("    ld a, c");
                self.line("    and $1F");
                self.line("    jr nz, __ezra_gb_mbc1_bank_ready");
                self.line("    inc a");
                self.line("__ezra_gb_mbc1_bank_ready:");
                self.line("    ld (2000h), a");
                self.line("    ld a, c");
                self.line("    srl a");
                self.line("    srl a");
                self.line("    srl a");
                self.line("    srl a");
                self.line("    srl a");
                self.line("    and $03");
                self.line("    ld (4000h), a");
                self.line("    xor a");
                self.line("    ld (6000h), a");
            }
            GameBoyBankingMapper::Mbc5 => {
                self.line("    ld a, c");
                self.line("    ld (2000h), a");
                self.line("    ld a, b");
                self.line("    and $01");
                self.line("    ld (3000h), a");
            }
        }
        self.line("    ret");
        self.line("__ezra_gb_far_call:");
        self.line("    ld c, a");
        self.line("    ld d, b");
        self.line("    ld a, (FFB0h)");
        self.line("    push af");
        self.line("    ld a, (FFB1h)");
        self.line("    push af");
        self.line("    ld a, c");
        self.line("    ld b, d");
        self.line("    call __ezra_gb_select_bank_9");
        self.line("    ld de, __ezra_gb_far_return");
        self.line("    push de");
        self.line("    jp hl");
        self.line("__ezra_gb_far_return:");
        self.line("    pop af");
        self.line("    ld b, a");
        self.line("    pop af");
        self.line("    call __ezra_gb_select_bank_9");
        self.line("    ret");
    }

    fn emit_banked_payloads(
        &mut self,
        program: &Program,
        emitted: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        let mut banks = self
            .banked_layout
            .data
            .keys()
            .copied()
            .chain(self.banked_layout.functions.values().copied())
            .collect::<Vec<_>>();
        banks.sort_unstable();
        banks.dedup();
        for bank in banks {
            self.line(&format!("__ezra_bank_{bank}_start:"));
            if let Some(data) = self.banked_layout.data.get(&bank).cloned() {
                for (name, bytes) in data {
                    self.line(&format!("__ezra_banked_data_{name}:"));
                    for line in byte_data_lines("db", &bytes, ByteLiteralStyle::HexSuffix, 16) {
                        self.line(&line);
                    }
                }
            }
            for declaration in &program.declarations {
                if let Declaration::Function(function) = unwrapped_declaration(declaration)
                    && self.banked_layout.functions.get(&function.name) == Some(&bank)
                    && emitted.contains(&function.name)
                {
                    self.emit_function(function)?;
                }
            }
            self.line(&format!("__ezra_bank_{bank}_end:"));
        }
        for (name, bank) in self.banked_layout.functions.clone() {
            self.line(&format!(
                "__ezra_far_{name}_address equ 4000h + (_{name} - __ezra_bank_{bank}_start)"
            ));
        }
        Ok(())
    }

    fn emit_storage_symbols(&mut self) {
        let mut symbols = self
            .model
            .globals
            .iter()
            .map(|(name, storage)| (name.clone(), *storage))
            .chain(
                self.model
                    .embeds
                    .iter()
                    .map(|(name, embed)| (name.clone(), embed.storage)),
            )
            .collect::<Vec<_>>();
        symbols.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, storage) in symbols {
            let address = self
                .banked_layout
                .data_addresses
                .get(&name)
                .copied()
                .unwrap_or(storage.address);
            self.line(&format!("_{name} equ {address:04X}h"));
        }
    }

    fn emit_static_initializers(&mut self, program: &Program) -> Result<(), Diagnostic> {
        for (name, embed) in self.model.embeds.clone() {
            if self.banked_layout.data_addresses.contains_key(&name) {
                continue;
            }
            for (offset, byte) in embed.bytes.iter().copied().enumerate() {
                self.lda_imm(byte);
                self.sta(embed.storage.address + offset as u32);
            }
        }
        let strings = self
            .model
            .strings
            .iter()
            .map(|(value, storage)| (value.clone(), *storage))
            .collect::<Vec<_>>();
        for (value, storage) in strings {
            for (offset, byte) in value.bytes().chain(std::iter::once(0)).enumerate() {
                self.lda_imm(byte);
                self.sta(storage.address + offset as u32);
            }
        }
        for declaration in &program.declarations {
            if let Declaration::Global(global) = unwrapped_declaration(declaration) {
                if self.banked_layout.globals.contains(&global.name) {
                    continue;
                }
                let storage = self.model.globals[&global.name];
                self.emit_initializer(storage, &global.ty, &global.value)?;
            }
        }
        Ok(())
    }

    fn emit_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        let return_label = self.next_label(&format!("{}_return", function.name));
        let naked = function.attrs.iter().any(|attr| attr == "naked");
        let interrupt = function.attrs.iter().any(|attr| attr == "interrupt");
        if interrupt && (!function.params.is_empty() || function.return_type.is_some()) {
            return Err(Diagnostic::new(format!(
                "interrupt function `{}` cannot have parameters or a return value",
                function.name
            )));
        }
        if naked
            && function.body.iter().any(|stmt| {
                !matches!(
                    stmt,
                    Stmt::Asm {
                        inputs,
                        outputs,
                        ..
                    } if inputs.is_empty() && outputs.is_empty()
                )
            })
        {
            return Err(Diagnostic::new(format!(
                "naked function `{}` may contain only asm blocks without operands",
                function.name
            )));
        }
        self.line(&format!("{}:", function_label(&function.name)));
        self.scopes.push(HashMap::new());
        self.return_labels.push(return_label.clone());
        self.return_types.push(function.return_type.clone());
        self.function_ram_bases.push(self.model.next_ram_address());

        if interrupt && !naked {
            self.line("    push af");
            self.line("    push bc");
            self.line("    push de");
            self.line("    push hl");
        }
        let signature = self
            .model
            .functions
            .get(&function.name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{}`", function.name)))?;
        for (index, param) in function.params.iter().enumerate() {
            let storage = self.model.allocate_type(&param.ty)?;
            self.bind(param.name.clone(), storage, param.ty.clone())?;
            self.copy(
                signature.argument_slots[index],
                storage,
                self.model.type_size(&param.ty)?,
            );
        }
        let planned_locals = plan_static_locals(function, &mut self.model)?;
        self.planned_locals.push(planned_locals);
        self.emit_block(&function.body)?;
        self.line(&format!("{return_label}:"));

        if interrupt {
            if !naked {
                self.line("    pop hl");
                self.line("    pop de");
                self.line("    pop bc");
                self.line("    pop af");
            }
            self.line("    reti");
        } else if !naked {
            self.line("    rts");
        }

        self.return_types.pop();
        self.return_labels.pop();
        self.function_ram_bases.pop();
        self.planned_locals.pop();
        self.scopes.pop();
        Ok(())
    }

    fn emit_block(&mut self, body: &[Stmt]) -> Result<(), Diagnostic> {
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        self.line(&format!("    ; source: {}", stmt_summary(stmt)));
        match stmt {
            Stmt::Let { name, ty, value } => {
                let binding = self
                    .planned_locals
                    .last()
                    .and_then(|locals| locals.get(name))
                    .cloned()
                    .ok_or_else(|| {
                        Diagnostic::new(format!("missing planned storage for local `{name}`"))
                    })?;
                self.bind(name.clone(), binding.storage, binding.ty)?;
                self.emit_initializer(binding.storage, ty, value)?;
            }
            Stmt::Assign { target, op, value } => {
                let ty = self.place_type(target)?;
                let Ok(width) = self.model.type_width(&ty) else {
                    if *op != AssignOp::Set {
                        return Err(Diagnostic::new(
                            "compound assignment requires a scalar value",
                        ));
                    }
                    let size = self.model.type_size(&ty)?;
                    let temporary = self.model.allocate(size)?;
                    self.emit_initializer(temporary, &ty, value)?;
                    self.emit_store_aggregate_place(target, temporary, size)?;
                    return Ok(());
                };
                if *op == AssignOp::Set {
                    self.emit_expr(value, &ty)?;
                } else {
                    self.emit_load_place(target, width)?;
                    if matches!(op, AssignOp::Shl | AssignOp::Shr)
                        && let Ok(count) = self.model.const_value(value)
                        && let Ok(count) = u32::try_from(count)
                    {
                        self.shift_constant(
                            width,
                            *op == AssignOp::Shr,
                            type_is_signed(&ty),
                            count,
                        );
                    } else if *op == AssignOp::Mul
                        && let Ok(factor) = self.model.const_value(value)
                        && self.multiply_constant(width, factor)
                    {
                    } else {
                        let left = self.model.allocate(u32::from(width))?;
                        self.copy(self.r0, left, u32::from(width));
                        self.emit_expr(value, &ty)?;
                        self.copy(self.r0, self.r1, u32::from(width));
                        self.copy(left, self.r0, u32::from(width));
                        self.emit_binary_op(assign_binary(*op), width, type_is_signed(&ty))?;
                    }
                }
                self.emit_store_place(target, width)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let else_label = self.next_label("if_else");
                let end_label = self.next_label("if_end");
                if !self.emit_jump_if_false(condition, &else_label)? {
                    self.emit_expr(condition, &Type::Named("bool".to_owned()))?;
                    self.jump_if_zero(self.r0.address, &else_label);
                }
                self.emit_block(then_body)?;
                self.line(&format!("    jmp {end_label}"));
                self.line(&format!("{else_label}:"));
                self.emit_block(else_body)?;
                self.line(&format!("{end_label}:"));
            }
            Stmt::While { condition, body } => {
                let condition_label = self.next_label("while_condition");
                let break_label = self.next_label("while_end");
                self.loops.push(LoopLabels {
                    continue_label: condition_label.clone(),
                    break_label: break_label.clone(),
                });
                self.line(&format!("{condition_label}:"));
                if !self.emit_jump_if_false(condition, &break_label)? {
                    self.emit_expr(condition, &Type::Named("bool".to_owned()))?;
                    self.jump_if_zero(self.r0.address, &break_label);
                }
                self.emit_block(body)?;
                self.line(&format!("    jmp {condition_label}"));
                self.line(&format!("{break_label}:"));
                self.loops.pop();
            }
            Stmt::Loop { body } => {
                let continue_label = self.next_label("loop_body");
                let break_label = self.next_label("loop_end");
                self.loops.push(LoopLabels {
                    continue_label: continue_label.clone(),
                    break_label: break_label.clone(),
                });
                self.line(&format!("{continue_label}:"));
                self.emit_block(body)?;
                self.line(&format!("    jmp {continue_label}"));
                self.line(&format!("{break_label}:"));
                self.loops.pop();
            }
            Stmt::Break => {
                let label = self
                    .loops
                    .last()
                    .ok_or_else(|| Diagnostic::new("break outside loop"))?
                    .break_label
                    .clone();
                self.line(&format!("    jmp {label}"));
            }
            Stmt::Continue => {
                let label = self
                    .loops
                    .last()
                    .ok_or_else(|| Diagnostic::new("continue outside loop"))?
                    .continue_label
                    .clone();
                self.line(&format!("    jmp {label}"));
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    let ty = self
                        .return_types
                        .last()
                        .and_then(Clone::clone)
                        .ok_or_else(|| Diagnostic::new("value return in void function"))?;
                    self.emit_expr(value, &ty)?;
                }
                let label = self
                    .return_labels
                    .last()
                    .expect("function return label")
                    .clone();
                self.line(&format!("    jmp {label}"));
            }
            Stmt::Asm {
                inputs,
                outputs,
                lines,
                ..
            } => self.emit_inline_asm(inputs, outputs, lines)?,
            Stmt::Out { port, .. } => {
                return Err(Diagnostic::new(format!(
                    "MOS 6502 does not support separate port I/O `{port}`; use mmio instead"
                )));
            }
            Stmt::Expr(expr) => {
                let ty = self.expr_type(expr).unwrap_or(Type::Named("u8".to_owned()));
                self.emit_expr(expr, &ty)?;
            }
        }
        Ok(())
    }

    fn emit_initializer(
        &mut self,
        storage: Storage,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        match (self.model.resolved_type(ty)?, value) {
            (Type::Array { .. }, Expr::Ident(name)) => {
                let source = self.binding(name)?;
                self.copy(source.storage, storage, storage.size);
            }
            (Type::Array { element, len }, Expr::Array(values)) => {
                let element_size = self.model.type_size(&element)?;
                let len = u32::try_from(self.model.const_value(&len)?)
                    .map_err(|_| Diagnostic::new("invalid array length"))?;
                for index in 0..len {
                    let target = Storage {
                        address: storage.address + index * element_size,
                        size: element_size,
                    };
                    if let Some(value) = values.get(index as usize) {
                        self.emit_initializer(target, &element, value)?;
                    } else {
                        self.zero(target);
                    }
                }
            }
            (Type::Named(name), Expr::StructInit { fields, .. })
                if self.model.structs.contains_key(&name) =>
            {
                self.zero(storage);
                let layout = self.model.structs[&name].clone();
                for (field_name, value) in fields {
                    let field = layout
                        .fields
                        .get(field_name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown field `{field_name}`")))?;
                    self.emit_initializer(
                        Storage {
                            address: storage.address + field.offset,
                            size: field.size,
                        },
                        &field.ty,
                        value,
                    )?;
                }
            }
            (Type::Named(name), Expr::Ident(source)) if self.model.structs.contains_key(&name) => {
                let source = self.binding(source)?;
                self.copy(source.storage, storage, storage.size);
            }
            (resolved @ (Type::Array { .. } | Type::Named(_)), Expr::Deref(pointer)) => {
                self.emit_expr(pointer, &Type::Ptr(Box::new(resolved)))?;
                self.copy_result_to_zp();
                self.copy_indirect_to_storage(storage, storage.size);
            }
            (resolved, _) => {
                let width = self.model.type_width(&resolved)?;
                self.emit_expr(value, &resolved)?;
                self.copy(self.r0, storage, u32::from(width));
            }
        }
        Ok(())
    }

    fn emit_jump_if_false(
        &mut self,
        condition: &Expr,
        false_label: &str,
    ) -> Result<bool, Diagnostic> {
        let Expr::Binary { left, op, right } = condition else {
            return Ok(false);
        };
        if !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            return Ok(false);
        }
        let Expr::Binary {
            left: source,
            op: BinaryOp::BitAnd,
            right: mask_expr,
        } = left.as_ref()
        else {
            return Ok(false);
        };
        let (Ok(mask), Ok(expected)) = (
            self.model.const_value(mask_expr),
            self.model.const_value(right),
        ) else {
            return Ok(false);
        };
        let source_ty = self.model.resolved_type(&self.expr_type(source)?)?;
        let width = self.model.type_width(&source_ty)?;
        let (Ok(mask), Ok(expected)) = (u32::try_from(mask), u32::try_from(expected)) else {
            return Ok(false);
        };
        let bits = u32::from(width) * 8;
        let width_mask = if bits >= u32::BITS {
            u32::MAX
        } else {
            (1_u32 << bits) - 1
        };
        if mask > width_mask || !mask.is_power_of_two() || (expected != 0 && expected != mask) {
            return Ok(false);
        }

        self.emit_expr(source, &source_ty)?;
        let byte_offset = mask.trailing_zeros() / 8;
        self.lda(self.r0.address + byte_offset);
        self.line(&format!("    bit {}, a", mask.trailing_zeros() % 8));
        let branch = if (*op == BinaryOp::Eq) == (expected == 0) {
            "jp nz,"
        } else {
            "jp z,"
        };
        self.line(&format!("    {branch} {false_label}"));
        Ok(true)
    }

    fn emit_expr(&mut self, expr: &Expr, expected: &Type) -> Result<(), Diagnostic> {
        let width = self.model.type_width(expected)?;
        match expr {
            Expr::Int(value) | Expr::TypedInt(value, _) => self.load_constant(*value, width),
            Expr::Bool(value) => self.load_constant(i64::from(*value), width),
            Expr::Char(value) => self.load_constant(i64::from(*value), width),
            Expr::String(value) => {
                let storage = self.model.intern_string(value)?;
                self.load_constant(i64::from(storage.address), width);
            }
            Expr::Ident(name) => {
                if let Some(value) = self.model.constants.get(name).copied() {
                    self.load_constant(value, width);
                } else {
                    let binding = self.binding(name)?;
                    let source_width = self.model.type_width(&binding.ty)?;
                    self.copy(binding.storage, self.r0, u32::from(source_width));
                    self.extend_result(source_width, width, type_is_signed(&binding.ty));
                }
            }
            Expr::In(port) => {
                return Err(Diagnostic::new(format!(
                    "MOS 6502 does not support separate port I/O `{port}`; use mmio instead"
                )));
            }
            Expr::AddressOf(name) => {
                let binding = self.binding(name)?;
                self.load_constant(i64::from(binding.storage.address), width);
            }
            Expr::AddressOfIndex { name, index } => {
                self.emit_named_index_address(name, index)?;
                self.copy_zp_to_result(width);
            }
            Expr::AddressOfField { base, field } => {
                let binding = self.binding(base)?;
                let field = self.model.field(&binding.ty, field)?;
                self.load_constant(i64::from(binding.storage.address + field.offset), width);
            }
            Expr::AddressOfAccess(path) => {
                let (_, _) = self.emit_access_address(path)?;
                self.copy_zp_to_result(width);
            }
            Expr::Index { name, index } => {
                let element = self.emit_named_index_address(name, index)?;
                let element_width = self.model.type_width(&element)?;
                self.load_indirect(element_width);
                self.extend_result(element_width, width, false);
            }
            Expr::Field { base, field } => {
                let constant_name = format!("{base}.{field}");
                if let Some(value) = self.model.constants.get(&constant_name).copied() {
                    self.load_constant(value, width);
                } else {
                    let binding = self.binding(base)?;
                    let field = self.model.field(&binding.ty, field)?.clone();
                    let source_width = self.model.type_width(&field.ty)?;
                    self.copy(
                        Storage {
                            address: binding.storage.address + field.offset,
                            size: field.size,
                        },
                        self.r0,
                        u32::from(source_width),
                    );
                    self.extend_result(source_width, width, type_is_signed(&field.ty));
                }
            }
            Expr::Access(path) => {
                let (ty, _) = self.emit_access_address(path)?;
                let source_width = self.model.type_width(&ty)?;
                self.load_indirect(source_width);
                self.extend_result(source_width, width, false);
            }
            Expr::Deref(pointer) => {
                self.emit_expr(pointer, &Type::Ptr(Box::new(expected.clone())))?;
                self.copy_result_to_zp();
                self.load_indirect(width);
            }
            Expr::BankedPointer { pointer, .. } => self.emit_expr(pointer, expected)?,
            Expr::Call { path, args } => self.emit_call(path, args, expected)?,
            Expr::Unary { op, expr } => {
                self.emit_expr(expr, expected)?;
                self.emit_unary(*op, width);
            }
            Expr::Binary { left, op, right } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.emit_short_circuit(left, *op, right)?;
                    self.extend_result(1, width, false);
                    return Ok(());
                }
                let operand_ty = if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or)
                {
                    self.expr_type(left)
                        .or_else(|_| self.expr_type(right))
                        .unwrap_or(expected.clone())
                } else {
                    expected.clone()
                };
                let operand_width = self.model.type_width(&operand_ty)?;
                self.emit_expr(left, &operand_ty)?;
                if matches!(op, BinaryOp::Shl | BinaryOp::Shr)
                    && let Ok(count) = self.model.const_value(right)
                    && let Ok(count) = u32::try_from(count)
                {
                    self.shift_constant(
                        operand_width,
                        *op == BinaryOp::Shr,
                        type_is_signed(&operand_ty),
                        count,
                    );
                    return Ok(());
                }
                if *op == BinaryOp::Mul
                    && let Ok(factor) = self.model.const_value(right)
                    && self.multiply_constant(operand_width, factor)
                {
                    return Ok(());
                }
                let left_storage = self.model.allocate(u32::from(operand_width))?;
                self.copy(self.r0, left_storage, u32::from(operand_width));
                self.emit_expr(right, &operand_ty)?;
                self.copy(self.r0, self.r1, u32::from(operand_width));
                self.copy(left_storage, self.r0, u32::from(operand_width));
                if matches!(op, BinaryOp::Add | BinaryOp::Sub)
                    && let Type::Ptr(inner) = self.model.resolved_type(&operand_ty)?
                {
                    self.scale_storage(self.r1, operand_width, self.model.type_size(&inner)?);
                }
                self.emit_binary_op(*op, operand_width, type_is_signed(&operand_ty))?;
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.extend_result(1, width, false);
                }
            }
            Expr::Cast { ty, expr } => {
                let source_ty = self.expr_type(expr).unwrap_or_else(|_| ty.clone());
                let source_width = self.model.type_width(&source_ty)?;
                self.emit_expr(expr, &source_ty)?;
                self.extend_result(source_width, width, type_is_signed(&source_ty));
            }
            Expr::Array(_) | Expr::StructInit { .. } => {
                return Err(Diagnostic::new("aggregate value requires storage context"));
            }
        }
        Ok(())
    }

    fn live_storage_segments(&self, live: Storage, args: &[Expr]) -> Vec<Storage> {
        let mut excluded = args
            .iter()
            .filter_map(|arg| match arg {
                // The callee can mutate this storage through its pointer parameter.
                // Restoring a pre-call snapshot would discard that mutation.
                Expr::AddressOf(name) => self.binding(name).ok().map(|binding| binding.storage),
                _ => None,
            })
            .filter_map(|storage| {
                let start = storage.address.max(live.address);
                let end = storage
                    .address
                    .saturating_add(storage.size)
                    .min(live.address.saturating_add(live.size));
                (start < end).then_some((start, end))
            })
            .collect::<Vec<_>>();
        excluded.sort_unstable();

        let mut saved = Vec::new();
        let mut cursor = live.address;
        for (start, end) in excluded {
            if start > cursor {
                saved.push(Storage {
                    address: cursor,
                    size: start - cursor,
                });
            }
            cursor = cursor.max(end);
        }
        let live_end = live.address.saturating_add(live.size);
        if cursor < live_end {
            saved.push(Storage {
                address: cursor,
                size: live_end - cursor,
            });
        }
        saved
    }

    fn emit_call(
        &mut self,
        path: &[String],
        args: &[Expr],
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        let name = path.join(".");
        match name.as_str() {
            "mem.peek8" | "ezra.mem.peek8" => {
                self.emit_expr(&args[0], &Type::Ptr(Box::new(Type::Named("u8".to_owned()))))?;
                self.copy_result_to_zp();
                self.load_indirect(1);
                return Ok(());
            }
            "mem.poke8" | "ezra.mem.poke8" => {
                self.emit_expr(&args[0], &Type::Ptr(Box::new(Type::Named("u8".to_owned()))))?;
                let destination = self.model.allocate(2)?;
                self.copy(self.r0, destination, 2);
                self.emit_expr(&args[1], &Type::Named("u8".to_owned()))?;
                self.set_zp_from_storage(POINTER_ZP, destination);
                self.lda(self.r0.address);
                self.line("    ldy #$00");
                self.line(&format!("    sta (${:02X}),y", POINTER_ZP));
                return Ok(());
            }
            "mem.memcpy" | "ezra.mem.memcpy" => {
                self.emit_memcpy(args)?;
                return Ok(());
            }
            "mem.memset" | "ezra.mem.memset" => {
                self.emit_memset(args)?;
                return Ok(());
            }
            _ => {}
        }
        let resolved_name = resolve_called_function(path, &self.model)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        let signature = self.model.functions[&resolved_name].clone();
        if signature.params.len() != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments, got {}",
                signature.params.len(),
                args.len()
            )));
        }
        let mut evaluated_args = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let ty = &signature.params[index];
            self.emit_expr(arg, ty)?;
            let storage = self.model.allocate(self.model.type_size(ty)?)?;
            self.copy(self.r0, storage, storage.size);
            evaluated_args.push(storage);
        }
        for (storage, argument_slot) in evaluated_args.into_iter().zip(&signature.argument_slots) {
            self.copy(storage, *argument_slot, storage.size);
        }
        let saved = self
            .function_ram_bases
            .last()
            .map(|base| Storage {
                address: *base,
                size: self.model.next_ram_address() - *base,
            })
            .map(|live| self.live_storage_segments(live, args))
            .unwrap_or_default();
        for storage in &saved {
            for offset in 0..storage.size {
                self.lda(storage.address + offset);
                self.line("    pha");
            }
        }
        if let Some(bank) = self.banked_layout.functions.get(&resolved_name).copied() {
            self.line(&format!("    ld a, {:02X}h", bank & 0xFF));
            self.line(&format!("    ld b, {:02X}h", bank >> 8));
            self.line(&format!("    ld hl, __ezra_far_{resolved_name}_address"));
            self.line("    call __ezra_gb_far_call");
        } else {
            self.line(&format!("    jsr {}", function_label(&resolved_name)));
        }
        let return_storage = signature
            .return_type
            .as_ref()
            .map(|ty| self.model.type_size(ty))
            .transpose()?
            .map(|size| self.model.allocate(size))
            .transpose()?;
        if let Some(return_storage) = return_storage {
            self.copy(self.r0, return_storage, return_storage.size);
        }
        for storage in saved.iter().rev() {
            for offset in (0..storage.size).rev() {
                self.line("    pla");
                self.sta(storage.address + offset);
            }
        }
        if let Some(return_storage) = return_storage {
            self.copy(return_storage, self.r0, return_storage.size);
        }
        if let Some(return_type) = &signature.return_type {
            let return_width = self.model.type_width(return_type)?;
            self.extend_result(return_width, self.model.type_width(expected)?, false);
        } else {
            self.zero(self.r0);
        }
        Ok(())
    }

    fn emit_memcpy(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 3 {
            return Err(Diagnostic::new("mem.memcpy requires three arguments"));
        }
        let pointer = Type::Ptr(Box::new(Type::Named("u8".to_owned())));
        self.emit_expr(&args[0], &pointer)?;
        let destination = self.model.allocate(2)?;
        self.copy(self.r0, destination, 2);
        self.emit_expr(&args[1], &pointer)?;
        let source = self.model.allocate(2)?;
        self.copy(self.r0, source, 2);
        self.emit_expr(&args[2], &Type::Named("u16".to_owned()))?;
        let length = self.model.allocate(2)?;
        self.copy(self.r0, length, 2);
        let loop_label = self.next_label("memcpy_loop");
        let done = self.next_label("memcpy_done");
        self.set_zp_from_storage(POINTER_ZP, source);
        self.set_zp_from_storage(POINTER_ZP + 2, destination);
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 2, &done);
        self.line("    ldy #$00");
        self.line(&format!("    lda (${:02X}),y", POINTER_ZP));
        self.line(&format!("    sta (${:02X}),y", POINTER_ZP + 2));
        self.increment_zp(POINTER_ZP);
        self.increment_zp(POINTER_ZP + 2);
        self.decrement(length, 2);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{done}:"));
        self.zero(self.r0);
        Ok(())
    }

    fn emit_memset(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 3 {
            return Err(Diagnostic::new("mem.memset requires three arguments"));
        }
        let pointer = Type::Ptr(Box::new(Type::Named("u8".to_owned())));
        self.emit_expr(&args[0], &pointer)?;
        let destination = self.model.allocate(2)?;
        self.copy(self.r0, destination, 2);
        self.emit_expr(&args[1], &Type::Named("u8".to_owned()))?;
        let value = self.model.allocate(1)?;
        self.copy(self.r0, value, 1);
        self.emit_expr(&args[2], &Type::Named("u16".to_owned()))?;
        let length = self.model.allocate(2)?;
        self.copy(self.r0, length, 2);
        let loop_label = self.next_label("memset_loop");
        let done = self.next_label("memset_done");
        self.set_zp_from_storage(POINTER_ZP, destination);
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(length, 2, &done);
        self.lda(value.address);
        self.line("    ldy #$00");
        self.line(&format!("    sta (${:02X}),y", POINTER_ZP));
        self.increment_zp(POINTER_ZP);
        self.decrement(length, 2);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{done}:"));
        self.zero(self.r0);
        Ok(())
    }

    fn emit_binary_op(&mut self, op: BinaryOp, width: u8, signed: bool) -> Result<(), Diagnostic> {
        match op {
            BinaryOp::Add => self.add(width),
            BinaryOp::Sub => self.sub(width),
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
                for offset in 0..u32::from(width) {
                    self.lda(self.r0.address + offset);
                    let mnemonic = match op {
                        BinaryOp::BitAnd => "and",
                        BinaryOp::BitOr => "ora",
                        BinaryOp::BitXor => "eor",
                        _ => unreachable!(),
                    };
                    self.line(&format!("    {mnemonic} ${:04X}", self.r1.address + offset));
                    self.sta(self.r0.address + offset);
                }
            }
            BinaryOp::Mul => self.multiply(width, signed),
            BinaryOp::Div | BinaryOp::Mod => self.divide(width, op == BinaryOp::Mod, signed),
            BinaryOp::Shl | BinaryOp::Shr => self.shift(width, op == BinaryOp::Shr, signed),
            BinaryOp::And | BinaryOp::Or => self.logical(op),
            op if is_comparison(op) => self.compare(op, width, signed),
            _ => return Err(Diagnostic::new("unsupported 6502 binary operation")),
        }
        Ok(())
    }

    fn emit_unary(&mut self, op: UnaryOp, width: u8) {
        match op {
            UnaryOp::BitNot => {
                for offset in 0..u32::from(width) {
                    self.lda(self.r0.address + offset);
                    self.line("    eor #$FF");
                    self.sta(self.r0.address + offset);
                }
            }
            UnaryOp::Neg => {
                for offset in 0..u32::from(width) {
                    self.lda(self.r0.address + offset);
                    self.line("    eor #$FF");
                    self.sta(self.r0.address + offset);
                }
                self.line("    clc");
                for offset in 0..u32::from(width) {
                    self.lda(self.r0.address + offset);
                    self.line(&format!(
                        "    adc #${:02X}",
                        if offset == 0 { 1 } else { 0 }
                    ));
                    self.sta(self.r0.address + offset);
                }
            }
            UnaryOp::Not => {
                let true_label = self.next_label("not_true");
                let done = self.next_label("not_done");
                self.jump_if_zero(self.r0.address, &true_label);
                self.load_constant(0, 1);
                self.line(&format!("    jmp {done}"));
                self.line(&format!("{true_label}:"));
                self.load_constant(1, 1);
                self.line(&format!("{done}:"));
            }
        }
    }

    fn add(&mut self, width: u8) {
        self.line("    clc");
        for offset in 0..u32::from(width) {
            self.lda(self.r0.address + offset);
            self.line(&format!("    adc ${:04X}", self.r1.address + offset));
            self.sta(self.r0.address + offset);
        }
    }

    fn sub(&mut self, width: u8) {
        self.line("    sec");
        for offset in 0..u32::from(width) {
            self.lda(self.r0.address + offset);
            self.line(&format!("    sbc ${:04X}", self.r1.address + offset));
            self.sta(self.r0.address + offset);
        }
    }

    fn multiply_constant(&mut self, width: u8, factor: i64) -> bool {
        let magnitude = factor.unsigned_abs();
        match magnitude {
            0 => self.zero(self.r0),
            1 => {}
            value if value.is_power_of_two() => {
                self.shift_constant(width, false, false, value.trailing_zeros());
            }
            3 | 5 | 7 | 9 => {
                let original = self
                    .model
                    .allocate(u32::from(width))
                    .expect("constant multiply scratch");
                self.copy(self.r0, original, u32::from(width));
                self.shift_constant(width, false, false, magnitude.ilog2());
                self.copy(original, self.r1, u32::from(width));
                if magnitude == 7 {
                    self.sub(width);
                } else {
                    self.add(width);
                }
            }
            6 | 10 => {
                let odd_factor = magnitude / 2;
                let optimized = self.multiply_constant(width, odd_factor as i64);
                debug_assert!(optimized);
                self.shift_constant(width, false, false, 1);
            }
            _ => return false,
        }
        if factor < 0 {
            self.emit_unary(UnaryOp::Neg, width);
        }
        true
    }

    fn multiply(&mut self, width: u8, signed: bool) {
        let loop_label = self.next_label("mul_loop");
        let done = self.next_label("mul_done");
        let multiplicand = self
            .model
            .allocate(u32::from(width))
            .expect("multiply scratch");
        let multiplier = self
            .model
            .allocate(u32::from(width))
            .expect("multiply scratch");
        let negative = self.model.allocate(1).expect("multiply sign");
        self.zero(negative);
        if signed {
            self.normalize_signed_operand(self.r0, width, negative, false);
            self.normalize_signed_operand(self.r1, width, negative, true);
        }
        self.copy(self.r0, multiplicand, u32::from(width));
        self.copy(self.r1, multiplier, u32::from(width));
        self.zero(self.r0);
        self.line(&format!("{loop_label}:"));
        self.jump_storage_zero(multiplier, width, &done);
        self.copy(multiplicand, self.r1, u32::from(width));
        self.add(width);
        self.decrement(multiplier, width);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{done}:"));
        if signed {
            self.negate_if_flag(self.r0, width, negative);
        }
    }

    fn divide(&mut self, width: u8, remainder: bool, signed: bool) {
        let loop_label = self.next_label("div_loop");
        let done = self.next_label("div_done");
        let zero = self.next_label("div_zero");
        let quotient_negative = self.model.allocate(1).expect("division sign");
        let remainder_negative = self.model.allocate(1).expect("division sign");
        self.zero(quotient_negative);
        self.zero(remainder_negative);
        if signed {
            self.lda(self.r0.address + u32::from(width - 1));
            let dividend_positive = self.next_label("dividend_positive");
            self.branch_long("bpl", &dividend_positive);
            self.toggle(quotient_negative);
            self.toggle(remainder_negative);
            self.negate_storage(self.r0, width);
            self.line(&format!("{dividend_positive}:"));
            self.normalize_signed_operand(self.r1, width, quotient_negative, true);
        }
        self.zero(self.r2);
        self.jump_storage_zero(self.r1, width, &zero);
        self.line(&format!("{loop_label}:"));
        self.jump_if_less(self.r0, self.r1, width, &done);
        self.sub(width);
        self.increment(self.r2, width);
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{zero}:"));
        self.zero(self.r0);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{done}:"));
        if !remainder {
            self.copy(self.r2, self.r0, u32::from(width));
        }
        if signed {
            self.negate_if_flag(
                self.r0,
                width,
                if remainder {
                    remainder_negative
                } else {
                    quotient_negative
                },
            );
        }
    }

    fn shift_constant(&mut self, width: u8, right: bool, signed: bool, count: u32) {
        let bits = u32::from(width) * 8;
        let count = count.min(bits);
        let byte_count = count / 8;
        let bit_count = count % 8;

        if byte_count != 0 {
            if right {
                if signed && byte_count == u32::from(width) {
                    self.lda(self.r0.address + u32::from(width - 1));
                }
                for offset in 0..u32::from(width) - byte_count {
                    self.lda(self.r0.address + offset + byte_count);
                    self.sta(self.r0.address + offset);
                }
                if signed {
                    if byte_count != u32::from(width) {
                        self.lda(self.r0.address + u32::from(width) - byte_count - 1);
                    }
                    self.line("    asl a");
                    self.line("    lda #$00");
                    self.line("    sbc #$00");
                    self.line("    eor #$FF");
                } else {
                    self.lda_imm(0);
                }
                for offset in u32::from(width) - byte_count..u32::from(width) {
                    self.sta(self.r0.address + offset);
                }
            } else {
                for offset in (byte_count..u32::from(width)).rev() {
                    self.lda(self.r0.address + offset - byte_count);
                    self.sta(self.r0.address + offset);
                }
                self.lda_imm(0);
                for offset in 0..byte_count {
                    self.sta(self.r0.address + offset);
                }
            }
        }

        for _ in 0..bit_count {
            self.shift_once(width, right, signed);
        }
    }

    fn shift_once(&mut self, width: u8, right: bool, signed: bool) {
        if right {
            if signed {
                self.lda(self.r0.address + u32::from(width - 1));
                self.line("    asl a");
            } else {
                self.line("    clc");
            }
            for offset in (0..u32::from(width)).rev() {
                self.line(&format!("    ror ${:04X}", self.r0.address + offset));
            }
        } else {
            self.line("    clc");
            for offset in 0..u32::from(width) {
                self.line(&format!("    rol ${:04X}", self.r0.address + offset));
            }
        }
    }

    fn shift(&mut self, width: u8, right: bool, signed: bool) {
        let loop_label = self.next_label("shift_loop");
        let done = self.next_label("shift_done");
        self.line(&format!("{loop_label}:"));
        self.jump_if_zero(self.r1.address, &done);
        self.shift_once(width, right, signed);
        self.line(&format!("    dec ${:04X}", self.r1.address));
        self.line(&format!("    jmp {loop_label}"));
        self.line(&format!("{done}:"));
    }

    fn logical(&mut self, op: BinaryOp) {
        let true_label = self.next_label("logic_true");
        let false_label = self.next_label("logic_false");
        let done = self.next_label("logic_done");
        match op {
            BinaryOp::And => {
                self.jump_if_zero(self.r0.address, &false_label);
                self.jump_if_zero(self.r1.address, &false_label);
                self.line(&format!("    jmp {true_label}"));
            }
            BinaryOp::Or => {
                self.jump_if_nonzero(self.r0.address, &true_label);
                self.jump_if_nonzero(self.r1.address, &true_label);
                self.line(&format!("    jmp {false_label}"));
            }
            _ => unreachable!(),
        }
        self.line(&format!("{true_label}:"));
        self.load_constant(1, 1);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{false_label}:"));
        self.load_constant(0, 1);
        self.line(&format!("{done}:"));
    }

    fn emit_short_circuit(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let decisive = self.next_label("logical_decisive");
        let done = self.next_label("logical_done");
        let bool_type = Type::Named("bool".to_owned());
        self.emit_expr(left, &bool_type)?;
        match op {
            BinaryOp::And => self.jump_if_zero(self.r0.address, &decisive),
            BinaryOp::Or => self.jump_if_nonzero(self.r0.address, &decisive),
            _ => unreachable!(),
        }
        self.emit_expr(right, &bool_type)?;
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{decisive}:"));
        self.load_constant(i64::from(op == BinaryOp::Or), 1);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn compare(&mut self, op: BinaryOp, width: u8, signed: bool) {
        let true_label = self.next_label("compare_true");
        let false_label = self.next_label("compare_false");
        let done = self.next_label("compare_done");
        if signed && !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            let top = u32::from(width - 1);
            self.lda(self.r0.address + top);
            self.line("    eor #$80");
            self.sta(self.r0.address + top);
            self.lda(self.r1.address + top);
            self.line("    eor #$80");
            self.sta(self.r1.address + top);
        }
        match op {
            BinaryOp::Eq | BinaryOp::Ne => {
                for offset in 0..u32::from(width) {
                    self.lda(self.r0.address + offset);
                    self.line(&format!("    cmp ${:04X}", self.r1.address + offset));
                    self.branch_long(
                        "bne",
                        if op == BinaryOp::Ne {
                            &true_label
                        } else {
                            &false_label
                        },
                    );
                }
                self.line(&format!(
                    "    jmp {}",
                    if op == BinaryOp::Eq {
                        &true_label
                    } else {
                        &false_label
                    }
                ));
            }
            BinaryOp::Lt | BinaryOp::Le => {
                self.jump_if_less(self.r0, self.r1, width, &true_label);
                if op == BinaryOp::Le {
                    self.jump_if_equal(self.r0, self.r1, width, &true_label);
                }
                self.line(&format!("    jmp {false_label}"));
            }
            BinaryOp::Gt | BinaryOp::Ge => {
                self.jump_if_less(self.r0, self.r1, width, &false_label);
                if op == BinaryOp::Gt {
                    self.jump_if_equal(self.r0, self.r1, width, &false_label);
                }
                self.line(&format!("    jmp {true_label}"));
            }
            _ => unreachable!(),
        }
        self.line(&format!("{true_label}:"));
        self.load_constant(1, 1);
        self.line(&format!("    jmp {done}"));
        self.line(&format!("{false_label}:"));
        self.load_constant(0, 1);
        self.line(&format!("{done}:"));
    }

    fn emit_load_place(&mut self, place: &Place, width: u8) -> Result<(), Diagnostic> {
        match self.place_address(place)? {
            Address::Direct(address) => self.copy(
                Storage {
                    address,
                    size: u32::from(width),
                },
                self.r0,
                u32::from(width),
            ),
            Address::Indirect => self.load_indirect(width),
        }
        Ok(())
    }

    fn emit_store_place(&mut self, place: &Place, width: u8) -> Result<(), Diagnostic> {
        let saved = self.model.allocate(u32::from(width))?;
        self.copy(self.r0, saved, u32::from(width));
        match self.place_address(place)? {
            Address::Direct(address) => self.copy(
                saved,
                Storage {
                    address,
                    size: u32::from(width),
                },
                u32::from(width),
            ),
            Address::Indirect => {
                for offset in 0..u32::from(width) {
                    self.lda(saved.address + offset);
                    self.line(&format!("    ldy #${offset:02X}"));
                    self.line(&format!("    sta (${:02X}),y", POINTER_ZP));
                }
            }
        }
        Ok(())
    }

    fn emit_store_aggregate_place(
        &mut self,
        place: &Place,
        source: Storage,
        size: u32,
    ) -> Result<(), Diagnostic> {
        match self.place_address(place)? {
            Address::Direct(address) => self.copy(source, Storage { address, size }, size),
            Address::Indirect => {
                for offset in 0..size {
                    self.lda(source.address + offset);
                    self.line("    ldy #$00");
                    self.line(&format!("    sta (${:02X}),y", POINTER_ZP));
                    if offset + 1 < size {
                        self.increment_zp(POINTER_ZP);
                    }
                }
            }
        }
        Ok(())
    }

    fn copy_indirect_to_storage(&mut self, storage: Storage, size: u32) {
        for offset in 0..size {
            self.line("    ldy #$00");
            self.line(&format!("    lda (${:02X}),y", POINTER_ZP));
            self.sta(storage.address + offset);
            if offset + 1 < size {
                self.increment_zp(POINTER_ZP);
            }
        }
    }

    fn place_address(&mut self, place: &Place) -> Result<Address, Diagnostic> {
        match place {
            Place::Ident(name) => {
                if self.banked_layout.globals.contains(name) {
                    return Err(Diagnostic::new(format!(
                        "banked global `{name}` is ROM-resident and cannot be assigned"
                    )));
                }
                let binding = self.binding(name)?;
                Ok(Address::Direct(binding.storage.address))
            }
            Place::Index { name, index } => {
                if self.banked_layout.globals.contains(name) {
                    return Err(Diagnostic::new(format!(
                        "banked global `{name}` is ROM-resident and cannot be assigned"
                    )));
                }
                self.emit_named_index_address(name, index)?;
                Ok(Address::Indirect)
            }
            Place::Field { base, field } => {
                if self.banked_layout.globals.contains(base) {
                    return Err(Diagnostic::new(format!(
                        "banked global `{base}` is ROM-resident and cannot be assigned"
                    )));
                }
                let binding = self.binding(base)?;
                let layout = self.model.field(&binding.ty, field)?;
                Ok(Address::Direct(binding.storage.address + layout.offset))
            }
            Place::Access(path) => {
                if self.banked_layout.globals.contains(&path.root) {
                    return Err(Diagnostic::new(format!(
                        "banked global `{}` is ROM-resident and cannot be assigned",
                        path.root
                    )));
                }
                self.emit_access_address(path)?;
                Ok(Address::Indirect)
            }
            Place::Deref(expr) => {
                let Type::Ptr(inner) = self.model.resolved_type(&self.expr_type(expr)?)? else {
                    return Err(Diagnostic::new("dereference requires pointer"));
                };
                self.emit_expr(expr, &Type::Ptr(inner.clone()))?;
                self.copy_result_to_zp();
                Ok(Address::Indirect)
            }
        }
    }

    fn place_type(&self, place: &Place) -> Result<Type, Diagnostic> {
        match place {
            Place::Ident(name) => Ok(self.binding(name)?.ty),
            Place::Index { name, .. } => {
                element_type(&self.model.resolved_type(&self.binding(name)?.ty)?)
            }
            Place::Field { base, field } => {
                Ok(self.model.field(&self.binding(base)?.ty, field)?.ty.clone())
            }
            Place::Access(path) => self.access_type(path),
            Place::Deref(expr) => match self.model.resolved_type(&self.expr_type(expr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                _ => Err(Diagnostic::new("dereference requires pointer")),
            },
        }
    }

    fn emit_named_index_address(&mut self, name: &str, index: &Expr) -> Result<Type, Diagnostic> {
        let binding = self.binding(name)?;
        let resolved = self.model.resolved_type(&binding.ty)?;
        let element = element_type(&resolved)?;
        let element_size = self.model.type_size(&element)?;
        match resolved {
            Type::Array { .. } => self.set_pointer(binding.storage.address),
            Type::Ptr(_) => {
                self.copy(binding.storage, self.r0, 2);
                self.copy_result_to_zp();
            }
            _ => return Err(Diagnostic::new("indexing requires array or pointer")),
        }
        self.add_index_to_pointer(index, element_size)?;
        Ok(element)
    }

    fn emit_access_address(&mut self, path: &AccessPath) -> Result<(Type, bool), Diagnostic> {
        let binding = self.binding(&path.root)?;
        let mut ty = self.model.resolved_type(&binding.ty)?;
        match &ty {
            Type::Ptr(_) => {
                self.copy(binding.storage, self.r0, 2);
                self.copy_result_to_zp();
                if let Type::Ptr(inner) = ty {
                    ty = *inner;
                }
            }
            _ => self.set_pointer(binding.storage.address),
        }
        for segment in &path.segments {
            match segment {
                AccessSegment::Field(name) => {
                    let field = self.model.field(&ty, name)?.clone();
                    self.add_pointer_constant(field.offset);
                    ty = field.ty;
                }
                AccessSegment::Index(index) => {
                    let element = element_type(&self.model.resolved_type(&ty)?)?;
                    let size = self.model.type_size(&element)?;
                    self.add_index_to_pointer(index, size)?;
                    ty = element;
                }
            }
        }
        Ok((ty, true))
    }

    fn add_index_to_pointer(&mut self, index: &Expr, element_size: u32) -> Result<(), Diagnostic> {
        if let Ok(index) = self.model.const_value(index) {
            let offset = u32::try_from(index)
                .ok()
                .and_then(|index| index.checked_mul(element_size))
                .ok_or_else(|| Diagnostic::new("array index offset overflow"))?;
            self.add_pointer_constant(offset);
            return Ok(());
        }
        let saved_lo = self.model.allocate(2)?;
        self.lda(POINTER_ZP);
        self.sta(saved_lo.address);
        self.lda(POINTER_ZP + 1);
        self.sta(saved_lo.address + 1);
        self.emit_expr(index, &Type::Named("u16".to_owned()))?;
        self.lda(self.r0.address);
        self.line("    ld l, a");
        self.lda(self.r0.address + 1);
        self.line("    ld h, a");
        self.scale_index_register(element_size);
        self.lda(saved_lo.address);
        self.line("    ld c, a");
        self.lda(saved_lo.address + 1);
        self.line("    ld b, a");
        self.line("    add hl, bc");
        self.line("    ld a, l");
        self.sta(POINTER_ZP);
        self.line("    ld a, h");
        self.sta(POINTER_ZP + 1);
        Ok(())
    }

    fn scale_index_register(&mut self, scale: u32) {
        let scale = scale as u16;
        if scale == 0 {
            self.line("    ld hl, 0000h");
            return;
        }
        if scale == 1 {
            return;
        }
        self.line("    ld d, h");
        self.line("    ld e, l");
        let highest_bit = u16::BITS - 1 - scale.leading_zeros();
        for bit in (0..highest_bit).rev() {
            self.line("    add hl, hl");
            if scale & (1 << bit) != 0 {
                self.line("    add hl, de");
            }
        }
    }

    fn expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Int(value) => Ok(if (0..=0xFF).contains(value) {
                Type::Named("u8".to_owned())
            } else if (0..=0xFFFF).contains(value) {
                Type::Named("u16".to_owned())
            } else {
                Type::Named("u24".to_owned())
            }),
            Expr::TypedInt(_, ty) | Expr::Cast { ty, .. } => Ok(ty.clone()),
            Expr::Bool(_) => Ok(Type::Named("bool".to_owned())),
            Expr::Char(_) | Expr::In(_) => Ok(Type::Named("u8".to_owned())),
            Expr::String(_) => Ok(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            Expr::Ident(name) => self
                .model
                .constant_types
                .get(name)
                .cloned()
                .or_else(|| self.binding(name).ok().map(|binding| binding.ty))
                .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`"))),
            Expr::Index { name, .. } => {
                element_type(&self.model.resolved_type(&self.binding(name)?.ty)?)
            }
            Expr::Field { base, field } => {
                let constant_name = format!("{base}.{field}");
                if let Some(ty) = self.model.constant_types.get(&constant_name) {
                    Ok(ty.clone())
                } else {
                    Ok(self.model.field(&self.binding(base)?.ty, field)?.ty.clone())
                }
            }
            Expr::AddressOfIndex { name, .. } => Ok(Type::Ptr(Box::new(element_type(
                &self.model.resolved_type(&self.binding(name)?.ty)?,
            )?))),
            Expr::AddressOfField { base, field } => Ok(Type::Ptr(Box::new(
                self.model.field(&self.binding(base)?.ty, field)?.ty.clone(),
            ))),
            Expr::Access(path) => self.access_type(path),
            Expr::AddressOfAccess(path) => Ok(Type::Ptr(Box::new(self.access_type(path)?))),
            Expr::AddressOf(name) => Ok(Type::Ptr(Box::new(self.binding(name)?.ty))),
            Expr::StructInit { ty, .. } => Ok(Type::Named(ty.clone())),
            Expr::Deref(expr) => match self.model.resolved_type(&self.expr_type(expr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                _ => Err(Diagnostic::new("dereference requires pointer")),
            },
            Expr::BankedPointer { pointer, .. } => self.expr_type(pointer),
            Expr::Call { path, .. } => self
                .model
                .functions
                .get(&path.join("."))
                .or_else(|| path.last().and_then(|name| self.model.functions.get(name)))
                .and_then(|signature| signature.return_type.clone())
                .ok_or_else(|| Diagnostic::new("void function has no value")),
            Expr::Unary {
                op: UnaryOp::Not, ..
            } => Ok(Type::Named("bool".to_owned())),
            Expr::Unary { expr, .. } => self.expr_type(expr),
            Expr::Binary { left, op, .. }
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) =>
            {
                let _ = left;
                Ok(Type::Named("bool".to_owned()))
            }
            Expr::Binary { left, .. } => self.expr_type(left),
            Expr::Array(_) => Err(Diagnostic::new("array type requires context")),
        }
    }

    fn access_type(&self, path: &AccessPath) -> Result<Type, Diagnostic> {
        let mut ty = self.model.resolved_type(&self.binding(&path.root)?.ty)?;
        if let Type::Ptr(inner) = ty {
            ty = *inner;
        }
        for segment in &path.segments {
            ty = match segment {
                AccessSegment::Field(name) => self.model.field(&ty, name)?.ty.clone(),
                AccessSegment::Index(_) => element_type(&self.model.resolved_type(&ty)?)?,
            };
        }
        Ok(ty)
    }

    fn emit_inline_asm(
        &mut self,
        inputs: &[crate::ast::AsmInput],
        outputs: &[crate::ast::AsmOutput],
        lines: &[String],
    ) -> Result<(), Diagnostic> {
        let mut operands = HashMap::new();
        for input in inputs {
            let binding = self.binding(&input.name)?;
            operands.insert(
                input.name.clone(),
                format!("${:04X}", binding.storage.address),
            );
        }
        for output in outputs {
            let binding = self.binding(&output.name)?;
            operands.insert(
                output.name.clone(),
                format!("${:04X}", binding.storage.address),
            );
        }
        let local_label_prefix = self.next_label("asm");
        for line in lines {
            let mut emitted = rewrite_local_labels(line, &local_label_prefix);
            for (name, value) in &operands {
                emitted = emitted.replace(&format!("{{{name}}}"), value);
            }
            if emitted.contains(['{', '}']) {
                return Err(Diagnostic::new(format!(
                    "unknown inline asm operand placeholder in `{line}`"
                )));
            }
            self.raw_line(&format!("    {emitted}"));
        }
        Ok(())
    }

    fn bind(&mut self, name: String, storage: Storage, ty: Type) -> Result<(), Diagnostic> {
        if self
            .scopes
            .iter()
            .rev()
            .any(|scope| scope.contains_key(&name))
        {
            return Err(Diagnostic::new(format!(
                "local `{name}` shadows an existing name"
            )));
        }
        self.scopes
            .last_mut()
            .expect("function scope")
            .insert(name, Binding { storage, ty });
        Ok(())
    }

    fn binding(&self, name: &str) -> Result<Binding, Diagnostic> {
        if let Some(binding) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return Ok(binding.clone());
        }
        if let Some(storage) = self.model.globals.get(name) {
            return Ok(Binding {
                storage: *storage,
                ty: self.model.global_types[name].clone(),
            });
        }
        if let Some(embed) = self.model.embeds.get(name) {
            return Ok(Binding {
                storage: embed.storage,
                ty: Type::Array {
                    element: Box::new(Type::Named("u8".to_owned())),
                    len: Box::new(Expr::Int(embed.storage.size.into())),
                },
            });
        }
        Err(Diagnostic::new(format!("unknown variable `{name}`")))
    }

    fn copy(&mut self, source: Storage, target: Storage, size: u32) {
        for offset in 0..size {
            self.lda(source.address + offset);
            self.sta(target.address + offset);
        }
    }

    fn zero(&mut self, storage: Storage) {
        self.lda_imm(0);
        for offset in 0..storage.size {
            self.sta(storage.address + offset);
        }
    }

    fn load_constant(&mut self, value: i64, width: u8) {
        for offset in 0..u32::from(width) {
            self.lda_imm(((value as u64 >> (offset * 8)) & 0xFF) as u8);
            self.sta(self.r0.address + offset);
        }
    }

    fn extend_result(&mut self, source_width: u8, target_width: u8, signed: bool) {
        if target_width <= source_width {
            return;
        }
        if signed {
            let positive = self.next_label("extend_positive");
            let done = self.next_label("extend_done");
            self.lda(self.r0.address + u32::from(source_width - 1));
            self.branch_long("bpl", &positive);
            self.lda_imm(0xFF);
            self.line(&format!("    jmp {done}"));
            self.line(&format!("{positive}:"));
            self.lda_imm(0);
            self.line(&format!("{done}:"));
        } else {
            self.lda_imm(0);
        }
        for offset in u32::from(source_width)..u32::from(target_width) {
            self.sta(self.r0.address + offset);
        }
    }

    fn scale_storage(&mut self, storage: Storage, width: u8, scale: u32) {
        if scale <= 1 {
            return;
        }
        let source = self
            .model
            .allocate(u32::from(width))
            .expect("pointer scale source");
        let result = self
            .model
            .allocate(u32::from(width))
            .expect("pointer scale result");
        self.copy(storage, source, u32::from(width));
        self.zero(result);
        for _ in 0..scale {
            self.copy(result, self.r0, u32::from(width));
            self.copy(source, self.r1, u32::from(width));
            self.add(width);
            self.copy(self.r0, result, u32::from(width));
        }
        self.copy(result, storage, u32::from(width));
    }

    fn normalize_signed_operand(
        &mut self,
        storage: Storage,
        width: u8,
        negative: Storage,
        toggle_sign: bool,
    ) {
        let positive = self.next_label("signed_positive");
        self.lda(storage.address + u32::from(width - 1));
        self.branch_long("bpl", &positive);
        if toggle_sign {
            self.toggle(negative);
        } else {
            self.lda_imm(1);
            self.sta(negative.address);
        }
        self.negate_storage(storage, width);
        self.line(&format!("{positive}:"));
    }

    fn negate_if_flag(&mut self, storage: Storage, width: u8, flag: Storage) {
        let done = self.next_label("sign_done");
        self.jump_if_zero(flag.address, &done);
        self.negate_storage(storage, width);
        self.line(&format!("{done}:"));
    }

    fn negate_storage(&mut self, storage: Storage, width: u8) {
        for offset in 0..u32::from(width) {
            self.lda(storage.address + offset);
            self.line("    eor #$FF");
            self.sta(storage.address + offset);
        }
        self.line("    clc");
        for offset in 0..u32::from(width) {
            self.lda(storage.address + offset);
            self.line(&format!(
                "    adc #${:02X}",
                if offset == 0 { 1 } else { 0 }
            ));
            self.sta(storage.address + offset);
        }
    }

    fn toggle(&mut self, storage: Storage) {
        self.lda(storage.address);
        self.line("    eor #$01");
        self.sta(storage.address);
    }

    fn set_zp_from_storage(&mut self, zero_page: u32, storage: Storage) {
        self.lda(storage.address);
        self.sta(zero_page);
        self.lda(storage.address + 1);
        self.sta(zero_page + 1);
    }

    fn increment_zp(&mut self, zero_page: u32) {
        let done = self.next_label("pointer_incremented");
        self.line(&format!("    inc ${zero_page:02X}"));
        self.branch_long("bne", &done);
        self.line(&format!("    inc ${:02X}", zero_page + 1));
        self.line(&format!("{done}:"));
    }

    fn set_pointer(&mut self, address: u32) {
        self.lda_imm(address as u8);
        self.sta(POINTER_ZP);
        self.lda_imm((address >> 8) as u8);
        self.sta(POINTER_ZP + 1);
    }

    fn add_pointer_constant(&mut self, value: u32) {
        self.line("    clc");
        self.lda(POINTER_ZP);
        self.line(&format!("    adc #${:02X}", value as u8));
        self.sta(POINTER_ZP);
        self.lda(POINTER_ZP + 1);
        self.line(&format!("    adc #${:02X}", (value >> 8) as u8));
        self.sta(POINTER_ZP + 1);
    }

    fn copy_result_to_zp(&mut self) {
        self.lda(self.r0.address);
        self.sta(POINTER_ZP);
        self.lda(self.r0.address + 1);
        self.sta(POINTER_ZP + 1);
    }

    fn copy_zp_to_result(&mut self, width: u8) {
        self.lda(POINTER_ZP);
        self.sta(self.r0.address);
        self.lda(POINTER_ZP + 1);
        self.sta(self.r0.address + 1);
        for offset in 2..u32::from(width) {
            self.lda_imm(0);
            self.sta(self.r0.address + offset);
        }
    }

    fn load_indirect(&mut self, width: u8) {
        for offset in 0..u32::from(width) {
            self.line(&format!("    ldy #${offset:02X}"));
            self.line(&format!("    lda (${:02X}),y", POINTER_ZP));
            self.sta(self.r0.address + offset);
        }
    }

    fn increment(&mut self, storage: Storage, width: u8) {
        let done = self.next_label("increment_done");
        for offset in 0..u32::from(width) {
            self.line(&format!("    inc ${:04X}", storage.address + offset));
            self.branch_long("bne", &done);
        }
        self.line(&format!("{done}:"));
    }

    fn decrement(&mut self, storage: Storage, width: u8) {
        self.line("    sec");
        for offset in 0..u32::from(width) {
            self.lda(storage.address + offset);
            self.line(&format!(
                "    sbc #${:02X}",
                if offset == 0 { 1 } else { 0 }
            ));
            self.sta(storage.address + offset);
        }
    }

    fn jump_storage_zero(&mut self, storage: Storage, width: u8, label: &str) {
        let nonzero = self.next_label("nonzero");
        for offset in 0..u32::from(width) {
            self.jump_if_nonzero(storage.address + offset, &nonzero);
        }
        self.line(&format!("    jmp {label}"));
        self.line(&format!("{nonzero}:"));
    }

    fn jump_if_equal(&mut self, left: Storage, right: Storage, width: u8, label: &str) {
        let different = self.next_label("different");
        for offset in 0..u32::from(width) {
            self.lda(left.address + offset);
            self.line(&format!("    cmp ${:04X}", right.address + offset));
            self.branch_long("bne", &different);
        }
        self.line(&format!("    jmp {label}"));
        self.line(&format!("{different}:"));
    }

    fn jump_if_less(&mut self, left: Storage, right: Storage, width: u8, label: &str) {
        let done = self.next_label("compare_ordered");
        for offset in (0..u32::from(width)).rev() {
            self.lda(left.address + offset);
            self.line(&format!("    cmp ${:04X}", right.address + offset));
            self.branch_long("bcc", label);
            self.branch_long("bne", &done);
        }
        self.line(&format!("{done}:"));
    }

    fn jump_if_zero(&mut self, address: u32, label: &str) {
        self.lda(address);
        self.branch_long("beq", label);
    }

    fn jump_if_nonzero(&mut self, address: u32, label: &str) {
        self.lda(address);
        self.branch_long("bne", label);
    }

    fn branch_long(&mut self, branch: &str, target: &str) {
        let skip = self.next_label("branch_skip");
        let inverse = match branch {
            "beq" => "bne",
            "bne" => "beq",
            "bcc" => "bcs",
            "bcs" => "bcc",
            "bpl" => "bmi",
            "bmi" => "bpl",
            _ => unreachable!("unsupported branch"),
        };
        self.line(&format!("    {inverse} {skip}"));
        self.line(&format!("    jmp {target}"));
        self.line(&format!("{skip}:"));
    }

    fn lda(&mut self, address: u32) {
        self.line(&format!("    lda ${address:04X}"));
    }

    fn sta(&mut self, address: u32) {
        self.line(&format!("    sta ${address:04X}"));
    }

    fn lda_imm(&mut self, value: u8) {
        self.line(&format!("    lda #${value:02X}"));
    }

    fn next_label(&mut self, name: &str) -> String {
        let label = format!(".L_{}_{}", sanitize(name), self.labels);
        self.labels += 1;
        label
    }

    fn raw_line(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
    }

    fn line(&mut self, line: &str) {
        let text = line.trim();
        if let Some(value) = text.strip_prefix("ldy #$") {
            self.indirect_offset =
                u8::from_str_radix(value, 16).expect("generated indirect offset");
            return;
        }
        let translated = translate_lr35902_line(line, self.indirect_offset);
        self.indirect_offset = 0;
        self.out.push_str(&translated);
        self.out.push('\n');
    }
}

fn translate_lr35902_line(line: &str, indirect_offset: u8) -> String {
    let indent = if line.starts_with("    ") { "    " } else { "" };
    let text = line.trim();
    if text.is_empty()
        || text.starts_with(';')
        || text.starts_with("section ")
        || text.ends_with(':')
    {
        return line.to_owned();
    }
    let (op, arg) = text.split_once(' ').unwrap_or((text, ""));
    let arg = arg.trim();
    let hex = |s: &str| s.trim_start_matches('#').trim_start_matches('$').to_owned() + "h";
    let direct = |s: &str| hex(s);
    let two = |lr: &str, rhs: &str| {
        format!(
            "    push af\n    ld a, ({})\n    ld b, a\n    pop af\n    {lr} b",
            direct(rhs)
        )
    };
    let indirect = |store: bool, pointer: &str| {
        let address = pointer
            .trim_start_matches('(')
            .split(')')
            .next()
            .unwrap_or(pointer);
        let mut result = if store {
            "    push af\n".to_owned()
        } else {
            String::new()
        };
        result.push_str(&format!(
            "    ld a, ({})\n    ld l, a\n    ld a, ({})\n    ld h, a",
            hex(address),
            hex(&format!(
                "{:02X}",
                u16::from_str_radix(address.trim_start_matches('$'), 16).unwrap_or(0) + 1
            ))
        ));
        for _ in 0..indirect_offset {
            result.push_str("\n    inc hl");
        }
        if store {
            result.push_str("\n    pop af\n    ld (hl), a");
        } else {
            result.push_str("\n    ld a, (hl)");
        }
        result
    };
    match op {
        "lda" if arg.starts_with('#') => format!("{indent}ld a, {}", hex(arg)),
        "lda" if arg.starts_with('(') => indirect(false, arg),
        "lda" => format!("{indent}ld a, ({})", direct(arg)),
        "sta" if arg.starts_with('(') => indirect(true, arg),
        "sta" => format!("{indent}ld ({}), a", direct(arg)),
        "jsr" => format!("{indent}call {arg}"),
        "rts" => format!("{indent}ret"),
        "pha" => format!("{indent}push af"),
        "pla" => format!("{indent}pop af"),
        "ora" => two("or a,", arg),
        "and" => two("and", arg),
        "eor" if arg.starts_with('#') => format!("    ld b, {}\n    xor b", hex(arg)),
        "eor" => two("xor", arg),
        "adc" if arg.starts_with('#') => format!("    ld b, {}\n    adc a, b", hex(arg)),
        "adc" => two("adc a,", arg),
        "sbc" if arg.starts_with('#') => format!("    ld b, {}\n    sbc a, b", hex(arg)),
        "sbc" => two("sbc a,", arg),
        "cmp" => two("cp", arg),
        "asl" if arg == "a" => "    sla a".to_owned(),
        "inc" | "dec" | "rol" | "ror" | "asl" => format!(
            "    ld a, ({})\n    {} a\n    ld ({}), a",
            direct(arg),
            match op {
                "asl" => "sla",
                "rol" => "rl",
                "ror" => "rr",
                _ => op,
            },
            direct(arg)
        ),
        "beq" => format!("{indent}jp z, {arg}"),
        "bne" => format!("{indent}jp nz, {arg}"),
        "bcc" => format!("{indent}jp nc, {arg}"),
        "bcs" => format!("{indent}jp c, {arg}"),
        "bpl" => format!("{indent}bit 7, a\n{indent}jp z, {arg}"),
        "bmi" => format!("{indent}bit 7, a\n{indent}jp nz, {arg}"),
        "clc" => format!("{indent}and a"),
        "sec" => format!("{indent}scf"),
        "jmp" => format!("{indent}jp {arg}"),
        "cli" => format!("{indent}di"),
        "rti" => format!("{indent}reti"),
        "sei" | "call" | "ret" | "reti" | "nop" | "di" => line.to_owned(),
        _ => line.to_owned(),
    }
}

enum Address {
    Direct(u32),
    Indirect,
}

fn element_type(ty: &Type) -> Result<Type, Diagnostic> {
    match ty {
        Type::Array { element, .. } | Type::Ptr(element) => Ok((**element).clone()),
        _ => Err(Diagnostic::new("indexing requires array or pointer")),
    }
}

fn type_is_signed(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if name.starts_with('i'))
}

fn is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    )
}

const LR35902_MEMORY_LOCAL_CLASS: RegClass = RegClass(0);
const LR35902_STATIC_SPILL_CLASS: SpillClassId = SpillClassId(0);

fn lr35902_local_target() -> Target {
    Target {
        units: ["A", "B", "C", "D", "E", "H", "L"]
            .into_iter()
            .map(RegisterUnit::new)
            .collect(),
        registers: vec![
            PhysicalRegister::new("A", vec![RegUnit(0)]),
            PhysicalRegister::new("B", vec![RegUnit(1)]),
            PhysicalRegister::new("C", vec![RegUnit(2)]),
            PhysicalRegister::new("D", vec![RegUnit(3)]),
            PhysicalRegister::new("E", vec![RegUnit(4)]),
            PhysicalRegister::new("H", vec![RegUnit(5)]),
            PhysicalRegister::new("L", vec![RegUnit(6)]),
            PhysicalRegister::new("BC", vec![RegUnit(1), RegUnit(2)]),
            PhysicalRegister::new("DE", vec![RegUnit(3), RegUnit(4)]),
            PhysicalRegister::new("HL", vec![RegUnit(5), RegUnit(6)]),
        ],
        register_classes: vec![
            RegisterClass::new("memory-only", vec![]),
            RegisterClass::new("byte", (0..7).map(PhysReg).collect()),
            RegisterClass::new("pair", vec![PhysReg(7), PhysReg(8), PhysReg(9)]),
        ],
        spill_classes: vec![
            SpillClass::new("static-bytes", None, 1)
                .with_base_alignment(1)
                .for_register_classes(vec![LR35902_MEMORY_LOCAL_CLASS]),
        ],
    }
}

fn plan_static_locals(
    function: &Function,
    model: &mut SemanticModel,
) -> Result<HashMap<String, Binding>, Diagnostic> {
    let mut locals = Vec::new();
    let mut local_types = HashMap::new();
    collect_static_locals(&function.body, model, &mut locals, &mut local_types)?;
    let planned = allocate_source_locals(&lr35902_local_target(), &locals, &function.body, &[])
        .map_err(|diagnostics| {
            Diagnostic::new(format!(
                "LR35902 local allocation failed: {}",
                diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        })?;
    let backing_size = planned
        .allocation
        .spill_slots
        .iter()
        .map(|slot| slot.offset.saturating_add(slot.size))
        .max()
        .unwrap_or(0);
    let backing = (backing_size != 0)
        .then(|| model.allocate(backing_size))
        .transpose()?;
    let mut bindings = HashMap::new();
    for (name, ty) in local_types {
        let vreg = planned.locals.vreg(&name).ok_or_else(|| {
            Diagnostic::new(format!("missing LR35902 local allocation for `{name}`"))
        })?;
        let slot_index = match planned.allocation.location(vreg) {
            Some(Location::Spill(slot_index)) => slot_index,
            Some(Location::Register(_)) => {
                return Err(Diagnostic::new(format!(
                    "LR35902 local `{name}` was assigned a register"
                )));
            }
            Some(Location::Unused) | None => {
                return Err(Diagnostic::new(format!(
                    "LR35902 local `{name}` has no storage allocation"
                )));
            }
        };
        let slot = planned
            .allocation
            .spill_slots
            .get(slot_index)
            .ok_or_else(|| Diagnostic::new(format!("invalid spill slot for local `{name}`")))?;
        if slot.class != LR35902_STATIC_SPILL_CLASS {
            return Err(Diagnostic::new(format!(
                "invalid spill class for LR35902 local `{name}`"
            )));
        }
        let backing = backing.ok_or_else(|| {
            Diagnostic::new(format!("missing static backing storage for local `{name}`"))
        })?;
        bindings.insert(
            name,
            Binding {
                storage: Storage {
                    address: backing.address + slot.offset,
                    size: model.type_size(&ty)?,
                },
                ty,
            },
        );
    }
    Ok(bindings)
}

fn collect_static_locals(
    body: &[Stmt],
    model: &SemanticModel,
    locals: &mut Vec<SourceLocal>,
    local_types: &mut HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    for stmt in body {
        match stmt {
            Stmt::Let { name, ty, .. } => {
                if local_types.insert(name.clone(), ty.clone()).is_some() {
                    return Err(Diagnostic::new(format!("duplicate local `{name}`")));
                }
                locals.push(
                    SourceLocal::new(
                        name.clone(),
                        model.type_size(ty)?,
                        1,
                        LR35902_MEMORY_LOCAL_CLASS,
                    )
                    .with_spill_classes(vec![LR35902_STATIC_SPILL_CLASS])
                    .with_force_memory(true),
                );
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_static_locals(then_body, model, locals, local_types)?;
                collect_static_locals(else_body, model, locals, local_types)?;
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                collect_static_locals(body, model, locals, local_types)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn assign_binary(op: AssignOp) -> BinaryOp {
    match op {
        AssignOp::Add => BinaryOp::Add,
        AssignOp::Sub => BinaryOp::Sub,
        AssignOp::Mul => BinaryOp::Mul,
        AssignOp::Div => BinaryOp::Div,
        AssignOp::Mod => BinaryOp::Mod,
        AssignOp::BitAnd => BinaryOp::BitAnd,
        AssignOp::BitOr => BinaryOp::BitOr,
        AssignOp::BitXor => BinaryOp::BitXor,
        AssignOp::Shl => BinaryOp::Shl,
        AssignOp::Shr => BinaryOp::Shr,
        AssignOp::Set => unreachable!(),
    }
}

fn reachable_function_names(program: &Program, model: &SemanticModel) -> HashSet<String> {
    let mut graph = HashMap::new();
    let mut roots = vec!["main".to_owned()];

    for declaration in &program.declarations {
        match declaration {
            Declaration::Function(function) => {
                let mut calls = Vec::new();
                collect_stmt_calls(&function.body, &mut calls);
                graph.insert(
                    function.name.clone(),
                    calls
                        .into_iter()
                        .filter_map(|path| resolve_called_function(&path, model))
                        .collect::<Vec<_>>(),
                );
                if function
                    .attrs
                    .iter()
                    .any(|attr| attr == "naked" || attr == "interrupt")
                {
                    roots.push(function.name.clone());
                }
            }
            Declaration::Global(global) => {
                let mut calls = Vec::new();
                collect_expr_calls(&global.value, &mut calls);
                roots.extend(
                    calls
                        .into_iter()
                        .filter_map(|path| resolve_called_function(&path, model)),
                );
            }
            _ => {}
        }
    }

    let mut reachable = HashSet::new();
    let mut pending = roots;
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        if let Some(calls) = graph.get(&name) {
            pending.extend(calls.iter().cloned());
        }
    }
    reachable
}

fn resolve_called_function(path: &[String], model: &SemanticModel) -> Option<String> {
    let qualified = path.join(".");
    if model.functions.contains_key(&qualified) {
        Some(qualified)
    } else {
        path.last()
            .filter(|name| model.functions.contains_key(*name))
            .cloned()
    }
}

fn collect_stmt_calls(stmts: &[Stmt], calls: &mut Vec<Vec<String>>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::Return(Some(value)) | Stmt::Expr(value) => {
                collect_expr_calls(value, calls);
            }
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
                collect_stmt_calls(then_body, calls);
                collect_stmt_calls(else_body, calls);
            }
            Stmt::While { condition, body } => {
                collect_expr_calls(condition, calls);
                collect_stmt_calls(body, calls);
            }
            Stmt::Loop { body } => collect_stmt_calls(body, calls),
            Stmt::Out { value, .. } => collect_expr_calls(value, calls),
            Stmt::Return(None) | Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => {}
        }
    }
}

fn collect_place_calls(place: &Place, calls: &mut Vec<Vec<String>>) {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => collect_expr_calls(index, calls),
        Place::Access(path) => collect_access_path_calls(path, calls),
        Place::Ident(_) | Place::Field { .. } => {}
    }
}

fn collect_expr_calls(expr: &Expr, calls: &mut Vec<Vec<String>>) {
    match expr {
        Expr::Array(values) => {
            for value in values {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
            collect_expr_calls(index, calls);
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => collect_access_path_calls(path, calls),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Cast { expr: value, .. } => {
            collect_expr_calls(value, calls);
        }
        Expr::Call { path, args } => {
            calls.push(path.clone());
            for arg in args {
                collect_expr_calls(arg, calls);
            }
        }
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

fn collect_access_path_calls(path: &AccessPath, calls: &mut Vec<Vec<String>>) {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            collect_expr_calls(index, calls);
        }
    }
}

fn rewrite_local_labels(line: &str, prefix: &str) -> String {
    line.split_whitespace()
        .map(|token| {
            token
                .strip_prefix('.')
                .map(|local| format!(".{prefix}_{local}"))
                .unwrap_or_else(|| token.to_owned())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn function_label(name: &str) -> String {
    format!("_{name}")
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(all(test, feature = "lr35902"))]
mod tests;
