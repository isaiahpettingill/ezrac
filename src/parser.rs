use std::path::Path;

use pest::{Parser, iterators::Pair};
use pest_derive::Parser;

use crate::{
    ast::{
        AliasDecl, AssignOp, BinaryOp, ConstDecl, Declaration, Expr, Function, GlobalDecl,
        MmioDecl, Param, Place, PortDecl, Program, Stmt, Type, UnaryOp,
    },
    diagnostic::{Diagnostic, SourceLocation},
};

#[derive(Parser)]
#[grammar = "ezra.pest"]
struct EzraParser;

pub fn parse_program(file: &Path, source: &str) -> Result<Program, Diagnostic> {
    let mut pairs =
        EzraParser::parse(Rule::program, source).map_err(|error| pest_error(file, error))?;
    let program = pairs
        .next()
        .ok_or_else(|| Diagnostic::new("parser produced no program"))?;
    let declarations = program
        .into_inner()
        .filter(|pair| pair.as_rule() != Rule::EOI)
        .map(build_decl)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Program { declarations })
}

fn build_decl(pair: Pair<'_, Rule>) -> Result<Declaration, Diagnostic> {
    match pair.as_rule() {
        Rule::import_decl => Ok(Declaration::Import(
            pair.into_inner().next().unwrap().as_str().to_owned(),
        )),
        Rule::const_decl => build_const(pair).map(Declaration::Const),
        Rule::alias_decl => build_alias(pair).map(Declaration::Alias),
        Rule::port_decl => build_port(pair).map(Declaration::Port),
        Rule::mmio_decl => build_mmio(pair).map(Declaration::Mmio),
        Rule::global_decl => build_global(pair).map(Declaration::Global),
        Rule::fn_decl => build_fn(pair).map(Declaration::Function),
        _ => unreachable!("unexpected decl rule {:?}", pair.as_rule()),
    }
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
    let mut value = None;
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::visibility => public = true,
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::expr => value = Some(build_expr(inner)?),
            _ => {}
        }
    }
    Ok(PortDecl {
        public,
        name: name.unwrap(),
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

fn build_fn(pair: Pair<'_, Rule>) -> Result<Function, Diagnostic> {
    let mut public = false;
    let mut attrs = Vec::new();
    let mut name = None;
    let mut params = Vec::new();
    let mut return_type = None;
    let mut body = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::attr => attrs.push(inner.as_str().to_owned()),
            Rule::visibility => public = true,
            Rule::ident => name = Some(inner.as_str().to_owned()),
            Rule::params => params = build_params(inner)?,
            Rule::ret_ty => return_type = Some(build_type(inner.into_inner().next().unwrap())?),
            Rule::block => body = Some(build_block(inner)?),
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

fn build_block(pair: Pair<'_, Rule>) -> Result<Vec<Stmt>, Diagnostic> {
    pair.into_inner().map(build_stmt).collect()
}

fn build_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Diagnostic> {
    match pair.as_rule() {
        Rule::let_stmt => {
            let mut inner = pair.into_inner();
            Ok(Stmt::Let {
                name: inner.next().unwrap().as_str().to_owned(),
                ty: build_type(inner.next().unwrap())?,
                value: build_expr(inner.next().unwrap())?,
            })
        }
        Rule::assign_stmt => {
            let mut inner = pair.into_inner();
            Ok(Stmt::Assign {
                target: build_place(inner.next().unwrap())?,
                op: build_assign_op(inner.next().unwrap().as_str()),
                value: build_expr(inner.next().unwrap())?,
            })
        }
        Rule::if_stmt => {
            let mut inner = pair.into_inner();
            let condition = build_expr(inner.next().unwrap())?;
            let then_body = build_block(inner.next().unwrap())?;
            let else_body = match inner.next() {
                Some(block) => build_block(block)?,
                None => Vec::new(),
            };
            Ok(Stmt::If {
                condition,
                then_body,
                else_body,
            })
        }
        Rule::while_stmt => {
            let mut inner = pair.into_inner();
            Ok(Stmt::While {
                condition: build_expr(inner.next().unwrap())?,
                body: build_block(inner.next().unwrap())?,
            })
        }
        Rule::loop_stmt => Ok(Stmt::Loop {
            body: build_block(pair.into_inner().next().unwrap())?,
        }),
        Rule::break_stmt => Ok(Stmt::Break),
        Rule::continue_stmt => Ok(Stmt::Continue),
        Rule::return_stmt => Ok(Stmt::Return(
            pair.into_inner().next().map(build_expr).transpose()?,
        )),
        Rule::out_stmt => {
            let mut inner = pair.into_inner();
            Ok(Stmt::Out {
                port: inner.next().unwrap().as_str().to_owned(),
                value: build_expr(inner.next().unwrap())?,
            })
        }
        Rule::expr_stmt => Ok(Stmt::Expr(build_expr(pair.into_inner().next().unwrap())?)),
        _ => unreachable!("unexpected stmt rule {:?}", pair.as_rule()),
    }
}

fn build_place(pair: Pair<'_, Rule>) -> Result<Place, Diagnostic> {
    let inner = pair.into_inner().next().unwrap();
    match inner.as_rule() {
        Rule::ident => Ok(Place::Ident(inner.as_str().to_owned())),
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

fn build_assign_op(op: &str) -> AssignOp {
    match op {
        "=" => AssignOp::Set,
        "+=" => AssignOp::Add,
        "-=" => AssignOp::Sub,
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
                len: parts.next().unwrap().as_str().to_owned(),
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
        Rule::in_expr => Ok(Expr::In(
            pair.into_inner().next().unwrap().as_str().to_owned(),
        )),
        Rule::addr_index_expr => {
            let mut inner = pair.into_inner();
            Ok(Expr::AddressOfIndex {
                name: inner.next().unwrap().as_str().to_owned(),
                index: Box::new(build_expr(inner.next().unwrap())?),
            })
        }
        Rule::index_expr => {
            let mut inner = pair.into_inner();
            Ok(Expr::Index {
                name: inner.next().unwrap().as_str().to_owned(),
                index: Box::new(build_expr(inner.next().unwrap())?),
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
        Rule::call_expr => {
            let mut inner = pair.into_inner();
            let path = split_path(inner.next().unwrap().as_str());
            let args = inner
                .next()
                .map(|args| args.into_inner().map(build_expr).collect())
                .unwrap_or_else(|| Ok(Vec::new()))?;
            Ok(Expr::Call { path, args })
        }
        Rule::path_expr => Ok(Expr::Ident(pair.as_str().to_owned())),
        Rule::literal => build_expr(pair.into_inner().next().unwrap()),
        Rule::bool_lit => Ok(Expr::Bool(pair.as_str() == "true")),
        Rule::int_lit => Ok(Expr::Int(parse_int(pair.as_str())?)),
        Rule::char_lit => Ok(Expr::Char(parse_char(pair.as_str())?)),
        Rule::string_lit => Ok(Expr::String(parse_string(pair.as_str())?)),
        _ => unreachable!("unexpected expr rule {:?}", pair.as_rule()),
    }
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
            op: build_binary_op(op.as_str()),
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

fn parse_int(text: &str) -> Result<i64, Diagnostic> {
    let digits = text
        .trim_end_matches("u8")
        .trim_end_matches("i8")
        .trim_end_matches("u16")
        .trim_end_matches("i16")
        .trim_end_matches("u24")
        .trim_end_matches("i24");
    if let Some(hex) = digits.strip_prefix("0x") {
        i64::from_str_radix(hex, 16)
    } else if let Some(bin) = digits.strip_prefix("0b") {
        i64::from_str_radix(bin, 2)
    } else {
        digits.parse()
    }
    .map_err(|_| Diagnostic::new(format!("invalid integer literal `{text}`")))
}

fn parse_char(text: &str) -> Result<u8, Diagnostic> {
    let body = &text[1..text.len() - 1];
    let value = parse_escaped(body)?;
    Ok(value.into_bytes().first().copied().unwrap_or(0))
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

fn pest_error(file: &Path, error: pest::error::Error<Rule>) -> Diagnostic {
    let (line, column) = match error.line_col {
        pest::error::LineColLocation::Pos((line, column)) => (line, column),
        pest::error::LineColLocation::Span((line, column), _) => (line, column),
    };
    Diagnostic::at(
        SourceLocation {
            file: file.to_path_buf(),
            line,
            column,
        },
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_main_with_out() {
        let program = parse_program(
            Path::new("game.ezra"),
            "port DEBUG_CHAR: u8 = 0x0C\nfn main() { out DEBUG_CHAR, 'A' }",
        )
        .unwrap();

        assert!(program.main_function().is_some());
        assert_eq!(program.declarations.len(), 2);
    }

    #[test]
    fn parses_in_port_expression() {
        let program = parse_program(
            Path::new("game.ezra"),
            "port PAD1_LO: u8 = 0x01\nfn main() { let pad: u8 = in PAD1_LO }",
        )
        .unwrap();

        assert!(program.main_function().is_some());
    }

    #[test]
    fn parses_volatile_mmio_declaration() {
        let program = parse_program(
            Path::new("game.ezra"),
            "volatile mmio FRAMEBUFFER: ptr<u8> = 0x080000\nfn main() {}",
        )
        .unwrap();

        assert!(matches!(program.declarations[0], Declaration::Mmio(_)));
    }

    #[test]
    fn parses_type_alias_declaration() {
        let program = parse_program(
            Path::new("game.ezra"),
            "pub alias subpx = i24\nfn main() { let x: subpx = 0 }",
        )
        .unwrap();

        assert!(matches!(program.declarations[0], Declaration::Alias(_)));
    }

    #[test]
    fn parses_array_literal_index_and_address_of_index() {
        let program = parse_program(
            Path::new("game.ezra"),
            "global palette: [u8; 4] = [1, 2]\nfn main() { palette[1] = 3\nlet p: ptr<u8> = &palette[0] }",
        )
        .unwrap();

        assert!(program.main_function().is_some());
    }
}
