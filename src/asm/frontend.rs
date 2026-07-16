//! Shared, target-independent assembly syntax and semantic trees.
//!
//! Parsing is intentionally split in two stages. [`parse_assembly_syntax`]
//! retains preprocessing constructs, while [`lower_parsed_assembly`] only
//! accepts syntax from which a target assembler can safely consume semantic
//! assembly items.

use crate::compat::prelude::*;
use crate::diagnostic::{Diagnostic, SourceLocation, SourcePosition, SourceSpan};
use pest::{Parser, iterators::Pair};
use pest_derive::Parser;

#[derive(Parser)]
#[grammar = "asm/assembly.pest"]
struct AssemblyParser;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssemblyProgram {
    pub items: Vec<LocatedAssemblyItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocatedAssemblyItem {
    pub location: SourceLocation,
    pub kind: AssemblyItem,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssemblyItem {
    Label(String),
    Equ {
        name: String,
        value: AssemblyExpression,
    },
    Section(String),
    Org(AssemblyExpression),
    Data {
        width: DataWidth,
        values: Vec<AssemblyDataValue>,
    },
    Instruction(AssemblyInstruction),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssemblyInstruction {
    pub mnemonic: String,
    pub operands: Vec<String>,
}

impl AssemblyInstruction {
    pub fn to_text(&self) -> String {
        if self.operands.is_empty() {
            return self.mnemonic.clone();
        }
        format!("{} {}", self.mnemonic, self.operands.join(", "))
    }

    /// Render canonical input for the existing CPU-specific encoders.
    ///
    /// Commas are compact because several addressing dialects, notably 6502,
    /// use a top-level comma as part of one indexed operand rather than as an
    /// instruction-operand separator.
    pub fn to_encoder_text(&self) -> String {
        if self.operands.is_empty() {
            return self.mnemonic.clone();
        }
        format!("{} {}", self.mnemonic, self.operands.join(","))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataWidth {
    Byte,
    Word,
}

impl DataWidth {
    pub const fn bytes(self) -> usize {
        match self {
            Self::Byte => 1,
            Self::Word => 2,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssemblyDataValue {
    Expression(AssemblyExpression),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssemblyExpression {
    Symbol(String),
    Current,
    Number(u64),
    Unary {
        operator: AssemblyUnaryOperator,
        expression: Box<AssemblyExpression>,
    },
    Binary {
        operator: AssemblyBinaryOperator,
        left: Box<AssemblyExpression>,
        right: Box<AssemblyExpression>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssemblyUnaryOperator {
    Plus,
    Negate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssemblyBinaryOperator {
    Add,
    Subtract,
    Multiply,
    BitAnd,
    BitOr,
    BitXor,
}

/// A parsed source file before include, define, macro, or conditional expansion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedAssembly {
    pub source_name: String,
    pub items: Vec<LocatedParsedAssemblyItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocatedParsedAssemblyItem {
    pub location: SourceLocation,
    pub kind: ParsedAssemblyItem,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedAssemblyItem {
    Include {
        path: String,
    },
    Define {
        name: String,
        value: String,
    },
    MacroDefinition {
        name: String,
        parameters: Vec<String>,
        body: Vec<LocatedParsedAssemblyItem>,
    },
    MacroInvocation {
        name: String,
        arguments: Vec<String>,
    },
    Conditional {
        condition: String,
        then_items: Vec<LocatedParsedAssemblyItem>,
        else_items: Vec<LocatedParsedAssemblyItem>,
    },
    Label(String),
    Equ {
        name: String,
        value: String,
    },
    Section(String),
    Org(String),
    Data {
        width: DataWidth,
        values: Vec<ParsedAssemblyDataValue>,
    },
    Directive {
        name: String,
        arguments: Vec<String>,
    },
    Instruction(AssemblyInstruction),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedAssemblyDataValue {
    Expression(String),
    StringLiteral(String),
}

/// Parse assembly and preserve preprocessing constructs as structural nodes.
pub fn parse_assembly_syntax(
    source_name: &str,
    source: &str,
) -> Result<ParsedAssembly, Diagnostic> {
    let mut parsed = AssemblyParser::parse(Rule::assembly, source)
        .map_err(|error| pest_diagnostic(source_name, error))?;
    let root = parsed
        .next()
        .ok_or_else(|| Diagnostic::new("assembly parser produced no syntax tree"))?;
    let mut items = Vec::new();
    for pair in root.into_inner() {
        append_syntax_pair(source_name, pair, &mut items)?;
    }
    Ok(ParsedAssembly {
        source_name: source_name.to_owned(),
        items,
    })
}

/// Parse one target-independent assembly expression.
pub fn parse_assembly_expression(text: &str) -> Result<AssemblyExpression, Diagnostic> {
    parse_expression_at(
        "<assembly expression>",
        text,
        &SourceLocation {
            file: "<assembly expression>".into(),
            line: 1,
            column: 1,
        },
    )
}

/// Convert preprocessed syntax into the normalized semantic assembly tree.
///
/// Includes, defines, macros, invocations, and conditionals are rejected: a
/// preprocessor must remove those nodes before semantic lowering. Generic
/// target-specific directives are also rejected unless normalized beforehand.
pub fn lower_parsed_assembly(parsed: ParsedAssembly) -> Result<AssemblyProgram, Diagnostic> {
    let mut items = Vec::new();
    for item in parsed.items {
        let location = item.location;
        let kind = match item.kind {
            ParsedAssemblyItem::Label(name) => AssemblyItem::Label(name),
            ParsedAssemblyItem::Equ { name, value } => AssemblyItem::Equ {
                name,
                value: parse_expression_at(&parsed.source_name, &value, &location)?,
            },
            ParsedAssemblyItem::Section(name) => AssemblyItem::Section(name),
            ParsedAssemblyItem::Org(value) => {
                AssemblyItem::Org(parse_expression_at(&parsed.source_name, &value, &location)?)
            }
            ParsedAssemblyItem::Data { width, values } => {
                let mut lowered = Vec::new();
                for value in values {
                    lowered.push(match value {
                        ParsedAssemblyDataValue::Expression(expression) => {
                            AssemblyDataValue::Expression(parse_expression_at(
                                &parsed.source_name,
                                &expression,
                                &location,
                            )?)
                        }
                        ParsedAssemblyDataValue::StringLiteral(string) => AssemblyDataValue::Bytes(
                            decode_quoted_bytes(&string)
                                .map_err(|message| Diagnostic::at(location.clone(), message))?,
                        ),
                    });
                }
                AssemblyItem::Data {
                    width,
                    values: lowered,
                }
            }
            ParsedAssemblyItem::Instruction(instruction) => AssemblyItem::Instruction(instruction),
            ParsedAssemblyItem::Include { .. }
            | ParsedAssemblyItem::Define { .. }
            | ParsedAssemblyItem::MacroDefinition { .. }
            | ParsedAssemblyItem::MacroInvocation { .. }
            | ParsedAssemblyItem::Conditional { .. } => {
                return Err(Diagnostic::at(
                    location,
                    "assembly preprocessing node survived semantic lowering",
                ));
            }
            ParsedAssemblyItem::Directive { name, .. } => {
                return Err(Diagnostic::at(
                    location,
                    format!("unsupported shared assembly directive `{name}`"),
                ));
            }
        };
        items.push(LocatedAssemblyItem { location, kind });
    }
    Ok(AssemblyProgram { items })
}

fn append_syntax_pair(
    source_name: &str,
    pair: Pair<'_, Rule>,
    output: &mut Vec<LocatedParsedAssemblyItem>,
) -> Result<(), Diagnostic> {
    match pair.as_rule() {
        Rule::statement_line => {
            let statement = pair
                .into_inner()
                .find(|inner| inner.as_rule() == Rule::statement)
                .ok_or_else(|| Diagnostic::new("assembly statement had no contents"))?;
            append_statement(source_name, statement, output)
        }
        Rule::macro_definition => append_macro(source_name, pair, output),
        Rule::conditional_block => append_conditional(source_name, pair, output),
        Rule::EOI => Ok(()),
        _ => Ok(()),
    }
}

fn append_macro(
    source_name: &str,
    pair: Pair<'_, Rule>,
    output: &mut Vec<LocatedParsedAssemblyItem>,
) -> Result<(), Diagnostic> {
    let mut inner = pair.into_inner();
    let header = inner
        .next()
        .ok_or_else(|| Diagnostic::new("macro definition had no header"))?;
    let location = pair_location(source_name, &header);
    let signature = header
        .into_inner()
        .find(|pair| pair.as_rule() == Rule::statement)
        .map(|pair| pair.as_str().trim())
        .ok_or_else(|| Diagnostic::at(location.clone(), "macro definition had no signature"))?;
    let (name, parameters) = parse_macro_signature(signature)
        .map_err(|message| Diagnostic::at(location.clone(), message))?;
    let mut body = Vec::new();
    for item in inner {
        append_syntax_pair(source_name, item, &mut body)?;
    }
    output.push(LocatedParsedAssemblyItem {
        location,
        kind: ParsedAssemblyItem::MacroDefinition {
            name,
            parameters,
            body,
        },
    });
    Ok(())
}

fn append_conditional(
    source_name: &str,
    pair: Pair<'_, Rule>,
    output: &mut Vec<LocatedParsedAssemblyItem>,
) -> Result<(), Diagnostic> {
    let mut inner = pair.into_inner();
    let header = inner
        .next()
        .ok_or_else(|| Diagnostic::new("conditional block had no header"))?;
    let location = pair_location(source_name, &header);
    let condition = header
        .into_inner()
        .find(|pair| pair.as_rule() == Rule::statement)
        .map(|pair| pair.as_str().trim().to_owned())
        .ok_or_else(|| Diagnostic::at(location.clone(), "conditional block had no condition"))?;
    let mut then_items = Vec::new();
    let mut else_items = Vec::new();
    for item in inner {
        if item.as_rule() == Rule::else_clause {
            for alternative in item.into_inner() {
                append_syntax_pair(source_name, alternative, &mut else_items)?;
            }
        } else {
            append_syntax_pair(source_name, item, &mut then_items)?;
        }
    }
    output.push(LocatedParsedAssemblyItem {
        location,
        kind: ParsedAssemblyItem::Conditional {
            condition,
            then_items,
            else_items,
        },
    });
    Ok(())
}

fn append_statement(
    source_name: &str,
    pair: Pair<'_, Rule>,
    output: &mut Vec<LocatedParsedAssemblyItem>,
) -> Result<(), Diagnostic> {
    let location = pair_location(source_name, &pair);
    let text = pair.as_str().trim_end();

    if let Some(colon) = top_level_label_colon(text) {
        let name = text[..colon].trim();
        if !name.is_empty() {
            output.push(LocatedParsedAssemblyItem {
                location: location.clone(),
                kind: ParsedAssemblyItem::Label(name.to_owned()),
            });
            let tail_start = colon + 1;
            let tail = text[tail_start..].trim_start();
            if !tail.is_empty() {
                let whitespace = text[tail_start..].len() - text[tail_start..].trim_start().len();
                let tail_location = offset_location(&location, &text[..tail_start + whitespace]);
                append_statement_text(tail, tail_location, output)?;
            }
            return Ok(());
        }
    }

    append_statement_text(text, location, output)
}

fn append_statement_text(
    text: &str,
    location: SourceLocation,
    output: &mut Vec<LocatedParsedAssemblyItem>,
) -> Result<(), Diagnostic> {
    let (head, rest) = split_head(text);
    if head.is_empty() {
        return Ok(());
    }

    if matches_keyword(head, "include") || matches_keyword(head, "%include") {
        let path = decode_include_path(rest)
            .map_err(|message| Diagnostic::at(location.clone(), message))?;
        return push_parsed(output, location, ParsedAssemblyItem::Include { path });
    }

    if matches_keyword(head, "%define") {
        let (name, value) = split_head(rest);
        if name.is_empty() || value.trim().is_empty() {
            return Err(Diagnostic::at(location, "expected `%define NAME value`"));
        }
        return push_parsed(
            output,
            location,
            ParsedAssemblyItem::Define {
                name: name.to_owned(),
                value: value.trim().to_owned(),
            },
        );
    }

    if let Some(invocation) = head.strip_prefix('%') {
        if invocation.is_empty() {
            return Err(Diagnostic::at(location, "missing macro invocation name"));
        }
        let (name, argument_text) = if let Some(open) = invocation.find('(') {
            let name = invocation[..open].trim();
            let joined = format!("{}{}", &invocation[open..], rest);
            (name.to_owned(), joined)
        } else {
            (invocation.to_owned(), rest.to_owned())
        };
        let argument_text = strip_enclosing_delimiters(argument_text.trim(), '(', ')');
        let arguments = split_delimited(argument_text, ',')
            .map_err(|message| Diagnostic::at(location.clone(), message))?;
        return push_parsed(
            output,
            location,
            ParsedAssemblyItem::MacroInvocation { name, arguments },
        );
    }

    let normalized_head = head.strip_prefix('.').unwrap_or(head);
    if normalized_head.eq_ignore_ascii_case("equ") {
        let parts = split_delimited(rest, ',')
            .map_err(|message| Diagnostic::at(location.clone(), message))?;
        let (name, value) = if parts.len() == 2 {
            (parts[0].clone(), parts[1].clone())
        } else {
            let (name, value) = split_head(rest);
            (name.to_owned(), value.trim().to_owned())
        };
        if name.is_empty() || value.is_empty() {
            return Err(Diagnostic::at(location, "expected `.equ NAME, expression`"));
        }
        return push_parsed(output, location, ParsedAssemblyItem::Equ { name, value });
    }

    if let Some((name, value)) = split_infix_equ(text) {
        return push_parsed(
            output,
            location,
            ParsedAssemblyItem::Equ {
                name: name.to_owned(),
                value: value.to_owned(),
            },
        );
    }
    if let Some((name, value)) = split_top_level_once(text, '=') {
        if !name.trim().is_empty()
            && !name.trim().chars().any(char::is_whitespace)
            && !value.trim().is_empty()
        {
            return push_parsed(
                output,
                location,
                ParsedAssemblyItem::Equ {
                    name: name.trim().to_owned(),
                    value: value.trim().to_owned(),
                },
            );
        }
    }

    if normalized_head.eq_ignore_ascii_case("section") {
        if rest.trim().is_empty() {
            return Err(Diagnostic::at(
                location,
                "section directive requires a name",
            ));
        }
        return push_parsed(
            output,
            location,
            ParsedAssemblyItem::Section(rest.trim().to_owned()),
        );
    }
    if normalized_head.eq_ignore_ascii_case("org") {
        if rest.trim().is_empty() {
            return Err(Diagnostic::at(
                location,
                "org directive requires an expression",
            ));
        }
        return push_parsed(
            output,
            location,
            ParsedAssemblyItem::Org(rest.trim().to_owned()),
        );
    }

    if let Some(width) = data_width(normalized_head) {
        let fields = split_delimited(rest, ',')
            .map_err(|message| Diagnostic::at(location.clone(), message))?;
        if fields.is_empty() {
            return Err(Diagnostic::at(location, "data directive requires a value"));
        }
        let values = fields
            .into_iter()
            .map(|field| {
                if is_quoted(&field) {
                    ParsedAssemblyDataValue::StringLiteral(field)
                } else {
                    ParsedAssemblyDataValue::Expression(field)
                }
            })
            .collect();
        return push_parsed(output, location, ParsedAssemblyItem::Data { width, values });
    }

    let operands =
        split_delimited(rest, ',').map_err(|message| Diagnostic::at(location.clone(), message))?;
    if head.starts_with('.') || is_named_generic_directive(normalized_head) {
        return push_parsed(
            output,
            location,
            ParsedAssemblyItem::Directive {
                name: normalized_head.to_owned(),
                arguments: operands,
            },
        );
    }
    push_parsed(
        output,
        location,
        ParsedAssemblyItem::Instruction(AssemblyInstruction {
            mnemonic: head.to_owned(),
            operands,
        }),
    )
}

fn push_parsed(
    output: &mut Vec<LocatedParsedAssemblyItem>,
    location: SourceLocation,
    kind: ParsedAssemblyItem,
) -> Result<(), Diagnostic> {
    output.push(LocatedParsedAssemblyItem { location, kind });
    Ok(())
}

fn parse_macro_signature(text: &str) -> Result<(String, Vec<String>), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("missing macro name".to_owned());
    }
    if let Some(open) = top_level_open(text, '(') {
        let name = text[..open].trim();
        let parameters = strip_enclosing_delimiters(text[open..].trim(), '(', ')');
        if name.is_empty() || parameters == text[open..].trim() {
            return Err("invalid macro parameter list".to_owned());
        }
        return Ok((name.to_owned(), split_delimited(parameters, ',')?));
    }
    let (name, parameters) = split_head(text);
    let parameters = strip_enclosing_delimiters(parameters.trim(), '(', ')');
    Ok((name.to_owned(), split_delimited(parameters, ',')?))
}

fn parse_expression_at(
    source_name: &str,
    text: &str,
    location: &SourceLocation,
) -> Result<AssemblyExpression, Diagnostic> {
    let mut parsed = AssemblyParser::parse(Rule::expression, text).map_err(|error| {
        let mut diagnostic = pest_diagnostic(source_name, error);
        if let Some(span) = &mut diagnostic.span {
            span.file = location.file.clone();
            span.start.line = span
                .start
                .line
                .saturating_add(location.line.saturating_sub(1));
            span.end.line = span
                .end
                .line
                .saturating_add(location.line.saturating_sub(1));
            if span.start.line == location.line {
                span.start.column = span
                    .start
                    .column
                    .saturating_add(location.column.saturating_sub(1));
            }
            if span.end.line == location.line {
                span.end.column = span
                    .end
                    .column
                    .saturating_add(location.column.saturating_sub(1));
            }
        }
        diagnostic
    })?;
    let expression = parsed
        .next()
        .and_then(|pair| {
            pair.into_inner()
                .find(|inner| inner.as_rule() == Rule::bit_or)
        })
        .ok_or_else(|| Diagnostic::at(location.clone(), "expression parser produced no value"))?;
    build_expression(expression).map_err(|message| Diagnostic::at(location.clone(), message))
}

fn build_expression(pair: Pair<'_, Rule>) -> Result<AssemblyExpression, String> {
    match pair.as_rule() {
        Rule::bit_or => build_binary(pair, Rule::or_op),
        Rule::bit_xor => build_binary(pair, Rule::xor_op),
        Rule::bit_and => build_binary(pair, Rule::and_op),
        Rule::additive => build_binary(pair, Rule::add_op),
        Rule::multiplicative => build_binary(pair, Rule::multiply_op),
        Rule::unary => {
            let mut operators = Vec::new();
            let mut value = None;
            for inner in pair.into_inner() {
                match inner.as_rule() {
                    Rule::unary_op => operators.push(inner.as_str()),
                    Rule::primary => value = Some(build_expression(inner)?),
                    _ => {}
                }
            }
            let mut value = value.ok_or_else(|| "unary expression had no operand".to_owned())?;
            for operator in operators.into_iter().rev() {
                value = AssemblyExpression::Unary {
                    operator: match operator {
                        "+" => AssemblyUnaryOperator::Plus,
                        "-" => AssemblyUnaryOperator::Negate,
                        _ => return Err(format!("unsupported unary operator `{operator}`")),
                    },
                    expression: Box::new(value),
                };
            }
            Ok(value)
        }
        Rule::primary => {
            let inner = pair
                .into_inner()
                .next()
                .ok_or_else(|| "primary expression had no value".to_owned())?;
            build_expression(inner)
        }
        Rule::number => Ok(AssemblyExpression::Number(parse_number(pair.as_str())?)),
        Rule::current => Ok(AssemblyExpression::Current),
        Rule::symbol => Ok(AssemblyExpression::Symbol(pair.as_str().to_owned())),
        _ => Err(format!("unexpected expression rule {:?}", pair.as_rule())),
    }
}

fn build_binary(pair: Pair<'_, Rule>, operator_rule: Rule) -> Result<AssemblyExpression, String> {
    let mut inner = pair.into_inner();
    let first = inner
        .next()
        .ok_or_else(|| "binary expression had no left operand".to_owned())?;
    let mut expression = build_expression(first)?;
    while let Some(operator) = inner.next() {
        if operator.as_rule() != operator_rule {
            continue;
        }
        let right = inner
            .next()
            .ok_or_else(|| "binary expression had no right operand".to_owned())?;
        expression = AssemblyExpression::Binary {
            operator: match operator.as_str() {
                "+" => AssemblyBinaryOperator::Add,
                "-" => AssemblyBinaryOperator::Subtract,
                "*" => AssemblyBinaryOperator::Multiply,
                "&" => AssemblyBinaryOperator::BitAnd,
                "|" => AssemblyBinaryOperator::BitOr,
                "^" => AssemblyBinaryOperator::BitXor,
                other => return Err(format!("unsupported binary operator `{other}`")),
            },
            left: Box::new(expression),
            right: Box::new(build_expression(right)?),
        };
    }
    Ok(expression)
}

fn parse_number(text: &str) -> Result<u64, String> {
    let (digits, radix) =
        if let Some(value) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (value, 16)
        } else if let Some(value) = text.strip_prefix('$') {
            (value, 16)
        } else if let Some(value) = text.strip_prefix('>') {
            (
                value
                    .strip_suffix('h')
                    .or_else(|| value.strip_suffix('H'))
                    .unwrap_or(value),
                16,
            )
        } else if let Some(value) = text.strip_suffix('h').or_else(|| text.strip_suffix('H')) {
            (value, 16)
        } else if let Some(value) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
            (value, 2)
        } else if let Some(value) = text.strip_prefix('%') {
            (value, 2)
        } else if let Some(value) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
            (value, 8)
        } else {
            (text, 10)
        };
    u64::from_str_radix(digits, radix)
        .map_err(|_| format!("invalid assembly integer literal `{text}`"))
}

fn decode_include_path(text: &str) -> Result<String, String> {
    let text = text.trim();
    if !is_quoted(text) {
        return Err("expected `include \"path\"`".to_owned());
    }
    let bytes = decode_quoted_bytes(text)?;
    String::from_utf8(bytes).map_err(|_| "include path is not valid UTF-8".to_owned())
}

fn decode_quoted_bytes(text: &str) -> Result<Vec<u8>, String> {
    let mut chars = text.chars();
    let quote = chars
        .next()
        .ok_or_else(|| "empty string literal".to_owned())?;
    if !matches!(quote, '\'' | '"') || !text.ends_with(quote) || text.len() < 2 {
        return Err("malformed string literal".to_owned());
    }
    let end = text.len() - quote.len_utf8();
    let mut output = String::new();
    let mut contents = text[quote.len_utf8()..end].chars();
    while let Some(character) = contents.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        let escaped = contents
            .next()
            .ok_or_else(|| "unexpected end of string escape".to_owned())?;
        output.push(match escaped {
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '0' => '\0',
            other => return Err(format!("unknown string escape `\\{other}`")),
        });
    }
    Ok(output.into_bytes())
}

fn split_head(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    match text.find(char::is_whitespace) {
        Some(index) => (&text[..index], text[index..].trim_start()),
        None => (text, ""),
    }
}

fn matches_keyword(text: &str, keyword: &str) -> bool {
    text.strip_prefix('.')
        .unwrap_or(text)
        .eq_ignore_ascii_case(keyword)
}

fn is_named_generic_directive(name: &str) -> bool {
    [
        "align", "assume", "bits", "cpu", "extern", "global", "globl", "public",
    ]
    .iter()
    .any(|directive| name.eq_ignore_ascii_case(directive))
}

fn data_width(directive: &str) -> Option<DataWidth> {
    if directive.eq_ignore_ascii_case("db")
        || directive.eq_ignore_ascii_case("defb")
        || directive.eq_ignore_ascii_case("byte")
    {
        Some(DataWidth::Byte)
    } else if directive.eq_ignore_ascii_case("dw")
        || directive.eq_ignore_ascii_case("defw")
        || directive.eq_ignore_ascii_case("word")
    {
        Some(DataWidth::Word)
    } else {
        None
    }
}

fn is_quoted(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
}

fn split_delimited(text: &str, delimiter: char) -> Result<Vec<String>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' | '[' | '{' => stack.push(character),
            ')' | ']' | '}' => {
                let expected = match character {
                    ')' => '(',
                    ']' => '[',
                    '}' => '{',
                    _ => unreachable!(),
                };
                if stack.pop() != Some(expected) {
                    return Err(format!("unbalanced delimiter `{character}`"));
                }
            }
            value if value == delimiter && stack.is_empty() => {
                let part = text[start..index].trim();
                if part.is_empty() {
                    return Err("empty item in comma-separated list".to_owned());
                }
                parts.push(part.to_owned());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if quote.is_some() || !stack.is_empty() {
        return Err("unbalanced quoted string or delimiter".to_owned());
    }
    let final_part = text[start..].trim();
    if final_part.is_empty() {
        return Err("empty item in comma-separated list".to_owned());
    }
    parts.push(final_part.to_owned());
    Ok(parts)
}

fn top_level_label_colon(text: &str) -> Option<usize> {
    let index = top_level_character(text, ':')?;
    let candidate = text[..index].trim();
    (!candidate.is_empty() && !candidate.chars().any(char::is_whitespace)).then_some(index)
}

fn split_infix_equ(text: &str) -> Option<(&str, &str)> {
    let mut offset = 0;
    for part in text.split_whitespace() {
        let relative = text[offset..].find(part)?;
        let start = offset + relative;
        if part.eq_ignore_ascii_case("equ") {
            let name = text[..start].trim();
            let value = text[start + part.len()..].trim();
            if !name.is_empty() && !value.is_empty() && !name.chars().any(char::is_whitespace) {
                return Some((name, value));
            }
        }
        offset = start + part.len();
    }
    None
}

fn split_top_level_once(text: &str, needle: char) -> Option<(&str, &str)> {
    let index = top_level_character(text, needle)?;
    Some((&text[..index], &text[index + needle.len_utf8()..]))
}

fn top_level_open(text: &str, needle: char) -> Option<usize> {
    top_level_character(text, needle)
}

fn top_level_character(text: &str, needle: char) -> Option<usize> {
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            value if value == needle && stack.is_empty() => return Some(index),
            '(' | '[' | '{' => stack.push(character),
            ')' | ']' | '}' => {
                stack.pop();
            }
            _ => {}
        }
    }
    None
}

fn strip_enclosing_delimiters(text: &str, open: char, close: char) -> &str {
    text.strip_prefix(open)
        .and_then(|inner| inner.strip_suffix(close))
        .unwrap_or(text)
        .trim()
}

fn pair_location(source_name: &str, pair: &Pair<'_, Rule>) -> SourceLocation {
    let (line, column) = pair.as_span().start_pos().line_col();
    SourceLocation {
        file: source_name.into(),
        line,
        column,
    }
}

fn offset_location(location: &SourceLocation, prefix: &str) -> SourceLocation {
    SourceLocation {
        file: location.file.clone(),
        line: location.line,
        column: location.column + prefix.chars().count(),
    }
}

fn pest_diagnostic(source_name: &str, error: pest::error::Error<Rule>) -> Diagnostic {
    let ((line, column), (end_line, end_column)) = match error.line_col {
        pest::error::LineColLocation::Pos((line, column)) => {
            ((line, column), (line, column.saturating_add(1)))
        }
        pest::error::LineColLocation::Span(start, end) => (start, end),
    };
    Diagnostic::at_span(
        SourceSpan {
            file: source_name.into(),
            start: SourcePosition { line, column },
            end: SourcePosition {
                line: end_line,
                column: end_column,
            },
        },
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> ParsedAssembly {
        parse_assembly_syntax("test.asm", source).unwrap()
    }

    #[test]
    fn semicolon_inside_string_is_data_not_a_comment() {
        let parsed = parse("db \"a;b\", 1 ; real comment\n");
        assert_eq!(parsed.items.len(), 1);
        assert!(matches!(
            &parsed.items[0].kind,
            ParsedAssemblyItem::Data { values, .. }
                if values == &vec![
                    ParsedAssemblyDataValue::StringLiteral("\"a;b\"".to_owned()),
                    ParsedAssemblyDataValue::Expression("1".to_owned()),
                ]
        ));
        let lowered = lower_parsed_assembly(parsed).unwrap();
        assert!(matches!(
            &lowered.items[0].kind,
            AssemblyItem::Data { values, .. }
                if values[0] == AssemblyDataValue::Bytes(b"a;b".to_vec())
        ));
    }

    #[test]
    fn expression_precedence_works_without_whitespace() {
        let lowered = lower_parsed_assembly(parse("answer equ 1+2*3&7|8^9")).unwrap();
        let AssemblyItem::Equ { value, .. } = &lowered.items[0].kind else {
            panic!("expected equate");
        };
        assert!(matches!(
            value,
            AssemblyExpression::Binary {
                operator: AssemblyBinaryOperator::BitOr,
                ..
            }
        ));
    }

    #[test]
    fn nested_macros_and_conditionals_are_structural() {
        let parsed = parse(
            "%macro outer(a)\n%if a\n%macro inner(x)\ndb x\n%endmacro\n%else\ndb 0\n%endif\n%endmacro\n",
        );
        let ParsedAssemblyItem::MacroDefinition { body, .. } = &parsed.items[0].kind else {
            panic!("expected macro");
        };
        let ParsedAssemblyItem::Conditional {
            then_items,
            else_items,
            ..
        } = &body[0].kind
        else {
            panic!("expected conditional");
        };
        assert!(matches!(
            then_items[0].kind,
            ParsedAssemblyItem::MacroDefinition { .. }
        ));
        assert!(matches!(
            else_items[0].kind,
            ParsedAssemblyItem::Data { .. }
        ));
    }

    #[test]
    fn malformed_syntax_has_precise_location() {
        let error = parse_assembly_syntax("bad.asm", "ld a, (1 + 2\n").unwrap_err();
        let location = error.location().unwrap();
        assert_eq!(location.line, 1);
        assert!(location.column >= 7);
    }

    #[test]
    fn label_and_instruction_on_one_line_become_two_items() {
        let parsed = parse("start: ld a, 1\n");
        assert_eq!(parsed.items.len(), 2);
        assert!(matches!(parsed.items[0].kind, ParsedAssemblyItem::Label(_)));
        assert!(matches!(
            parsed.items[1].kind,
            ParsedAssemblyItem::Instruction(_)
        ));
        assert!(parsed.items[1].location.column > parsed.items[0].location.column);
    }

    #[test]
    fn commas_inside_parentheses_and_brackets_do_not_split_operands() {
        let parsed = parse("op (a, b), [x, y], final\n%call pair(1, 2), [3, 4]\n");
        let ParsedAssemblyItem::Instruction(instruction) = &parsed.items[0].kind else {
            panic!("expected instruction");
        };
        assert_eq!(instruction.operands, vec!["(a, b)", "[x, y]", "final"]);
        let ParsedAssemblyItem::MacroInvocation { arguments, .. } = &parsed.items[1].kind else {
            panic!("expected macro invocation");
        };
        assert_eq!(arguments, &vec!["pair(1, 2)", "[3, 4]"]);
    }
}
