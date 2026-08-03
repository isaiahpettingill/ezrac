    use std::path::Path;

    use crate::{ast::BinaryOp, parser::parse_program};

    use super::*;

    fn named(name: &str) -> Type {
        Type::Named(name.to_owned())
    }

    fn ident(name: &str) -> Expr {
        Expr::Ident(name.to_owned())
    }

    fn cast(ty: &str, expr: Expr) -> Expr {
        Expr::Cast {
            ty: named(ty),
            expr: Box::new(expr),
        }
    }

    fn arithmetic(op: BinaryOp) -> Expr {
        cast(
            "u8",
            Expr::Binary {
                left: Box::new(ident("left")),
                op,
                right: Box::new(ident("right")),
            },
        )
    }

    #[test]
    fn demanded_bits_and_bytes_follow_u16_and_u24_narrowing() {
        let u16_demand = unsigned_narrowing_demand(&named("u16"), &named("u8")).unwrap();
        assert_eq!(u16_demand.bits(), 0x00FF);
        assert_eq!(u16_demand.bytes(), 0b001);

        let u24_demand = unsigned_narrowing_demand(&named("u24"), &named("u16")).unwrap();
        assert_eq!(u24_demand.bits(), 0xFFFF);
        assert_eq!(u24_demand.bytes(), 0b011);

        let report = demand_for_expr(&cast("u8", ident("word")), 0xFF);
        assert_eq!(report.demanded().bits(), 0xFF);
        assert_eq!(report.demanded_bytes, 0b001);
        assert_eq!(report.input_demands, vec![u16_demand]);

        let report = demand_for_expr(&cast("u16", ident("wide")), 0xFFFF);
        assert_eq!(report.demanded().bits(), 0xFFFF);
        assert_eq!(report.demanded_bytes, 0b011);
        assert_eq!(report.input_demands, vec![u24_demand]);
    }

    #[test]
    fn add_and_multiply_propagate_low_byte_demand_to_both_inputs() {
        for op in [BinaryOp::Add, BinaryOp::Mul] {
            let report = demand_for_expr(&arithmetic(op), 0xFF);
            assert_eq!(report.demanded_bits, 0xFF);
            assert_eq!(report.demanded_bytes, 0b001);
            assert_eq!(report.input_demands.len(), 2);
            assert!(
                report
                    .input_demands
                    .iter()
                    .all(|demand| demand.bits() == u8::MAX as u32)
            );
            assert!(
                report
                    .input_demands
                    .iter()
                    .all(|demand| demand.bytes() == 0b001)
            );
        }
    }

    #[test]
    fn rewrites_pure_low_byte_add_and_multiply() {
        let mut program = parse_program(
            Path::new("test.ezra"),
            "fn add(left: u16, right: u16) -> u8 { return cast<u8>(left + right) } fn multiply(left: u16, right: u16) -> u8 { return cast<u8>(left * right) }",
        )
        .unwrap();
        apply_program(&mut program);

        for name in ["add", "multiply"] {
            let function = program
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Function(function) if function.name == name => Some(function),
                    _ => None,
                })
                .unwrap();
            let Stmt::Return(Some(Expr::Cast { expr, .. })) = &function.body[0] else {
                panic!("missing narrowing cast in {name}");
            };
            let Expr::Binary { left, right, .. } = expr.as_ref() else {
                panic!("narrowing cast did not expose byte arithmetic");
            };
            assert!(matches!(left.as_ref(), Expr::Cast { ty, .. } if ty == &named("u8")));
            assert!(matches!(right.as_ref(), Expr::Cast { ty, .. } if ty == &named("u8")));
        }
    }

    #[test]
    fn reports_effects_and_leaves_effectful_arithmetic_unchanged() {
        let expression = cast(
            "u8",
            Expr::Binary {
                left: Box::new(Expr::Call {
                    path: vec!["read".to_owned()],
                    args: Vec::new(),
                }),
                op: BinaryOp::Add,
                right: Box::new(Expr::In("STATUS".to_owned())),
            },
        );
        let report = demand_for_expr(&expression, u32::MAX);
        assert!(report.effects.calls);
        assert!(report.effects.port_reads);
        assert!(!report.effects.is_pure());

        let mut program = parse_program(
            Path::new("test.ezra"),
            "fn read() -> u16 { return 1 } port STATUS: u8 = 1 fn test(value: u16) -> u8 { return cast<u8>(read() + value) }",
        )
        .unwrap();
        apply_program(&mut program);
        let function = program
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Function(function) if function.name == "test" => Some(function),
                _ => None,
            })
            .unwrap();
        let Stmt::Return(Some(Expr::Cast { expr, .. })) = &function.body[0] else {
            panic!("missing narrowing cast");
        };
        let Expr::Binary { left, right, .. } = expr.as_ref() else {
            panic!("unexpected effectful expression shape");
        };
        assert!(!matches!(left.as_ref(), Expr::Cast { .. }));
        assert!(!matches!(right.as_ref(), Expr::Cast { .. }));
    }

    #[test]
    fn invalid_typed_literals_are_not_hidden_by_narrowing() {
        let mut program = parse_program(
            Path::new("test.ezra"),
            "fn test(value: u16) -> u8 { return cast<u8>(value + 0x10000u16) }",
        )
        .unwrap();
        apply_program(&mut program);
        let function = program
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Function(function) if function.name == "test" => Some(function),
                _ => None,
            })
            .unwrap();
        assert!(format!("{:?}", function.body).contains("TypedInt(65536, Named(\"u16\"))"));
    }
