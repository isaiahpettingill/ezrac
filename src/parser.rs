use crate::compat::{SourcePath, prelude::*, source_path_owned};
#[cfg(all(feature = "std", test))]
use std::path::Path;

use pest::{Parser, iterators::Pair};
use pest_derive::Parser;

use crate::{
    ast::{
        AccessPath, AccessSegment, AliasDecl, AsmInput, AsmOutput, AssignOp, BinaryOp,
        CfgPredicate, ConstDecl, Declaration, EmbedDecl, EmbedSource, Expr, ExternFunction,
        Function, GlobalDecl, MmioDecl, Param, Place, PortDecl, Program, SourceUnit, Stmt,
        StructDecl, Type, UnaryOp,
    },
    diagnostic::{Diagnostic, SourcePosition, SourceSpan},
};

#[derive(Parser)]
#[grammar = "ezra.pest"]
struct EzraParser;

pub fn parse_program(file: &SourcePath, source: &str) -> Result<Program, Diagnostic> {
    let original_source = source;
    let normalized;
    let source = if needs_implicit_deref_assignment_separators(source) {
        normalized = insert_implicit_deref_assignment_separators(source);
        normalized.as_str()
    } else {
        source
    };
    let mut pairs =
        EzraParser::parse(Rule::program, source).map_err(|error| pest_error(file, error))?;
    let program = pairs
        .next()
        .ok_or_else(|| Diagnostic::new("parser produced no program"))?;
    let declarations = program
        .into_inner()
        .filter(|pair| pair.as_rule() != Rule::EOI)
        .map(|pair| {
            let span = pair_span(file, &pair);
            build_decl(file, pair).map_err(|error| error.with_span_if_missing(span))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Program {
        source_path: source_path_owned(file),
        source_text: Some(original_source.to_owned()),
        source_units: vec![SourceUnit {
            path: source_path_owned(file),
            text: original_source.to_owned(),
        }],
        declarations,
    })
}

fn needs_implicit_deref_assignment_separators(source: &str) -> bool {
    source.lines().skip(1).any(line_starts_deref_assignment)
}

fn insert_implicit_deref_assignment_separators(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for (index, line) in source.lines().enumerate() {
        if index > 0 && line_starts_deref_assignment(line) && previous_line_can_end_stmt(&out) {
            out.push(';');
        }
        if index > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    if source.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn previous_line_can_end_stmt(source_so_far: &str) -> bool {
    let Some(ch) = source_so_far.chars().rev().find(|ch| !ch.is_whitespace()) else {
        return false;
    };
    !matches!(
        ch,
        ';' | '{'
            | '('
            | '['
            | ','
            | '='
            | '+'
            | '-'
            | '/'
            | '%'
            | '&'
            | '|'
            | '^'
            | '<'
            | '>'
            | '!'
            | '~'
    )
}

fn line_starts_deref_assignment(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('*') && line_contains_assignment_op(line)
}

fn line_contains_assignment_op(line: &str) -> bool {
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if in_string || in_char {
            match ch {
                '\\' => escaped = true,
                '"' if in_string => in_string = false,
                '\'' if in_char => in_char = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '\'' => in_char = true,
            '/' if chars.peek() == Some(&'/') => return false,
            '=' => return true,
            '+' | '-' | '*' | '/' | '%' | '&' | '|' | '^' => {
                if chars.peek() == Some(&'=') {
                    return true;
                }
            }
            '<' | '>' => {
                let mut lookahead = chars.clone();
                if lookahead.next() == Some(ch) && lookahead.next() == Some('=') {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn build_decl(file: &SourcePath, pair: Pair<'_, Rule>) -> Result<Declaration, Diagnostic> {
    match pair.as_rule() {
        Rule::decl => {
            let mut predicates = Vec::new();
            let mut declaration = None;
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    Rule::cfg_attr => predicates.push(build_cfg_attr(inner)?),
                    _ => declaration = Some(build_decl(file, inner)?),
                }
            }
            let declaration = declaration
                .ok_or_else(|| Diagnostic::new("conditional declaration missing declaration"))?;
            if predicates.is_empty() {
                Ok(declaration)
            } else {
                Ok(Declaration::Cfg {
                    predicates,
                    declaration: Box::new(declaration),
                })
            }
        }
        Rule::import_decl => Ok(Declaration::Import(
            pair.into_inner().next().unwrap().as_str().to_owned(),
        )),
        Rule::const_decl => build_const(pair).map(Declaration::Const),
        Rule::alias_decl => build_alias(pair).map(Declaration::Alias),
        Rule::port_decl => build_port(pair).map(Declaration::Port),
        Rule::mmio_decl => build_mmio(pair).map(Declaration::Mmio),
        Rule::embed_decl => build_embed(pair).map(Declaration::Embed),
        Rule::global_decl => build_global(pair).map(Declaration::Global),
        Rule::struct_decl => build_struct(pair).map(Declaration::Struct),
        Rule::extern_decl => build_extern(pair).map(Declaration::ExternAsmFunction),
        Rule::fn_decl => build_fn(file, pair).map(Declaration::Function),
        _ => unreachable!("unexpected decl rule {:?}", pair.as_rule()),
    }
}

fn build_cfg_attr(pair: Pair<'_, Rule>) -> Result<CfgPredicate, Diagnostic> {
    build_cfg_predicate(pair.into_inner().next().unwrap())
}

fn build_cfg_predicate(pair: Pair<'_, Rule>) -> Result<CfgPredicate, Diagnostic> {
    match pair.as_rule() {
        Rule::cfg_predicate => build_cfg_predicate(pair.into_inner().next().unwrap()),
        Rule::cfg_all => Ok(CfgPredicate::All(build_cfg_predicate_list(pair)?)),
        Rule::cfg_any => Ok(CfgPredicate::Any(build_cfg_predicate_list(pair)?)),
        Rule::cfg_not => Ok(CfgPredicate::Not(Box::new(build_cfg_predicate(
            pair.into_inner().next().unwrap(),
        )?))),
        Rule::cfg_call => build_cfg_call(pair),
        Rule::cfg_flag => match pair.as_str() {
            "debug" => Ok(CfgPredicate::Debug),
            "release" => Ok(CfgPredicate::Release),
            other => Err(Diagnostic::new(format!("unknown cfg predicate `{other}`"))),
        },
        _ => unreachable!("unexpected cfg predicate rule {:?}", pair.as_rule()),
    }
}

fn build_cfg_predicate_list(pair: Pair<'_, Rule>) -> Result<Vec<CfgPredicate>, Diagnostic> {
    pair.into_inner()
        .flat_map(|inner| inner.into_inner())
        .map(build_cfg_predicate)
        .collect()
}

fn build_cfg_call(pair: Pair<'_, Rule>) -> Result<CfgPredicate, Diagnostic> {
    let mut parts = pair.into_inner();
    let name = parts.next().unwrap().as_str();
    let value = parts
        .next()
        .ok_or_else(|| Diagnostic::new(format!("cfg predicate `{name}` is missing an argument")))?;
    match name {
        "target" => Ok(CfgPredicate::Target(parse_cfg_string(name, value)?)),
        "target_family" => Ok(CfgPredicate::TargetFamily(parse_cfg_string(name, value)?)),
        "cpu" => Ok(CfgPredicate::Cpu(parse_cfg_string(name, value)?)),
        "vendor" => Ok(CfgPredicate::Vendor(parse_cfg_string(name, value)?)),
        "os" => Ok(CfgPredicate::Os(parse_cfg_string(name, value)?)),
        "pointer_width" => Ok(CfgPredicate::PointerWidth(parse_cfg_int(name, value)?)),
        "address_width" => Ok(CfgPredicate::AddressWidth(parse_cfg_int(name, value)?)),
        "feature" => Ok(CfgPredicate::Feature(parse_cfg_string(name, value)?)),
        other => Err(Diagnostic::new(format!("unknown cfg predicate `{other}`"))),
    }
}

fn parse_cfg_string(name: &str, pair: Pair<'_, Rule>) -> Result<String, Diagnostic> {
    if pair.as_rule() != Rule::string_lit {
        return Err(Diagnostic::new(format!(
            "cfg predicate `{name}` expects a string argument"
        )));
    }
    parse_string(pair.as_str())
}

fn parse_cfg_int(name: &str, pair: Pair<'_, Rule>) -> Result<u16, Diagnostic> {
    if pair.as_rule() != Rule::int_lit {
        return Err(Diagnostic::new(format!(
            "cfg predicate `{name}` expects an integer argument"
        )));
    }
    let value = parse_int(pair.as_str())?;
    u16::try_from(value).map_err(|_| {
        Diagnostic::new(format!(
            "cfg predicate `{name}` integer argument is outside u16 range"
        ))
    })
}

fn build_embed(pair: Pair<'_, Rule>) -> Result<EmbedDecl, Diagnostic> {
    let mut public = false;
    let mut name = None;
    let mut source = None;
    let mut section = None;
    let mut align = None;
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::visibility => public = true,
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::embed_source => source = Some(build_embed_source(inner)?),
            Rule::embed_opts => {
                for opt in inner.into_inner() {
                    match opt.as_rule() {
                        Rule::section_name => section = Some(opt.as_str().to_owned()),
                        Rule::expr => align = Some(build_expr(opt)?),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(EmbedDecl {
        public,
        name: name.unwrap(),
        source: source.unwrap(),
        section,
        align,
    })
}

fn build_embed_source(pair: Pair<'_, Rule>) -> Result<EmbedSource, Diagnostic> {
    let inner = pair.into_inner().next().unwrap();
    match inner.as_rule() {
        Rule::file_embed => Ok(EmbedSource::File(parse_string(
            inner.into_inner().next().unwrap().as_str(),
        )?)),
        Rule::bytes_embed => {
            let bytes = inner
                .into_inner()
                .next()
                .map(|args| args.into_inner().map(build_expr).collect())
                .unwrap_or_else(|| Ok(Vec::new()))?;
            Ok(EmbedSource::Bytes(bytes))
        }
        Rule::text_embed => Ok(EmbedSource::Text(parse_string(
            inner.into_inner().next().unwrap().as_str(),
        )?)),
        Rule::cstr_embed => Ok(EmbedSource::CStr(parse_string(
            inner.into_inner().next().unwrap().as_str(),
        )?)),
        Rule::repeat_embed => {
            let mut parts = inner.into_inner();
            Ok(EmbedSource::Repeat {
                value: build_expr(parts.next().unwrap())?,
                len: build_expr(parts.next().unwrap())?,
            })
        }
        _ => unreachable!("unexpected embed source rule {:?}", inner.as_rule()),
    }
}

fn build_struct(pair: Pair<'_, Rule>) -> Result<StructDecl, Diagnostic> {
    let mut public = false;
    let mut name = None;
    let mut fields = Vec::new();
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::visibility => public = true,
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::field_decl => {
                let mut field = inner.into_inner();
                fields.push(crate::ast::FieldDecl {
                    name: field.next().unwrap().as_str().to_owned(),
                    ty: build_type(field.next().unwrap())?,
                });
            }
            _ => {}
        }
    }
    Ok(StructDecl {
        public,
        name: name.unwrap(),
        fields,
    })
}

fn build_global(pair: Pair<'_, Rule>) -> Result<GlobalDecl, Diagnostic> {
    let mut public = false;
    let mut name = None;
    let mut ty = None;
    let mut value = None;
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::visibility => public = true,
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::ty => ty = Some(build_type(inner)?),
            Rule::expr => value = Some(build_expr(inner)?),
            _ => {}
        }
    }
    Ok(GlobalDecl {
        public,
        name: name.unwrap(),
        ty: ty.unwrap(),
        value: value.unwrap(),
    })
}

fn build_const(pair: Pair<'_, Rule>) -> Result<ConstDecl, Diagnostic> {
    let mut public = false;
    let mut name = None;
    let mut ty = None;
    let mut value = None;
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::visibility => public = true,
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::ty => ty = Some(build_type(inner)?),
            Rule::expr => value = Some(build_expr(inner)?),
            _ => {}
        }
    }
    Ok(ConstDecl {
        public,
        name: name.unwrap(),
        ty: ty.unwrap(),
        value: value.unwrap(),
    })
}

fn build_alias(pair: Pair<'_, Rule>) -> Result<AliasDecl, Diagnostic> {
    let mut public = false;
    let mut name = None;
    let mut ty = None;
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::visibility => public = true,
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::ty => ty = Some(build_type(inner)?),
            _ => {}
        }
    }
    Ok(AliasDecl {
        public,
        name: name.unwrap(),
        ty: ty.unwrap(),
    })
}

fn build_port(pair: Pair<'_, Rule>) -> Result<PortDecl, Diagnostic> {
    let mut public = false;
    let mut name = None;
    let mut ty = None;
    let mut value = None;
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::visibility => public = true,
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::ty => ty = Some(build_type(inner)?),
            Rule::expr => value = Some(build_expr(inner)?),
            _ => {}
        }
    }
    Ok(PortDecl {
        public,
        name: name.unwrap(),
        ty: ty.unwrap(),
        value: value.unwrap(),
    })
}

fn build_mmio(pair: Pair<'_, Rule>) -> Result<MmioDecl, Diagnostic> {
    let mut public = false;
    let mut volatile = false;
    let mut name = None;
    let mut ty = None;
    let mut value = None;
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::visibility => public = true,
            Rule::volatile_kw => volatile = true,
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::ty => ty = Some(build_type(inner)?),
            Rule::expr => value = Some(build_expr(inner)?),
            _ => {}
        }
    }
    Ok(MmioDecl {
        public,
        volatile,
        name: name.unwrap(),
        ty: ty.unwrap(),
        value: value.unwrap(),
    })
}

fn build_fn(file: &SourcePath, pair: Pair<'_, Rule>) -> Result<Function, Diagnostic> {
    let mut public = false;
    let mut attrs = Vec::new();
    let mut name = None;
    let mut params = Vec::new();
    let mut return_type = None;
    let mut body = None;
    let mut body_spans = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::attr => attrs.push(inner.as_str().to_owned()),
            Rule::visibility => {
                if public {
                    return Err(Diagnostic::new("duplicate visibility `pub` on function"));
                }
                public = true;
            }
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::params => params = build_params(inner)?,
            Rule::ret_ty => return_type = Some(build_type(inner.into_inner().next().unwrap())?),
            Rule::block => {
                let (statements, spans) = build_block(file, inner)?;
                body = Some(statements);
                body_spans = spans;
            }
            _ => {}
        }
    }

    Ok(Function {
        public,
        attrs,
        name: name.unwrap(),
        params,
        return_type,
        body: body.unwrap_or_default(),
        body_spans,
    })
}

fn build_extern(pair: Pair<'_, Rule>) -> Result<ExternFunction, Diagnostic> {
    let mut public = false;
    let mut name = None;
    let mut params = Vec::new();
    let mut return_type = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::visibility => public = true,
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::params => params = build_params(inner)?,
            Rule::ret_ty => return_type = Some(build_type(inner.into_inner().next().unwrap())?),
            _ => {}
        }
    }

    Ok(ExternFunction {
        public,
        name: name.unwrap(),
        params,
        return_type,
    })
}

fn build_params(pair: Pair<'_, Rule>) -> Result<Vec<Param>, Diagnostic> {
    pair.into_inner()
        .filter(|pair| pair.as_rule() == Rule::param)
        .map(|param| {
            let mut inner = param.into_inner();
            Ok(Param {
                name: inner.next().unwrap().as_str().to_owned(),
                ty: build_type(inner.next().unwrap())?,
            })
        })
        .collect()
}

fn build_block(
    file: &SourcePath,
    pair: Pair<'_, Rule>,
) -> Result<(Vec<Stmt>, Vec<crate::ast::StmtSpan>), Diagnostic> {
    let mut statements = Vec::new();
    let mut spans = Vec::new();
    for statement in pair.into_inner() {
        let (statement, span) = build_stmt(file, statement)?;
        statements.push(statement);
        spans.push(span);
    }
    Ok((statements, spans))
}

fn build_stmt(
    file: &SourcePath,
    pair: Pair<'_, Rule>,
) -> Result<(Stmt, crate::ast::StmtSpan), Diagnostic> {
    let span = pair_span(file, &pair);
    let references = collect_pair_references(file, &pair);
    let (statement, children) = match pair.as_rule() {
        Rule::let_stmt => {
            let mut inner = pair.into_inner();
            (
                Stmt::Let {
                    name: inner.next().unwrap().as_str().to_owned(),
                    ty: build_type(inner.next().unwrap())?,
                    value: build_expr(inner.next().unwrap())?,
                },
                Vec::new(),
            )
        }
        Rule::assign_stmt => {
            let mut inner = pair.into_inner();
            (
                Stmt::Assign {
                    target: build_place(inner.next().unwrap())?,
                    op: build_assign_op(inner.next().unwrap().as_str()),
                    value: build_expr(inner.next().unwrap())?,
                },
                Vec::new(),
            )
        }
        Rule::if_stmt => {
            let mut inner = pair.into_inner();
            let condition = build_expr(inner.next().unwrap())?;
            let (then_body, mut children) = build_block(file, inner.next().unwrap())?;
            let (else_body, else_spans) = match inner.next() {
                Some(pair) if pair.as_rule() == Rule::if_stmt => {
                    let (statement, span) = build_stmt(file, pair)?;
                    (vec![statement], vec![span])
                }
                Some(block) => build_block(file, block)?,
                None => (Vec::new(), Vec::new()),
            };
            children.extend(else_spans);
            (
                Stmt::If {
                    condition,
                    then_body,
                    else_body,
                },
                children,
            )
        }
        Rule::while_stmt => {
            let mut inner = pair.into_inner();
            let condition = build_expr(inner.next().unwrap())?;
            let (body, children) = build_block(file, inner.next().unwrap())?;
            (Stmt::While { condition, body }, children)
        }
        Rule::loop_stmt => {
            let (body, children) = build_block(file, pair.into_inner().next().unwrap())?;
            (Stmt::Loop { body }, children)
        }
        Rule::break_stmt => (Stmt::Break, Vec::new()),
        Rule::continue_stmt => (Stmt::Continue, Vec::new()),
        Rule::return_stmt => (
            Stmt::Return(pair.into_inner().next().map(build_expr).transpose()?),
            Vec::new(),
        ),
        Rule::asm_stmt => {
            let mut volatile = false;
            let mut inputs = Vec::new();
            let mut outputs = Vec::new();
            let mut clobbers = Vec::new();
            let mut lines = Vec::new();
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    Rule::volatile_kw => volatile = true,
                    Rule::asm_operands => {
                        for operand in inner.into_inner().flat_map(|pair| pair.into_inner()) {
                            if operand.as_rule() == Rule::asm_operand {
                                match build_asm_operand(operand)? {
                                    AsmOperand::Input(input) => inputs.push(input),
                                    AsmOperand::Output(output) => outputs.push(output),
                                    AsmOperand::Clobber(clobber) => clobbers.push(clobber),
                                }
                            }
                        }
                    }
                    Rule::asm_line => {
                        let line = inner.into_inner().next().unwrap();
                        lines.push(parse_string(line.as_str())?);
                    }
                    _ => {}
                }
            }
            (
                Stmt::Asm {
                    volatile,
                    inputs,
                    outputs,
                    clobbers,
                    lines,
                },
                Vec::new(),
            )
        }
        Rule::out_stmt => {
            let mut inner = pair.into_inner();
            (
                Stmt::Out {
                    port: inner.next().unwrap().as_str().to_owned(),
                    value: build_expr(inner.next().unwrap())?,
                },
                Vec::new(),
            )
        }
        Rule::expr_stmt => (
            Stmt::Expr(build_expr(pair.into_inner().next().unwrap())?),
            Vec::new(),
        ),
        _ => unreachable!("unexpected stmt rule {:?}", pair.as_rule()),
    };
    Ok((
        statement,
        crate::ast::StmtSpan {
            span,
            children,
            references,
        },
    ))
}

fn collect_pair_references(
    file: &SourcePath,
    pair: &Pair<'_, Rule>,
) -> Vec<crate::ast::SourceReference> {
    let mut references = Vec::new();
    collect_pair_references_inner(file, pair.clone(), &mut references);
    references
}

fn collect_pair_references_inner(
    file: &SourcePath,
    pair: Pair<'_, Rule>,
    references: &mut Vec<crate::ast::SourceReference>,
) {
    // Keep every parser node: semantic code can select the smallest exact source
    // construct without re-tokenizing diagnostic text later.
    references.push(crate::ast::SourceReference {
        text: pair.as_str().to_owned(),
        span: pair_span(file, &pair),
    });
    for inner in pair.into_inner() {
        collect_pair_references_inner(file, inner, references);
    }
}

fn build_place(pair: Pair<'_, Rule>) -> Result<Place, Diagnostic> {
    if pair.as_rule() == Rule::deref_place {
        return Ok(Place::Deref(Box::new(build_deref_operand(
            pair.into_inner().next().unwrap(),
        )?)));
    }
    let inner = pair.into_inner().next().unwrap();
    match inner.as_rule() {
        Rule::ident => Ok(Place::Ident(inner.as_str().to_owned())),
        Rule::deref_place => Ok(Place::Deref(Box::new(build_deref_operand(
            inner.into_inner().next().unwrap(),
        )?))),
        Rule::access_place => Ok(Place::Access(build_access_path(inner)?)),
        Rule::field_place => {
            let mut parts = inner.into_inner();
            Ok(Place::Field {
                base: parts.next().unwrap().as_str().to_owned(),
                field: parts.next().unwrap().as_str().to_owned(),
            })
        }
        Rule::index_place => {
            let mut parts = inner.into_inner();
            Ok(Place::Index {
                name: parts.next().unwrap().as_str().to_owned(),
                index: Box::new(build_expr(parts.next().unwrap())?),
            })
        }
        _ => unreachable!("unexpected place rule {:?}", inner.as_rule()),
    }
}

fn build_deref_operand(pair: Pair<'_, Rule>) -> Result<Expr, Diagnostic> {
    match pair.as_rule() {
        Rule::ident => Ok(Expr::Ident(pair.as_str().to_owned())),
        Rule::deref_access_operand => Ok(Expr::Access(build_access_path(pair)?)),
        Rule::deref_call_operand => build_call_expr(pair),
        _ => build_expr(pair),
    }
}

fn build_assign_op(op: &str) -> AssignOp {
    match op {
        "=" => AssignOp::Set,
        "+=" => AssignOp::Add,
        "-=" => AssignOp::Sub,
        "*=" => AssignOp::Mul,
        "/=" => AssignOp::Div,
        "%=" => AssignOp::Mod,
        "&=" => AssignOp::BitAnd,
        "|=" => AssignOp::BitOr,
        "^=" => AssignOp::BitXor,
        "<<=" => AssignOp::Shl,
        ">>=" => AssignOp::Shr,
        _ => unreachable!("unknown assign op {op}"),
    }
}

fn build_type(pair: Pair<'_, Rule>) -> Result<Type, Diagnostic> {
    let inner = pair.into_inner().next().unwrap();
    match inner.as_rule() {
        Rule::named_ty => Ok(Type::Named(inner.as_str().to_owned())),
        Rule::ptr_ty => Ok(Type::Ptr(Box::new(build_type(
            inner.into_inner().next().unwrap(),
        )?))),
        Rule::array_ty => {
            let mut parts = inner.into_inner();
            Ok(Type::Array {
                element: Box::new(build_type(parts.next().unwrap())?),
                len: Box::new(build_expr(parts.next().unwrap())?),
            })
        }
        _ => unreachable!("unexpected type rule {:?}", inner.as_rule()),
    }
}

fn build_expr(pair: Pair<'_, Rule>) -> Result<Expr, Diagnostic> {
    match pair.as_rule() {
        Rule::expr
        | Rule::logical_or
        | Rule::logical_and
        | Rule::bit_or
        | Rule::bit_xor
        | Rule::bit_and
        | Rule::equality
        | Rule::comparison
        | Rule::shift
        | Rule::additive
        | Rule::multiplicative => build_binary_chain(pair),
        Rule::unary => build_unary(pair),
        Rule::primary => build_expr(pair.into_inner().next().unwrap()),
        Rule::cast_expr => {
            let mut inner = pair.into_inner();
            Ok(Expr::Cast {
                ty: build_type(inner.next().unwrap())?,
                expr: Box::new(build_expr(inner.next().unwrap())?),
            })
        }
        Rule::in_expr => Ok(Expr::In(parse_in_port(pair.as_str())?)),
        Rule::addr_access_expr => Ok(Expr::AddressOfAccess(build_access_path(pair)?)),
        Rule::addr_index_expr => {
            let mut inner = pair.into_inner();
            Ok(Expr::AddressOfIndex {
                name: inner.next().unwrap().as_str().to_owned(),
                index: Box::new(build_expr(inner.next().unwrap())?),
            })
        }
        Rule::addr_field_expr => {
            let mut inner = pair.into_inner();
            Ok(Expr::AddressOfField {
                base: inner.next().unwrap().as_str().to_owned(),
                field: inner.next().unwrap().as_str().to_owned(),
            })
        }
        Rule::addr_expr => Ok(Expr::AddressOf(
            pair.into_inner().next().unwrap().as_str().to_owned(),
        )),
        Rule::deref_expr => Ok(Expr::Deref(Box::new(build_deref_operand(
            pair.into_inner().next().unwrap(),
        )?))),
        Rule::struct_lit => {
            let mut inner = pair.into_inner();
            let ty = inner.next().unwrap().as_str().to_owned();
            let fields = inner
                .next()
                .map(|fields| {
                    fields
                        .into_inner()
                        .map(|field| {
                            let mut parts = field.into_inner();
                            Ok((
                                parts.next().unwrap().as_str().to_owned(),
                                build_expr(parts.next().unwrap())?,
                            ))
                        })
                        .collect()
                })
                .unwrap_or_else(|| Ok(Vec::new()))?;
            Ok(Expr::StructInit { ty, fields })
        }
        Rule::access_expr => Ok(Expr::Access(build_access_path(pair)?)),
        Rule::index_expr => {
            let mut inner = pair.into_inner();
            Ok(Expr::Index {
                name: inner.next().unwrap().as_str().to_owned(),
                index: Box::new(build_expr(inner.next().unwrap())?),
            })
        }
        Rule::field_expr => {
            let mut inner = pair.into_inner();
            Ok(Expr::Field {
                base: inner.next().unwrap().as_str().to_owned(),
                field: inner.next().unwrap().as_str().to_owned(),
            })
        }
        Rule::array_lit => {
            let values = pair
                .into_inner()
                .next()
                .map(|args| args.into_inner().map(build_expr).collect())
                .unwrap_or_else(|| Ok(Vec::new()))?;
            Ok(Expr::Array(values))
        }
        Rule::call_expr => build_call_expr(pair),
        Rule::path_expr => Ok(Expr::Ident(pair.as_str().to_owned())),
        Rule::literal => build_expr(pair.into_inner().next().unwrap()),
        Rule::bool_lit => Ok(Expr::Bool(pair.as_str() == "true")),
        Rule::int_lit => build_int_lit(pair.as_str()),
        Rule::char_lit => Ok(Expr::Char(parse_char(pair.as_str())?)),
        Rule::string_lit => Ok(Expr::String(parse_string(pair.as_str())?)),
        _ => unreachable!("unexpected expr rule {:?}", pair.as_rule()),
    }
}

fn build_access_path(pair: Pair<'_, Rule>) -> Result<AccessPath, Diagnostic> {
    let mut inner = pair.into_inner();
    let root = inner
        .next()
        .ok_or_else(|| Diagnostic::new("access path is missing a root"))?
        .as_str()
        .to_owned();
    let mut segments = Vec::new();
    for suffix in inner {
        let segment = if suffix.as_rule() == Rule::access_suffix {
            suffix.into_inner().next().unwrap()
        } else {
            suffix
        };
        match segment.as_rule() {
            Rule::field_suffix => {
                let field = segment.into_inner().next().unwrap().as_str().to_owned();
                segments.push(AccessSegment::Field(field));
            }
            Rule::index_suffix => {
                let index = segment.into_inner().next().unwrap();
                segments.push(AccessSegment::Index(Box::new(build_expr(index)?)));
            }
            _ => unreachable!("unexpected access suffix {:?}", segment.as_rule()),
        }
    }
    Ok(AccessPath { root, segments })
}

fn build_call_expr(pair: Pair<'_, Rule>) -> Result<Expr, Diagnostic> {
    let mut inner = pair.into_inner();
    let path = split_path(inner.next().unwrap().as_str());
    let args = inner
        .next()
        .map(|args| args.into_inner().map(build_expr).collect())
        .unwrap_or_else(|| Ok(Vec::new()))?;
    Ok(Expr::Call { path, args })
}

fn build_binary_chain(pair: Pair<'_, Rule>) -> Result<Expr, Diagnostic> {
    let mut inner = pair.into_inner();
    let Some(first) = inner.next() else {
        return Err(Diagnostic::new("empty expression"));
    };
    let mut expr = build_expr(first)?;
    while let Some(op) = inner.next() {
        let right = build_expr(inner.next().unwrap())?;
        expr = Expr::Binary {
            left: Box::new(expr),
            op: build_binary_op(op.as_str().trim()),
            right: Box::new(right),
        };
    }
    Ok(expr)
}

fn build_unary(pair: Pair<'_, Rule>) -> Result<Expr, Diagnostic> {
    let mut ops = Vec::new();
    let mut primary = None;
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::unary_op {
            ops.push(inner.as_str().to_owned());
        } else {
            primary = Some(build_expr(inner)?);
        }
    }
    let mut expr = primary.unwrap();
    for op in ops.into_iter().rev() {
        expr = Expr::Unary {
            op: match op.as_str() {
                "-" => UnaryOp::Neg,
                "~" => UnaryOp::BitNot,
                "!" => UnaryOp::Not,
                _ => unreachable!("unknown unary op {op}"),
            },
            expr: Box::new(expr),
        };
    }
    Ok(expr)
}

fn build_binary_op(op: &str) -> BinaryOp {
    match op {
        "*" => BinaryOp::Mul,
        "/" => BinaryOp::Div,
        "%" => BinaryOp::Mod,
        "+" => BinaryOp::Add,
        "-" => BinaryOp::Sub,
        "<<" => BinaryOp::Shl,
        ">>" => BinaryOp::Shr,
        "<" => BinaryOp::Lt,
        "<=" => BinaryOp::Le,
        ">" => BinaryOp::Gt,
        ">=" => BinaryOp::Ge,
        "==" => BinaryOp::Eq,
        "!=" => BinaryOp::Ne,
        "&" => BinaryOp::BitAnd,
        "^" => BinaryOp::BitXor,
        "|" => BinaryOp::BitOr,
        "&&" => BinaryOp::And,
        "||" => BinaryOp::Or,
        _ => unreachable!("unknown binary op {op}"),
    }
}

fn split_path(path: &str) -> Vec<String> {
    path.split('.').map(str::to_owned).collect()
}

fn parse_in_port(value: &str) -> Result<String, Diagnostic> {
    value
        .strip_prefix("in")
        .map(str::trim_start)
        .filter(|port| !port.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Diagnostic::new("expected port after `in`"))
}

enum AsmOperand {
    Input(AsmInput),
    Output(AsmOutput),
    Clobber(String),
}

fn build_asm_operand(pair: Pair<'_, Rule>) -> Result<AsmOperand, Diagnostic> {
    let inner = pair.into_inner().next().unwrap();
    match inner.as_rule() {
        Rule::asm_input => {
            let mut parts = inner.into_inner();
            let name = parts.next().unwrap().as_str().to_owned();
            let ty = build_type(parts.next().unwrap())?;
            let class = parts
                .next()
                .map(|part| part.as_str().to_owned())
                .unwrap_or_else(|| infer_asm_operand_class(&ty));
            validate_asm_operand_class(&ty, &class)?;
            Ok(AsmOperand::Input(AsmInput { name, ty, class }))
        }
        Rule::asm_output => {
            let mut parts = inner.into_inner();
            let name = parts.next().unwrap().as_str().to_owned();
            let ty = build_type(parts.next().unwrap())?;
            let class = parts
                .next()
                .map(|part| part.as_str().to_owned())
                .unwrap_or_else(|| infer_asm_operand_class(&ty));
            validate_asm_operand_class(&ty, &class)?;
            Ok(AsmOperand::Output(AsmOutput { name, ty, class }))
        }
        Rule::asm_clobber => {
            let clobber = inner.into_inner().next().unwrap().as_str();
            if !is_allowed_asm_clobber(clobber) {
                return Err(Diagnostic::new(format!(
                    "unknown inline asm clobber `{clobber}`"
                )));
            }
            Ok(AsmOperand::Clobber(clobber.to_owned()))
        }
        _ => unreachable!("unexpected inline asm operand {:?}", inner.as_rule()),
    }
}

fn infer_asm_operand_class(ty: &Type) -> String {
    match type_storage_size(ty) {
        Some(1) => "reg8",
        Some(2) => "reg16",
        Some(3) => "reg24",
        _ => "mem",
    }
    .to_owned()
}

fn validate_asm_operand_class(ty: &Type, class: &str) -> Result<(), Diagnostic> {
    let ok = match class {
        "reg8" => type_storage_size(ty) == Some(1),
        "reg16" => type_storage_size(ty) == Some(2),
        "reg24" => type_storage_size(ty) == Some(3),
        "mem" | "imm" => true,
        _ => false,
    };
    if !ok {
        return Err(Diagnostic::new(format!(
            "inline asm operand class `{class}` is incompatible with type `{ty:?}`"
        )));
    }
    Ok(())
}

fn type_storage_size(ty: &Type) -> Option<u8> {
    match ty {
        Type::Named(name) if name == "u8" || name == "i8" || name == "bool" => Some(1),
        Type::Named(name) if name == "u16" || name == "i16" => Some(2),
        Type::Named(name) if name == "u24" || name == "i24" || name == "ptr24" => Some(3),
        Type::Ptr(_) => Some(3),
        Type::Named(_) | Type::Array { .. } => None,
    }
}

fn is_allowed_asm_clobber(clobber: &str) -> bool {
    let avr_register = clobber
        .strip_prefix('r')
        .and_then(|index| index.parse::<u8>().ok())
        .is_some_and(|index| index < 32);
    if avr_register {
        return true;
    }
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

fn parse_int(text: &str) -> Result<i64, Diagnostic> {
    let digits = strip_int_suffix(text).0;
    if let Some(hex) = digits.strip_prefix("0x") {
        i64::from_str_radix(hex, 16)
    } else if let Some(bin) = digits.strip_prefix("0b") {
        i64::from_str_radix(bin, 2)
    } else {
        digits.parse()
    }
    .map_err(|_| Diagnostic::new(format!("invalid integer literal `{text}`")))
}

fn build_int_lit(text: &str) -> Result<Expr, Diagnostic> {
    let (digits, suffix) = strip_int_suffix(text);
    let value = parse_int(digits)?;
    if let Some(suffix) = suffix {
        Ok(Expr::TypedInt(value, Type::Named(suffix.to_owned())))
    } else {
        Ok(Expr::Int(value))
    }
}

fn strip_int_suffix(text: &str) -> (&str, Option<&str>) {
    for suffix in ["u24", "i24", "u16", "i16", "u8", "i8"] {
        if let Some(digits) = text.strip_suffix(suffix) {
            return (digits, Some(suffix));
        }
    }
    (text, None)
}

fn parse_char(text: &str) -> Result<u8, Diagnostic> {
    let body = &text[1..text.len() - 1];
    let value = parse_escaped(body)?;
    let bytes = value.into_bytes();
    if bytes.len() != 1 {
        return Err(Diagnostic::new(
            "character literal must contain exactly one byte",
        ));
    }
    Ok(bytes[0])
}

fn parse_string(text: &str) -> Result<String, Diagnostic> {
    parse_escaped(&text[1..text.len() - 1])
}

fn parse_escaped(text: &str) -> Result<String, Diagnostic> {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        out.push(match chars.next() {
            Some('n') => '\n',
            Some('0') => '\0',
            Some('t') => '\t',
            Some('\\') => '\\',
            Some('\'') => '\'',
            Some('"') => '"',
            Some(other) => return Err(Diagnostic::new(format!("unknown escape `\\{other}`"))),
            None => return Err(Diagnostic::new("unexpected end of escape")),
        });
    }
    Ok(out)
}

fn pest_error(file: &SourcePath, error: pest::error::Error<Rule>) -> Diagnostic {
    let ((line, column), (end_line, end_column)) = match error.line_col {
        pest::error::LineColLocation::Pos((line, column)) => {
            ((line, column), (line, column.saturating_add(1)))
        }
        pest::error::LineColLocation::Span(start, end) => (start, end),
    };
    Diagnostic::at_span(
        SourceSpan {
            file: source_path_owned(file),
            start: SourcePosition { line, column },
            end: SourcePosition {
                line: end_line,
                column: end_column,
            },
        },
        error.to_string(),
    )
}

fn pair_span(file: &SourcePath, pair: &Pair<'_, Rule>) -> SourceSpan {
    let (line, column) = pair.as_span().start_pos().line_col();
    let (end_line, end_column) = pair.as_span().end_pos().line_col();
    SourceSpan {
        file: source_path_owned(file),
        start: SourcePosition { line, column },
        end: SourcePosition {
            line: end_line,
            column: end_column,
        },
    }
}

#[cfg(test)]
mod tests;
