use std::collections::HashMap;

use crate::{
    ast::{AssignOp, BinaryOp, Declaration, Expr, Function, Program, Stmt, Type, UnaryOp},
    diagnostic::Diagnostic,
};

const VAR_BASE: u32 = 0x04_0000;

pub fn emit_ez80_assembly(program: &Program) -> Result<String, Diagnostic> {
    let symbols = Symbols::from_program(program)?;
    let main = program
        .main_function()
        .ok_or_else(|| Diagnostic::new("missing required `fn main()`"))?;

    let mut emitter = Emitter {
        symbols,
        out: String::new(),
        label_counter: 0,
        scopes: Vec::new(),
        loop_stack: Vec::new(),
    };
    emitter.emit_prelude();
    emitter.emit_global_initializers(program)?;
    emitter.emit_start_tail();
    emitter.emit_function(main)?;
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        if function.name != "main" {
            emitter.emit_function(function)?;
        }
    }
    Ok(emitter.out)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Variable {
    addr: u32,
    size: u8,
}

struct Symbols {
    constants: HashMap<String, i64>,
    ports: HashMap<String, u8>,
    globals: HashMap<String, Variable>,
    functions: HashMap<String, FunctionSig>,
    next_addr: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FunctionSig {
    arity: usize,
}

impl Symbols {
    fn from_program(program: &Program) -> Result<Self, Diagnostic> {
        let mut symbols = Self {
            constants: sdk_constants(),
            ports: sdk_ports(),
            globals: HashMap::new(),
            functions: HashMap::new(),
            next_addr: VAR_BASE,
        };

        for declaration in &program.declarations {
            if let Declaration::Function(function) = declaration {
                for param in &function.params {
                    type_size(&param.ty)?;
                }
                symbols.functions.insert(
                    function.name.clone(),
                    FunctionSig {
                        arity: function.params.len(),
                    },
                );
            }
        }

        for declaration in &program.declarations {
            match declaration {
                Declaration::Const(decl) => {
                    let value = symbols.eval_i64(&decl.value)?;
                    symbols.constants.insert(decl.name.clone(), value);
                }
                Declaration::Port(decl) => {
                    let value = symbols.eval_i64(&decl.value)?;
                    if !(0..=0xFF).contains(&value) {
                        return Err(Diagnostic::new(format!(
                            "port `{}` value {value} is outside u8 range",
                            decl.name
                        )));
                    }
                    symbols.ports.insert(decl.name.clone(), value as u8);
                }
                Declaration::Global(decl) => {
                    let variable = symbols.alloc_var(type_size(&decl.ty)?);
                    symbols.globals.insert(decl.name.clone(), variable);
                }
                _ => {}
            }
        }

        Ok(symbols)
    }

    fn alloc_var(&mut self, size: u8) -> Variable {
        let variable = Variable {
            addr: self.next_addr,
            size,
        };
        self.next_addr += size as u32;
        variable
    }

    fn eval_i64(&self, expr: &Expr) -> Result<i64, Diagnostic> {
        match expr {
            Expr::Int(value) => Ok(*value),
            Expr::Char(value) => Ok(*value as i64),
            Expr::Bool(value) => Ok(i64::from(*value)),
            Expr::Ident(name) => self
                .constants
                .get(name)
                .copied()
                .ok_or_else(|| Diagnostic::new(format!("unknown constant `{name}`"))),
            Expr::Unary { op, expr } => {
                let value = self.eval_i64(expr)?;
                Ok(match op {
                    UnaryOp::Neg => -value,
                    UnaryOp::BitNot => !value,
                    UnaryOp::Not => i64::from(value == 0),
                })
            }
            Expr::Binary { left, op, right } => {
                let left = self.eval_i64(left)?;
                let right = self.eval_i64(right)?;
                Ok(match op {
                    BinaryOp::Mul => left * right,
                    BinaryOp::Div => checked_div(left, right)?,
                    BinaryOp::Mod => checked_mod(left, right)?,
                    BinaryOp::Add => left + right,
                    BinaryOp::Sub => left - right,
                    BinaryOp::Shl => left << right,
                    BinaryOp::Shr => left >> right,
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
            Expr::Cast { expr, .. } => self.eval_i64(expr),
            Expr::In(_) | Expr::Call { .. } | Expr::String(_) => Err(Diagnostic::new(format!(
                "expression `{expr:?}` is not a compile-time integer"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoopLabels {
    continue_label: String,
    break_label: String,
}

struct Emitter {
    symbols: Symbols,
    out: String,
    label_counter: usize,
    scopes: Vec<HashMap<String, Variable>>,
    loop_stack: Vec<LoopLabels>,
}

impl Emitter {
    fn emit_prelude(&mut self) {
        self.line("; generated by ezrac scaffold");
        self.line("; target: eZ80 ADL mode");
        self.line("section .text");
        self.line("__ezra_start:");
        self.line("    ld sp, 0F00000h");
    }

    fn emit_start_tail(&mut self) {
        self.line("    call _main");
        self.line("__ezra_exit:");
        self.line("    jp __ezra_exit");
        self.line("");
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
            self.emit_expr_to_a(&decl.value)?;
            self.emit_store_a(variable);
        }
        Ok(())
    }

    fn emit_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        self.line(&format!("_{}:", function.name));
        self.scopes.push(HashMap::new());
        self.bind_params(function)?;
        for stmt in &function.body {
            self.emit_stmt(stmt)?;
        }
        self.scopes.pop();
        if function.name == "main" {
            self.line("    jp __ezra_exit");
        } else {
            self.line("    ret");
        }
        Ok(())
    }

    fn bind_params(&mut self, function: &Function) -> Result<(), Diagnostic> {
        if function.params.len() > 3 {
            return Err(Diagnostic::new(format!(
                "function `{}` has {} parameters; current codegen supports at most 3 u8 parameters",
                function.name,
                function.params.len()
            )));
        }

        for (index, param) in function.params.iter().enumerate() {
            let variable = self.symbols.alloc_var(type_size(&param.ty)?);
            self.current_scope_mut()
                .insert(param.name.clone(), variable);
            match index {
                0 => {}
                1 => self.line("    ld a, b"),
                2 => self.line("    ld a, c"),
                _ => unreachable!("param count checked"),
            }
            self.emit_store_a(variable);
        }
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let { name, ty, value } => {
                let variable = self.symbols.alloc_var(type_size(ty)?);
                self.current_scope_mut().insert(name.clone(), variable);
                self.emit_expr_to_a(value)?;
                self.emit_store_a(variable);
            }
            Stmt::Assign { target, op, value } => {
                let variable = self.variable(target)?;
                self.emit_assignment_value(variable, *op, value)?;
                self.emit_store_a(variable);
            }
            Stmt::Out { port, value } => {
                let port = self.port(port)?;
                self.emit_expr_to_a(value)?;
                self.emit_out_a(port);
            }
            Stmt::Expr(Expr::Call { path, args }) => self.emit_call(path, args)?,
            Stmt::Expr(expr) => {
                self.emit_expr_to_a(expr)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let else_label = self.next_label("else");
                let end_label = self.next_label("endif");
                self.emit_expr_to_a(condition)?;
                self.line("    or a");
                self.line(&format!("    jp z, {else_label}"));
                for stmt in then_body {
                    self.emit_stmt(stmt)?;
                }
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{else_label}:"));
                for stmt in else_body {
                    self.emit_stmt(stmt)?;
                }
                self.line(&format!("{end_label}:"));
            }
            Stmt::While { condition, body } => {
                let start_label = self.next_label("while");
                let end_label = self.next_label("endwhile");
                self.loop_stack.push(LoopLabels {
                    continue_label: start_label.clone(),
                    break_label: end_label.clone(),
                });
                self.line(&format!("{start_label}:"));
                self.emit_expr_to_a(condition)?;
                self.line("    or a");
                self.line(&format!("    jp z, {end_label}"));
                for stmt in body {
                    self.emit_stmt(stmt)?;
                }
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
                for stmt in body {
                    self.emit_stmt(stmt)?;
                }
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
            Stmt::Return(None) => self.line("    ret"),
            Stmt::Return(Some(expr)) => {
                self.emit_expr_to_a(expr)?;
                self.line("    ret");
            }
        }
        Ok(())
    }

    fn emit_assignment_value(
        &mut self,
        variable: Variable,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
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
            AssignOp::Shl | AssignOp::Shr => {
                return Err(Diagnostic::new(
                    "shift assignment codegen is not implemented yet",
                ));
            }
        }
        Ok(())
    }

    fn emit_call(&mut self, path: &[String], args: &[Expr]) -> Result<(), Diagnostic> {
        match path_text(path).as_str() {
            "test.pass" | "ezra.test.pass" => {
                self.emit_out(0x0D, 0);
                self.emit_out(0x0E, 1);
            }
            "test.fail" | "ezra.test.fail" => {
                let expr = args.first().cloned().unwrap_or(Expr::Int(1));
                self.emit_expr_to_a(&expr)?;
                self.emit_out_a(0x0D);
                self.emit_out(0x0E, 1);
            }
            "test.assert_eq_u8" | "ezra.test.assert_eq_u8" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u8 requires three arguments",
                    ));
                }
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_a(&args[0])?;
                self.line("    ld b, a");
                self.emit_expr_to_a(&args[1])?;
                self.line("    ld c, a");
                self.line("    ld a, b");
                self.line("    cp c");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_out_a(0x0D);
                self.emit_out(0x0E, 1);
                self.line(&format!("{ok}:"));
            }
            "debug.char" | "ezra.debug.char" => {
                let expr = args
                    .first()
                    .ok_or_else(|| Diagnostic::new("debug.char requires one argument"))?;
                self.emit_expr_to_a(expr)?;
                self.emit_out_a(0x0C);
            }
            path if path.contains('.') => {
                self.line(&format!("    call _{}", path.replace('.', "_")))
            }
            path => self.emit_user_call(path, args)?,
        }
        Ok(())
    }

    fn emit_user_call(&mut self, name: &str, args: &[Expr]) -> Result<(), Diagnostic> {
        let sig = self
            .symbols
            .functions
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if sig.arity != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments but got {}",
                sig.arity,
                args.len()
            )));
        }
        if args.len() > 3 {
            return Err(Diagnostic::new(format!(
                "function `{name}` has {} arguments; current codegen supports at most 3 u8 arguments",
                args.len()
            )));
        }

        let mut temps = Vec::with_capacity(args.len());
        for arg in args {
            let temp = self.symbols.alloc_var(1);
            self.emit_expr_to_a(arg)?;
            self.emit_store_a(temp);
            temps.push(temp);
        }

        if let Some(temp) = temps.get(1).copied() {
            self.emit_load_a(temp);
            self.line("    ld b, a");
        }
        if let Some(temp) = temps.get(2).copied() {
            self.emit_load_a(temp);
            self.line("    ld c, a");
        }
        if let Some(temp) = temps.first().copied() {
            self.emit_load_a(temp);
        }
        self.line(&format!("    call _{name}"));
        Ok(())
    }

    fn emit_expr_to_a(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(variable) = self.variable_opt(name) {
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
            Expr::Int(_)
            | Expr::Char(_)
            | Expr::Bool(_)
            | Expr::Unary { .. }
            | Expr::Cast { .. } => {
                let value = self.u8(expr)?;
                self.line(&format!("    ld a, {:02X}h", value));
            }
            Expr::Binary { left, op, right } => self.emit_binary_expr(left, *op, right)?,
            Expr::Call { path, args }
                if matches!(
                    path_text(path).as_str(),
                    "input.read_pad" | "ezra.input.read_pad"
                ) =>
            {
                let index = args
                    .first()
                    .map(|expr| self.u8(expr))
                    .transpose()?
                    .unwrap_or(0);
                let port = match index {
                    0 => 0x01,
                    1 => 0x03,
                    2 => 0x05,
                    3 => 0x07,
                    _ => {
                        return Err(Diagnostic::new(
                            "input.read_pad index must be 0..3 in current codegen",
                        ));
                    }
                };
                self.line(&format!("    in0 a, ({port:02X}h)"));
            }
            Expr::Call { path, args } if path.len() == 1 => {
                self.emit_user_call(&path[0], args)?;
            }
            Expr::Call { .. } | Expr::String(_) => {
                return Err(Diagnostic::new(format!(
                    "expression `{expr:?}` is not supported in u8 codegen"
                )));
            }
        }
        Ok(())
    }

    fn emit_binary_expr(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        self.emit_expr_to_a(left)?;
        self.line("    ld b, a");
        self.emit_expr_to_a(right)?;
        self.line("    ld c, a");
        self.line("    ld a, b");
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
            BinaryOp::And | BinaryOp::Or => self.emit_logical(op),
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod | BinaryOp::Shl | BinaryOp::Shr => {
                return Err(Diagnostic::new(format!(
                    "binary operator `{op:?}` is not implemented in u8 codegen yet"
                )));
            }
        }
        Ok(())
    }

    fn emit_comparison(&mut self, op: BinaryOp) {
        let true_label = self.next_label("cmp_true");
        let end_label = self.next_label("cmp_end");
        let false_label = self.next_label("cmp_false");
        self.line("    cp c");
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

    fn emit_logical(&mut self, op: BinaryOp) {
        match op {
            BinaryOp::And => {
                let false_label = self.next_label("and_false");
                let end_label = self.next_label("and_end");
                self.line("    or a");
                self.line(&format!("    jp z, {false_label}"));
                self.line("    ld a, c");
                self.line("    or a");
                self.line(&format!("    jp z, {false_label}"));
                self.line("    ld a, 01h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{false_label}:"));
                self.line("    ld a, 00h");
                self.line(&format!("{end_label}:"));
            }
            BinaryOp::Or => {
                let true_label = self.next_label("or_true");
                let end_label = self.next_label("or_end");
                self.line("    or a");
                self.line(&format!("    jp nz, {true_label}"));
                self.line("    ld a, c");
                self.line("    or a");
                self.line(&format!("    jp nz, {true_label}"));
                self.line("    ld a, 00h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{true_label}:"));
                self.line("    ld a, 01h");
                self.line(&format!("{end_label}:"));
            }
            _ => unreachable!("not logical"),
        }
    }

    fn emit_out(&mut self, port: u8, value: u8) {
        self.line(&format!("    ld a, {:02X}h", value));
        self.emit_out_a(port);
    }

    fn emit_out_a(&mut self, port: u8) {
        self.line(&format!("    out0 ({:02X}h), a", port));
    }

    fn emit_load_a(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 1);
        self.line(&format!("    ld a, ({:06X}h)", variable.addr));
    }

    fn emit_store_a(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 1);
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
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

    fn variable_opt(&self, name: &str) -> Option<Variable> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .or_else(|| self.symbols.globals.get(name).copied())
    }

    fn current_scope_mut(&mut self) -> &mut HashMap<String, Variable> {
        self.scopes
            .last_mut()
            .expect("function scope exists during statement emission")
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

fn type_size(ty: &Type) -> Result<u8, Diagnostic> {
    match ty {
        Type::Named(name) if name == "u8" || name == "i8" || name == "bool" => Ok(1),
        Type::Named(name) => Err(Diagnostic::new(format!(
            "type `{name}` is parsed but not implemented in assembly codegen yet"
        ))),
        Type::Ptr(_) | Type::Array { .. } => Err(Diagnostic::new(
            "pointer and array storage codegen is not implemented yet",
        )),
    }
}

fn checked_div(left: i64, right: i64) -> Result<i64, Diagnostic> {
    if right == 0 {
        Err(Diagnostic::new("constant division by zero"))
    } else {
        Ok(left / right)
    }
}

fn checked_mod(left: i64, right: i64) -> Result<i64, Diagnostic> {
    if right == 0 {
        Err(Diagnostic::new("constant modulo by zero"))
    } else {
        Ok(left % right)
    }
}

fn path_text(path: &[String]) -> String {
    path.join(".")
}

fn sdk_constants() -> HashMap<String, i64> {
    HashMap::from([
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
        ("AUDIO_SUBMIT_BUFFER".to_owned(), 1),
        ("AUDIO_STOP".to_owned(), 2),
    ])
}

fn sdk_ports() -> HashMap<String, u8> {
    HashMap::from([
        ("PAD1_LO".to_owned(), 0x01),
        ("PAD1_HI".to_owned(), 0x02),
        ("VIDEO_CMD".to_owned(), 0x09),
        ("AUDIO_CMD".to_owned(), 0x0A),
        ("DEBUG_CHAR".to_owned(), 0x0C),
        ("TEST_RESULT".to_owned(), 0x0D),
        ("TEST_HALT".to_owned(), 0x0E),
    ])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{parser::parse_program, vm::run_assembly_test};

    use super::*;

    #[test]
    fn emits_test_pass_ports() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();

        assert!(asm.contains("out0 (0Dh), a"));
        assert!(asm.contains("out0 (0Eh), a"));
    }

    #[test]
    fn emits_and_runs_u8_loop_with_assertion() {
        let source = r#"
            global total: u8 = 0
            fn main() {
                let i: u8 = 0
                while i < 4 {
                    total += 2
                    i += 1
                }
                test.assert_eq_u8(total, 8, 7)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_user_function_returning_u8() {
        let source = r#"
            fn answer() -> u8 {
                return 42
            }

            fn main() {
                let x: u8 = answer()
                test.assert_eq_u8(x, 42, 9)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_user_function_with_u8_parameters() {
        let source = r#"
            fn inc(v: u8) -> u8 {
                return v + 1
            }

            fn add(a: u8, b: u8) -> u8 {
                return a + b
            }

            fn mix(a: u8, b: u8, c: u8) -> u8 {
                return a + b + c
            }

            fn main() {
                let x: u8 = inc(4)
                let y: u8 = add(x, 6)
                let z: u8 = mix(y, 2, 3)
                test.assert_eq_u8(z, 16, 8)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 2_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn emits_and_runs_direct_port_read() {
        let source = r#"
            port PAD1_LO: u8 = 0x01
            fn main() {
                let pad: u8 = in PAD1_LO
                test.assert_eq_u8(pad, 0, 4)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }
}
