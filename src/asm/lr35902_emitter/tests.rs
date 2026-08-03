    use super::*;
    use crate::{
        asm::AssemblyOptions,
        parser::parse_program,
        target::{Address24, CpuFamily},
        vm::assemble_subset_with_symbols_at,
    };
    use std::path::Path;

    fn emit(source: &str) -> String {
        let program = parse_program(Path::new("lr35902.ezra"), source).unwrap();
        let assembly = emit_lr35902_assembly_with_options(
            &program,
            AssemblyOptions {
                cpu: CpuFamily::Lr35902,
                stack_top: Address24::new(0x0aff),
                ram_base: Address24::new(0x0100),
                rodata_base: Address24::new(0x0800),
                asset_base: Address24::new(0x0900),
                default_sdk_symbols: false,
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
        assemble_subset_with_symbols_at(CpuFamily::Lr35902.into(), &assembly, 0).unwrap_or_else(
            |error| {
                panic!(
                    "{error}
{assembly}"
                )
            },
        );
        assembly
    }

    fn planned_main_locals(source: &str) -> (HashMap<String, Binding>, u32) {
        let program = parse_program(Path::new("lr35902-locals.ezra"), source).unwrap();
        let options = AssemblyOptions {
            cpu: CpuFamily::Lr35902,
            stack_top: Address24::new(0x0aff),
            ram_base: Address24::new(0x0100),
            rodata_base: Address24::new(0x0800),
            asset_base: Address24::new(0x0900),
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        };
        let mut model = SemanticModel::from_program(
            &program,
            16,
            options.ram_base.get(),
            options.rodata_base.get(),
            options.asset_base.get(),
        )
        .unwrap();
        let start = model.next_ram_address();
        let bindings = plan_static_locals(program.main_function().unwrap(), &mut model).unwrap();
        (bindings, model.next_ram_address() - start)
    }

    #[test]
    fn local_target_models_byte_pair_aliases_and_memory_only_source_class() {
        let target = lr35902_local_target();
        assert!(
            target.register_classes[LR35902_MEMORY_LOCAL_CLASS.0]
                .registers
                .is_empty()
        );
        assert!(target.registers_alias(PhysReg(1), PhysReg(7)));
        assert!(target.registers_alias(PhysReg(2), PhysReg(7)));
        assert!(target.registers_alias(PhysReg(3), PhysReg(8)));
        assert!(target.registers_alias(PhysReg(4), PhysReg(8)));
        assert!(target.registers_alias(PhysReg(5), PhysReg(9)));
        assert!(target.registers_alias(PhysReg(6), PhysReg(9)));
        assert!(!target.registers_alias(PhysReg(0), PhysReg(7)));
        assert!(!target.registers_alias(PhysReg(7), PhysReg(8)));
        assert_eq!(
            target.spill_classes[LR35902_STATIC_SPILL_CLASS.0].base_alignment,
            1
        );
    }

    #[test]
    fn static_local_plan_reuses_only_nonoverlapping_storage() {
        let (reused, reused_bytes) = planned_main_locals(
            "global result: u8 = 0; fn main() { let first: u8 = 1; result = first; let second: u8 = 2; result = second }",
        );
        assert_eq!(
            reused["first"].storage.address,
            reused["second"].storage.address
        );
        assert_eq!(reused_bytes, 1);

        let source = "global result: u8 = 0; fn main() { let first: u8 = 1; let second: u8 = 2; result = first + second }";
        let (overlapping, overlapping_bytes) = planned_main_locals(source);
        assert_ne!(
            overlapping["first"].storage.address,
            overlapping["second"].storage.address
        );
        assert_eq!(overlapping_bytes, 2);
        emit(source);
    }

    #[test]
    fn scalars_operators_control_flow_calls_and_recursion_assemble() {
        let assembly = emit(
            r#"
            global result: u24 = 0
            fn fib(n: u8) -> u16 { if n <= 1 { return cast<u16>(n) } return cast<u16>(fib(n - 1)) + cast<u16>(fib(n - 2)) }
            fn main() {
                let a: u8 = 3; let b: i16 = -2; let c: u24 = cast<u24>(a) + 0x10000
                if a != 0 && b < 0 { result = c }
                while a > 9 { continue }
                loop { result += cast<u24>(fib(a)); break }
            }
        "#,
        );
        assert!(assembly.contains("call _fib"));
        assert!(!assembly.contains("6502"));
    }

    #[test]
    fn lowers_one_bit_mask_branches_to_bit() {
        let assembly = emit(
            r#"
                fn bit_tests(byte: u8) {
                    if (byte & 0x20u8) == 0 {
                        let clear: u8 = byte
                    }
                    if (byte & 0x20u8) != 0 {
                        let set: u8 = byte
                    }
                    while (byte & 0x20u8) == 0 { break }
                    while (byte & 0x20u8) != 0 { break }
                }
                fn wide_bit_tests(word: u16, wide: u24) {
                    if (word & 0x0400u16) == 0 { }
                    if (word & 0x0400u16) != 0 { }
                    if (wide & 0x100000u24) == 0 { }
                    if (wide & 0x100000u24) != 0 { }
                }
                fn main() {
                    bit_tests(0x20u8)
                    wide_bit_tests(0x0400u16, 0x100000u24)
                }
            "#,
        );

        assert_eq!(assembly.matches("    bit 5, a").count(), 4, "{assembly}");
        assert_eq!(assembly.matches("    bit 2, a").count(), 2, "{assembly}");
        assert_eq!(assembly.matches("    bit 4, a").count(), 2, "{assembly}");
        assert!(!assembly.contains("    and b"), "{assembly}");
        for branch in [
            "    bit 5, a\n    jp nz, .L_if_else_",
            "    bit 5, a\n    jp z, .L_if_else_",
            "    bit 5, a\n    jp nz, .L_while_end_",
            "    bit 5, a\n    jp z, .L_while_end_",
        ] {
            assert!(assembly.contains(branch), "missing {branch}:\n{assembly}");
        }
    }

    #[test]
    fn constant_shifts_use_fixed_lr35902_instructions() {
        let assembly = emit(
            r#"
                global value: u24 = 0x123456
                global left: u24 = 0
                global right: u24 = 0
                global signed: i16 = -2
                global sign_fill: i16 = 0

                fn main() {
                    left = value << 12
                    right = value >> 16
                    sign_fill = signed >> 16
                }
            "#,
        );

        assert!(!assembly.contains("shift_loop"), "{assembly}");
        assert!(!assembly.contains("shift_done"), "{assembly}");
        assert!(assembly.matches("    rl a").count() >= 3, "{assembly}");
        assert!(assembly.contains("    xor b"), "{assembly}");
    }

    #[test]
    fn four_byte_scratch_slots_do_not_overlap_u32_i32_operations() {
        let program = parse_program(Path::new("lr35902-scratch.ezra"), "fn main() {}").unwrap();
        let options = AssemblyOptions {
            cpu: CpuFamily::Lr35902,
            stack_top: Address24::new(0x0aff),
            ram_base: Address24::new(0x0100),
            rodata_base: Address24::new(0x0800),
            asset_base: Address24::new(0x0900),
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        };
        let hir = HirProgram::from_ast(&program).unwrap();
        let tbir = TbirProgram::lower(&hir, &program, &options).unwrap();
        let model = SemanticModel::from_program(
            &tbir.lowered_program,
            16,
            options.ram_base.get(),
            options.rodata_base.get(),
            options.asset_base.get(),
        )
        .unwrap();
        let mut emitter = Emitter::new(model, None, BankedLayout::default());

        assert_eq!(emitter.r0.size, 4);
        assert_eq!(emitter.r1.size, 4);
        assert_eq!(emitter.r2.size, 4);
        assert!(emitter.r0.address + emitter.r0.size <= emitter.r1.address);
        assert!(emitter.r1.address + emitter.r1.size <= emitter.r2.address);

        emitter.add(4);
        emitter.sub(4);
        for offset in 0..4 {
            let r0_byte = format!("({:04X}h)", emitter.r0.address + offset);
            let r1_byte = format!("({:04X}h)", emitter.r1.address + offset);
            assert!(
                emitter.out.contains(&r0_byte),
                "missing u32/i32 lhs byte {offset}:\n{}",
                emitter.out
            );
            assert!(
                emitter.out.contains(&r1_byte),
                "missing u32/i32 rhs byte {offset}:\n{}",
                emitter.out
            );
        }
    }

    #[test]
    fn small_constant_multiplication_uses_shift_add_sequences() {
        let assembly = emit(
            r#"
                global value: u16 = 0x1234
                global times_three: u16 = 0
                global times_five: u16 = 0
                global times_six: u16 = 0
                global times_seven: u16 = 0
                global times_nine: u16 = 0
                global times_ten: u16 = 0
                global power_of_two: u16 = 0
                global signed: i16 = -1234
                global negative: i16 = 0

                fn main() {
                    times_three = value * 3
                    times_five = value * 5
                    times_six = value * 6
                    times_seven = value * 7
                    times_nine = value * 9
                    times_ten = value * 10
                    power_of_two = value * 8
                    negative = signed * -7
                }
            "#,
        );

        assert!(!assembly.contains("mul_loop"), "{assembly}");
        assert!(!assembly.contains("mul_done"), "{assembly}");
        assert!(assembly.contains("    adc "), "{assembly}");
        assert!(assembly.contains("    sbc "), "{assembly}");
        assert!(assembly.contains("    rl a"), "{assembly}");
        assert!(assembly.contains("    xor b"), "{assembly}");
    }

    #[test]
    fn constant_compound_multiplication_uses_shift_add_sequence() {
        let assembly = emit(
            r#"
                fn main() {
                    let value: u24 = 0x123456
                    value *= 3
                }
            "#,
        );

        assert!(!assembly.contains("mul_loop"), "{assembly}");
        assert!(assembly.contains("    adc "), "{assembly}");
        assert!(assembly.contains("    rl a"), "{assembly}");
    }

    #[test]
    fn aggregates_pointers_strings_and_inline_asm_assemble() {
        let assembly = emit(
            r#"
            struct Pair { lo: u8 hi: u16 }
            global pair: Pair = Pair { lo: 1, hi: 2 }
            fn main() {
                let xs: [u8; 3] = [1, 2, 3]; let copy: [u8; 3] = xs
                let p: ptr<u8> = &xs[0]; *p = xs[1]
                let q: Pair = Pair { lo: 4, hi: 5 }; pair = q
                let text: ptr<u8> = "lr"
                asm volatile(clobber memory) { "nop" }
            }
        "#,
        );
        assert!(assembly.contains("ld a, (hl)") || assembly.contains("ld (hl), a"));
    }

    #[test]
    fn variable_indices_use_direct_scaled_pointer_arithmetic() {
        let assembly = emit(
            r#"
            global bytes: [u8; 4] = [1, 2, 3, 4]
            global words: [u16; 4] = [1, 2, 3, 4]
            global triples: [u24; 4] = [1, 2, 3, 4]
            global byte_result: u8 = 0
            global word_result: u16 = 0
            global triple_result: u24 = 0
            fn read(index: u16) {
                byte_result = bytes[index]
                word_result = words[index]
                triple_result = triples[index]
            }
            fn main() { read(1) }
        "#,
        );

        assert!(!assembly.contains("index_scale"), "{assembly}");
        assert!(!assembly.contains("index_done"), "{assembly}");
        assert_eq!(assembly.matches("    add hl, bc").count(), 3, "{assembly}");
        assert_eq!(assembly.matches("    add hl, hl").count(), 2, "{assembly}");
        assert_eq!(assembly.matches("    add hl, de").count(), 1, "{assembly}");
    }

    #[test]
    fn volatile_index_expression_is_evaluated_once_before_pointer_math() {
        let assembly = emit(
            r#"
            global words: [u16; 4] = [1, 2, 3, 4]
            global result: u16 = 0
            fn volatile_index() -> u16 {
                asm volatile(clobber memory) { "nop" }
                return 1
            }
            fn read() { result = words[volatile_index()] }
            fn main() { read() }
        "#,
        );

        assert_eq!(
            assembly.matches("    call _volatile_index").count(),
            1,
            "{assembly}"
        );
        assert!(assembly.contains("    add hl, hl"), "{assembly}");
        assert!(assembly.contains("    add hl, bc"), "{assembly}");
        assert!(
            assembly.matches("    ld a, (hl)").count() >= 2,
            "{assembly}"
        );
        assert!(!assembly.contains("index_scale"), "{assembly}");
    }

    #[test]
    fn banked_array_variable_index_uses_banked_base_pointer() {
        let program = parse_program(
            Path::new("banked-variable-index.ezra"),
            r#"
            @cfg(bank(2)) global words: [u16; 4] = [1, 2, 3, 4]
            global result: u16 = 0
            fn read(index: u16) { result = words[index] }
            fn main() {
                asm volatile { "ld a, 2" "call __ezra_gb_select_bank" }
                read(1)
            }
        "#,
        )
        .unwrap();
        let assembly = emit_lr35902_assembly_with_options(
            &program,
            AssemblyOptions {
                cpu: CpuFamily::Lr35902,
                ram_base: Address24::new(0xC000),
                rodata_base: Address24::new(0xD000),
                asset_base: Address24::new(0xE000),
                gameboy_banking: Some(GameBoyBankingOptions {
                    mapper: GameBoyBankingMapper::Mbc1,
                }),
                ..AssemblyOptions::default()
            },
        )
        .unwrap();

        assert!(assembly.contains("__ezra_banked_data_words:"), "{assembly}");
        assert!(assembly.contains("    ld a, 40h"), "{assembly}");
        assert!(assembly.contains("    add hl, hl"), "{assembly}");
        assert!(assembly.contains("    add hl, bc"), "{assembly}");
        assert!(!assembly.contains("index_scale"), "{assembly}");
        assemble_subset_with_symbols_at(CpuFamily::Lr35902.into(), &assembly, 0x0150).unwrap();
    }

    #[test]
    fn banked_functions_emit_resident_far_call_trampolines() {
        let program = parse_program(
            Path::new("banked-function.ezra"),
            r#"
                @cfg(bank(2))
                fn worker() -> u8 { return 42 }
                fn main() { let value: u8 = worker() }
            "#,
        )
        .unwrap();
        let assembly = emit_lr35902_assembly_with_options(
            &program,
            AssemblyOptions {
                cpu: CpuFamily::Lr35902,
                ram_base: Address24::new(0xC000),
                rodata_base: Address24::new(0xD000),
                asset_base: Address24::new(0xE000),
                gameboy_banking: Some(GameBoyBankingOptions {
                    mapper: GameBoyBankingMapper::Mbc1,
                }),
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
        assert!(assembly.contains("__ezra_gb_far_call:"), "{assembly}");
        assert!(assembly.contains("call __ezra_gb_far_call"), "{assembly}");
        assert!(assembly.contains("__ezra_bank_2_start:"), "{assembly}");
        assemble_subset_with_symbols_at(CpuFamily::Lr35902.into(), &assembly, 0x0150).unwrap();
    }

    #[test]
    fn banked_pointer_uses_manual_mapping_from_bank_zero_and_rejects_mbc1_holes() {
        let options = AssemblyOptions {
            cpu: CpuFamily::Lr35902,
            ram_base: Address24::new(0xC000),
            rodata_base: Address24::new(0xD000),
            asset_base: Address24::new(0xE000),
            gameboy_banking: Some(GameBoyBankingOptions {
                mapper: GameBoyBankingMapper::Mbc1,
            }),
            ..AssemblyOptions::default()
        };
        let accepted = parse_program(
            Path::new("manual-banked-pointer.ezra"),
            r#"
            @cfg(bank(2)) embed value: bytes = bytes [0x42]
            fn main() {
                asm volatile { "ld a, 2" "call __ezra_gb_select_bank" }
                let first: u8 = *(value.ptr@2)
            }
        "#,
        )
        .unwrap();
        emit_lr35902_assembly_with_options(&accepted, options.clone()).unwrap();

        let rejected = parse_program(
            Path::new("invalid-banked-pointer.ezra"),
            r#"
            @cfg(bank(2)) embed value: bytes = bytes [0x42]
            fn main() { let first: u8 = *(value.ptr@32) }
        "#,
        )
        .unwrap();
        let error = emit_lr35902_assembly_with_options(&rejected, options).unwrap_err();
        assert!(error.message.contains("not selectable"), "{error}");
    }

    #[test]
    fn mbc5_far_calls_preserve_the_ninth_bank_bit() {
        let program = parse_program(
            Path::new("mbc5-banked-function.ezra"),
            r#"
                @cfg(bank(257))
                fn worker() {}
                fn main() { worker() }
            "#,
        )
        .unwrap();
        let assembly = emit_lr35902_assembly_with_options(
            &program,
            AssemblyOptions {
                cpu: CpuFamily::Lr35902,
                ram_base: Address24::new(0xC000),
                rodata_base: Address24::new(0xD000),
                asset_base: Address24::new(0xE000),
                gameboy_banking: Some(GameBoyBankingOptions {
                    mapper: GameBoyBankingMapper::Mbc5,
                }),
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
        assert!(assembly.contains("ld a, 01h\n    ld b, 01h"), "{assembly}");
        assert!(assembly.contains("ld (3000h), a"), "{assembly}");
        assemble_subset_with_symbols_at(CpuFamily::Lr35902.into(), &assembly, 0x0150).unwrap();
    }

    #[test]
    fn explicit_inline_calls_preserve_argument_and_unsafe_call_semantics() {
        let assembly = emit(
            r#"
            fn first() -> u8 { return 1 }
            fn second() -> u16 { return 2 }
            @inline fn bump(value: u16) -> u16 { return value + 1 }
            @inline fn pair(left: u8, right: u16) -> u16 {
                return bump(cast<u16>(left) + right)
            }
            @inline fn yes() -> bool { return true }
            fn main(flag: bool) {
                let value: u16 = pair(first(), second())
                let short: bool = flag && yes()
                while yes() { return }
            }
        "#,
        );

        assert_eq!(assembly.matches("call _first").count(), 1, "{assembly}");
        assert_eq!(assembly.matches("call _second").count(), 1, "{assembly}");
        assert!(
            assembly.find("call _first").unwrap() < assembly.find("call _second").unwrap(),
            "{assembly}"
        );
        assert!(!assembly.contains("call _pair"), "{assembly}");
        assert!(!assembly.contains("call _bump"), "{assembly}");
        assert_eq!(assembly.matches("call _yes").count(), 2, "{assembly}");
        assert!(assembly.contains("_yes:"), "{assembly}");
    }

    #[test]
    fn interrupt_and_naked_functions_assemble() {
        let assembly = emit(
            r#"
            interrupt fn irq() { asm volatile(clobber memory) { "nop" } }
            naked fn reset() { asm volatile(clobber memory) { "jp _reset" } }
            fn main() {}
        "#,
        );
        assert!(assembly.contains("reti"));
        assert!(assembly.contains("_reset:"));
    }
