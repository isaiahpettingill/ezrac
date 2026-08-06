use super::*;
use crate::{
    asm::AssemblyOptions,
    parser::parse_program,
    target::{Address24, CpuFamily},
    vm::assemble_subset_with_symbols_at,
};
use std::path::Path;

fn emit(source: &str) -> String {
    emit_with_arduboy_vectors(source, false)
}

fn emit_with_arduboy_vectors(source: &str, arduboy_executable: bool) -> String {
    let program = parse_program(Path::new("avr.ezra"), source).unwrap();
    let assembly = emit_avr_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::Avr,
            stack_top: Address24::new(0x0aff),
            ram_base: Address24::new(0x0104),
            rodata_base: Address24::new(0x0800),
            asset_base: Address24::new(0x0900),
            default_sdk_symbols: false,
            arduboy_executable,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    assemble_subset_with_symbols_at(CpuFamily::Avr.into(), &assembly, 0).unwrap_or_else(|error| {
        panic!(
            "{error}
{assembly}"
        )
    });
    assembly
}

fn plan(source: &str, function_name: &str) -> FunctionLocals {
    let program = parse_program(Path::new("avr.ezra"), source).unwrap();
    let options = AssemblyOptions {
        cpu: CpuFamily::Avr,
        ram_base: Address24::new(0x0104),
        rodata_base: Address24::new(0x0800),
        asset_base: Address24::new(0x0900),
        default_sdk_symbols: false,
        ..AssemblyOptions::default()
    };
    let hir = HirProgram::from_ast(&program).unwrap();
    let tbir = TbirProgram::lower(&hir, &program, &options).unwrap();
    let mut model = SemanticModel::from_program(
        &tbir.lowered_program,
        16,
        options.ram_base.get(),
        options.rodata_base.get(),
        options.asset_base.get(),
    )
    .unwrap();
    let function = tbir
        .lowered_program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.name == function_name => Some(function),
            _ => None,
        })
        .unwrap();
    plan_function_locals(function, &mut model).unwrap()
}

fn storage_address(locals: &FunctionLocals, name: &str) -> u32 {
    match &locals.bindings[name].location {
        BindingLocation::Storage(storage) => storage.address,
        BindingLocation::Register(_) => panic!("`{name}` should have a static home"),
    }
}

#[test]
fn local_target_has_legal_consecutive_aliasing_groups() {
    let target = avr_local_target();
    let diagnostics = crate::regalloc::validate(
        &target,
        &crate::regalloc::Function::new(vec![], vec![crate::regalloc::BasicBlock::default()]),
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(target.units.len(), 14);
    assert_eq!(target.register_classes.len(), 4);
    assert_eq!(target.register_classes[0].registers.len(), 14);
    assert_eq!(target.register_classes[1].registers.len(), 7);
    assert_eq!(target.register_classes[2].registers.len(), 4);
    assert_eq!(target.register_classes[3].registers.len(), 3);
    let byte_r2 = target.register_classes[0].registers[0];
    let pair_r2_r3 = target.register_classes[1].registers[0];
    let pair_r4_r5 = target.register_classes[1].registers[1];
    assert!(target.registers_alias(byte_r2, pair_r2_r3));
    assert!(!target.registers_alias(byte_r2, pair_r4_r5));
}

#[test]
fn scalar_locals_use_low_register_movs_without_local_static_traffic() {
    let source = r#"
            global input8: u8 = 1
            global input16: u16 = 0x0203
            global input24: u24 = 0x040506
            global input32: u32 = 0x0708090A
            global sink: u32 = 0
            fn byte_local() { let byte: u8 = input8; sink = cast<u32>(byte) }
            fn word_local() { let word: u16 = input16; sink = cast<u32>(word) }
            fn triple_local() { let triple: u24 = input24; sink = cast<u32>(triple) }
            fn long_local() { let long: u32 = input32; sink = long }
            fn main() { byte_local(); word_local(); triple_local(); long_local() }
        "#;
    for (function, name) in [
        ("byte_local", "byte"),
        ("word_local", "word"),
        ("triple_local", "triple"),
        ("long_local", "long"),
    ] {
        let locals = plan(source, function);
        assert!(
            matches!(
                &locals.bindings[name].location,
                BindingLocation::Register(_)
            ),
            "{name} did not get a register home"
        );
    }
    let assembly = emit(source);
    assert!(assembly.contains("    mov r2, r16"), "{assembly}");
    assert!(assembly.contains("    mov r16, r2"), "{assembly}");
    for register in AVR_LOCAL_FIRST_REGISTER..=AVR_LOCAL_LAST_REGISTER {
        assert!(
            !assembly.contains(&format!("lds r{register},")),
            "{assembly}"
        );
        assert!(
            !assembly.contains(&format!("sts r{register},")),
            "{assembly}"
        );
    }
}

#[test]
fn pressure_calls_asm_addresses_and_aggregates_spill_to_reused_static_storage() {
    let pressure = plan(
        r#"
                global input: u16 = 1
                global sink: u16 = 0
                fn main() {
                    let a: u16 = input; let b: u16 = input; let c: u16 = input; let d: u16 = input
                    let e: u16 = input; let f: u16 = input; let g: u16 = input; let h: u16 = input
                    sink = a + b + c + d + e + f + g + h
                }
            "#,
        "main",
    );
    assert!(
        pressure
            .bindings
            .values()
            .any(|binding| matches!(&binding.location, BindingLocation::Storage(_)))
    );

    let call_live = plan(
        r#"
                global input: u8 = 1
                global sink: u8 = 0
                fn value() -> u8 { return 2 }
                fn main() { let live: u8 = input; let result: u8 = value(); sink = live + result }
            "#,
        "main",
    );
    assert!(matches!(
        &call_live.bindings["live"].location,
        BindingLocation::Storage(_)
    ));

    let asm_live = plan(
        r#"
                global input: u8 = 1
                global sink: u8 = 0
                fn main() {
                    let live: u8 = input
                    asm volatile(clobber memory) { "nop" }
                    sink = live
                }
            "#,
        "main",
    );
    assert!(matches!(
        &asm_live.bindings["live"].location,
        BindingLocation::Storage(_)
    ));

    let forced = plan(
        r#"
                global sink: u8 = 0
                fn main() {
                    let addressed: u8 = 1
                    let aggregate: [u8; 2] = [2, 3]
                    let p: ptr<u8> = &addressed
                    asm volatile(in addressed: u8 as mem, clobber memory) { "lds r16, {addressed}" }
                    sink = *p + aggregate[0]
                }
            "#,
        "main",
    );
    assert!(matches!(
        &forced.bindings["addressed"].location,
        BindingLocation::Storage(_)
    ));
    assert!(matches!(
        &forced.bindings["aggregate"].location,
        BindingLocation::Storage(_)
    ));
    let _ = emit(
        r#"
                global sink: u8 = 0
                fn main() {
                    let addressed: u8 = 1
                    let aggregate: [u8; 2] = [2, 3]
                    let p: ptr<u8> = &addressed
                    asm volatile(in addressed: u8 as mem, clobber memory) { "lds r16, {addressed}" }
                    sink = *p + aggregate[0]
                }
            "#,
    );

    let reused = plan(
        r#"
                global sink: u8 = 0
                fn main() {
                    let first: u8 = 1; let first_ptr: ptr<u8> = &first; sink = *first_ptr
                    let second: u8 = 2; let second_ptr: ptr<u8> = &second; sink = *second_ptr
                }
            "#,
        "main",
    );
    assert_ne!(
        storage_address(&reused, "first"),
        storage_address(&reused, "second"),
        "address-taken locals keep dedicated spill slots"
    );
}

#[test]
fn generated_code_uses_low_registers_only_for_allocated_locals() {
    let assembly = emit(
        r#"
                global left: u32 = 0x12345678
                global right: u32 = 0x01020304
                global sink: u32 = 0
                fn main() { sink = left * right + (left >> 3) }
            "#,
    );
    for register in AVR_LOCAL_FIRST_REGISTER..=AVR_LOCAL_LAST_REGISTER {
        let name = format!("r{register}");
        assert!(
            !assembly.lines().any(|line| {
                line.split(|ch: char| !ch.is_ascii_alphanumeric())
                    .any(|word| word == name)
            }),
            "ordinary generated code used r{register}:\n{assembly}"
        );
    }
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
fn lowers_global_function_pointer_calls_with_the_avr_abi() {
    let assembly = emit(
        r#"
            global callback: ptr<fn(u8, u8)u8> = &add
            global answer: u8 = 0
            fn add(left: u8, right: u8) -> u8 { return left + right }
            fn main() { answer = callback(20, 22) }
        "#,
    );

    assert!(assembly.contains("icall"), "{assembly}");
    assert!(assembly.contains("jmp _add"), "{assembly}");
    assert!(assembly.contains("_add:"), "{assembly}");
    assert!(assembly.contains("pop r30"), "{assembly}");
    assert!(assembly.contains("pop r31"), "{assembly}");
}

#[test]
fn lowers_local_function_pointer_initializers_and_calls() {
    let assembly = emit(
        r#"
            global answer: u8 = 0
            fn add(left: u8, right: u8) -> u8 { return left + right }
            fn main() {
                let callback: ptr<fn(u8, u8)u8> = &add
                answer = callback(20, 22)
            }
        "#,
    );

    assert_eq!(assembly.matches("icall").count(), 1, "{assembly}");
    assert!(assembly.contains("jmp _add"), "{assembly}");
    assert!(assembly.contains("function_pointer_capture"), "{assembly}");
}

#[test]
fn lowers_two_result_calls_with_a_hidden_pointer_and_preserves_both_values() {
    let assembly = emit(
        r#"
            global sink: u16 = 0
            fn pair(value: u8) -> u8, u8 { return value, value + 1 }
            fn main() {
                let first: u8, second: u8 = pair(7)
                sink = cast<u16>(first) + cast<u16>(second)
            }
        "#,
    );

    assert!(assembly.contains("call _pair"), "{assembly}");
    assert!(assembly.contains("ldi r26,"), "{assembly}");
    assert!(assembly.contains("st z, r16"), "{assembly}");
}

#[test]
fn rejects_one_destination_avr_calls_to_two_result_functions() {
    let program = parse_program(
        Path::new("avr-two-result-error.ezra"),
        "fn pair() -> u8, bool { return 1, true } fn main() { let value: u8 = pair() }",
    )
    .unwrap();
    let error = emit_avr_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::Avr,
            stack_top: Address24::new(0x0AFF),
            ram_base: Address24::new(0x0104),
            rodata_base: Address24::new(0x0800),
            asset_base: Address24::new(0x0900),
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert!(
        error
            .message
            .contains("may only be used in a two-place binding or returned directly"),
        "{error}"
    );
}

#[test]
fn i32_operations_use_non_overlapping_four_byte_register_groups() {
    let assembly = emit(
        r#"
            global left_input: i32 = 0x12345678
            global right_input: i32 = 0x10203040
            global sink: i32 = 0
            fn main() {
                let left: i32 = left_input
                let right: i32 = right_input
                sink = left + right
            }
        "#,
    );
    for register in 2..=9 {
        assert!(
            assembly.contains(&format!("    mov r{register}, r16")),
            "{assembly}"
        );
        assert!(
            assembly.contains(&format!("    mov r16, r{register}")),
            "{assembly}"
        );
    }
    assert!(assembly.contains("    adc r16, r17"), "{assembly}");
}

#[test]
fn variable_indexing_scales_and_adds_without_pointer_increment_loops() {
    let byte_assembly = emit(
        r#"
            global bytes: [u8; 4] = [1, 2, 3, 4]
            global byte_sink: u8 = 0
            fn read(index: u16) -> u8 { return bytes[index] }
            fn main() { byte_sink = read(3) }
        "#,
    );
    assert!(!byte_assembly.contains("index_scale"), "{byte_assembly}");
    assert!(!byte_assembly.contains("index_done"), "{byte_assembly}");
    assert!(!byte_assembly.contains("lsl r16"), "{byte_assembly}");

    let word_assembly = emit(
        r#"
            global words: [u16; 4] = [1, 2, 3, 4]
            global word_sink: u16 = 0
            fn read(index: u16) -> u16 { return words[index] }
            fn main() { word_sink = read(3) }
        "#,
    );
    assert!(word_assembly.contains("lsl r16"), "{word_assembly}");
    assert!(word_assembly.contains("rol r16"), "{word_assembly}");
    assert!(!word_assembly.contains("index_scale"), "{word_assembly}");

    let triple_assembly = emit(
        r#"
            struct Triple { first: u8 second: u8 third: u8 }
            global triples: [Triple; 2] = [
                Triple { first: 1, second: 2, third: 3 },
                Triple { first: 4, second: 5, third: 6 }
            ]
            global triple_sink: u8 = 0
            fn read(index: u16) -> u8 { return triples[index].third }
            fn main() { triple_sink = read(1) }
        "#,
    );
    assert!(triple_assembly.contains("lsl r16"), "{triple_assembly}");
    assert!(
        triple_assembly.contains("adc r16, r17"),
        "{triple_assembly}"
    );
    assert!(
        !triple_assembly.contains("index_scale"),
        "{triple_assembly}"
    );
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
                let text: ptr<u8> = "avr"
                asm volatile(clobber memory) { "nop" }
            }
        "#,
    );
    assert!(assembly.contains("ld r16, z") || assembly.contains("st z, r16"));
    assert!(assembly.contains("lds r30, 0100h"));
    assert!(!assembly.contains("00F0h"));
    assert!(assembly.contains("ld r16, z\n    tst r16"));
}

#[test]
fn aggregate_constants_have_addressable_storage_and_mmio_names_are_writable_places() {
    let assembly = emit(
        r#"
            volatile mmio OUTPUT: ptr<u16> = 0x0A00
            const VALUES: [u16; 2] = [0x1234, 0x5678]
            fn main() {
                let pointer: ptr<u16> = &VALUES
                OUTPUT = pointer[1]
            }
        "#,
    );
    assert!(assembly.contains("sts 0A00h, r16"), "{assembly}");
    assert!(assembly.contains("sts 0800h, r16"), "{assembly}");
}

#[test]
fn const_arrays_with_characters_initialize_and_index() {
    let assembly = emit(
        r#"
            volatile mmio OUTPUT: ptr<u8> = 0x0A00
            const LEVELS: [u8; 2] = ['1', '2']
            const UNUSED: [u8; 2] = ['x', 'y']
            global values: [u8; 2] = [0, 0]

            fn read(index: u8) -> u8 { return LEVELS[index] }

            fn main() {
                let local: [u8; 2] = [3, 4]
                OUTPUT = read(1)
                values[0] = local[1]
            }
        "#,
    );

    assert!(assembly.contains("ldi r16, 31h"), "{assembly}");
    assert!(assembly.contains("ldi r16, 32h"), "{assembly}");
    assert!(assembly.contains("ldi r16, 78h"), "{assembly}");
    assert!(assembly.contains("sts 0A00h, r16"), "{assembly}");
}

#[test]
fn unrolls_constant_shifts_and_power_of_two_multiplication() {
    let assembly = emit(
        r#"
            global sink: u16 = 0
            fn main() {
                let left: u16 = 3
                sink = left << 2
                sink = sink >> 1
                let signed: i16 = -8
                sink = cast<u16>(signed >> 1)
                sink = left * 8
            }
        "#,
    );
    assert!(assembly.contains("lsl r16"), "{assembly}");
    assert!(assembly.contains("rol r16"), "{assembly}");
    assert!(assembly.contains("lsr r16"), "{assembly}");
    assert!(assembly.contains("ror r16"), "{assembly}");
    assert!(assembly.contains("asr r16"), "{assembly}");
    assert!(!assembly.contains("shift_loop"), "{assembly}");
    assert!(!assembly.contains("mul_loop"), "{assembly}");
    assert!(!assembly.contains("    mul "), "{assembly}");
    assert!(!assembly.contains("    muls "), "{assembly}");
    assert!(!assembly.contains("    mulsu "), "{assembly}");
}

#[test]
fn emits_native_wrapping_multiplication_and_restores_abi_zero_register() {
    let assembly = emit(
        r#"
            global u8_sink: u8 = 0
            global i8_sink: i8 = 0
            global i16_sink: i16 = 0
            global i24_sink: i24 = 0
            fn main() {
                let u8_left: u8 = 200; let u8_right: u8 = 3
                let i8_left: i8 = -100; let i8_right: i8 = 3
                let i16_left: i16 = -300; let i16_right: i16 = 200
                let i24_left: i24 = -70000; let i24_right: i24 = 40000
                u8_sink = u8_left * u8_right
                i8_sink = i8_left * i8_right
                i16_sink = i16_left * i16_right
                i24_sink = i24_left * i24_right
            }
        "#,
    );
    assert!(assembly.contains("    mul r18, r19"), "{assembly}");
    assert!(assembly.contains("    muls r18, r19"), "{assembly}");
    assert!(assembly.contains("    mulsu r18, r19"), "{assembly}");
    assert!(!assembly.contains("mul_loop"), "{assembly}");
    assert_eq!(
        assembly.matches("    clr r1").count(),
        5,
        "each multiply and startup must restore r1:\n{assembly}"
    );
}

#[test]
fn supports_more_than_four_and_overlapping_register_arguments() {
    let assembly = emit(
        r#"
            struct Triple { first: u8 second: u8 third: u8 }
            fn sum(a: u8, b: u16, c: u8, d: u16, e: u8) -> u16 {
                return cast<u16>(a) + b + cast<u16>(c) + d + cast<u16>(e)
            }
            fn first_after_wide(a: u8, b: u24, c: u8) -> u8 { return c }
            fn take_triple(value: Triple) -> u8 { return value.first }
            fn main() {
                let triple: Triple = Triple { first: 1, second: 2, third: 3 }
                let total: u16 = sum(1, 2, 3, 4, 5)
                let after: u8 = first_after_wide(1, 2, 3)
                let first: u8 = take_triple(triple)
            }
        "#,
    );
    assert!(assembly.contains("call _sum"), "{assembly}");
    assert!(assembly.contains("call _first_after_wide"), "{assembly}");
    assert!(assembly.contains("call _take_triple"), "{assembly}");
}

#[test]
fn arduboy_emits_complete_vector_table_and_named_handler() {
    let assembly = emit_with_arduboy_vectors(
        r#"
            interrupt fn avr_timer0_ovf() { asm volatile(clobber memory) { "nop" } }
            fn main() {}
        "#,
        true,
    );
    let lines = assembly.lines().collect::<Vec<_>>();
    let table_start = lines
        .iter()
        .position(|line| *line == "; ATmega32U4 interrupt vectors: 32 absolute 4-byte JMP entries")
        .unwrap()
        + 1;
    assert_eq!(lines[table_start], "    jmp __ezra_start");
    assert_eq!(lines[table_start + 17], "    jmp _avr_timer0_ovf");
    assert_eq!(lines[table_start + 1], "    jmp __ezra_unhandled_interrupt");
    assert!(
        lines[table_start + 32..]
            .iter()
            .take(3)
            .any(|line| *line == "__ezra_start:")
    );
    assert!(assembly.contains("push r31"));
    assert!(assembly.contains("out 3Fh, r0"));
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
            naked fn reset() { asm volatile(clobber memory) { "rjmp _reset" } }
            fn main() {}
        "#,
    );
    assert!(assembly.contains("reti"));
    assert!(assembly.contains("_reset:"));
}

#[test]
fn lowers_catalog_scalar_bit_and_integer_families() {
    let assembly = emit(
        r#"
            global sink: u32 = 0
            fn main() {
                let value: u16 = 0x1234u16
                let tested: bool = ezra.bits.test(value, 3)
                let set: u16 = ezra.bits.set(value, 4)
                let clear: u16 = ezra.bits.clear(value, 5)
                let toggled: u16 = ezra.bits.toggle(value, 6)
                let extracted: u16 = ezra.bits.extract(value, 4, 4)
                let inserted: u16 = ezra.bits.insert(value, 3u16, 4, 4)
                let swapped: u16 = ezra.bits.byte_swap(value)
                let reversed: u16 = ezra.bits.reverse(value)
                let ones: u8 = ezra.bits.count_ones(value)
                let leading: u8 = ezra.bits.leading_zeros(value)
                let trailing: u8 = ezra.bits.trailing_zeros(value)
                let rotated: u16 = ezra.bits.rotate_left(value, 3)
                let product: u16 = ezra.int.widening_mul(7u8, 9u8)
                let high: u16 = ezra.int.mul_high(value, value)
                let added: u16 = ezra.int.saturating_add(value, value)
                let subtracted: u16 = ezra.int.saturating_sub(value, value)
                sink = cast<u32>(cast<u16>(tested) + set + clear + toggled + extracted + inserted + swapped + reversed + cast<u16>(ones) + cast<u16>(leading) + cast<u16>(trailing) + rotated + product + high + added + subtracted)
            }
        "#,
    );
    assert!(
        assembly.contains("    mul r16, r18") || assembly.contains("    mul r18, r19"),
        "{assembly}"
    );
    assert!(assembly.contains("adc"), "{assembly}");
}

#[test]
fn lowers_catalog_paired_and_memory_families() {
    let assembly = emit(
        r#"
            global bytes: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8]
            global sink: u32 = 0
            fn main() {
                let left: u16 = 100
                let right: u16 = 7
                let quotient: u16, remainder: u16 = ezra.int.divmod(left, right)
                let sum: u16, carry: bool = ezra.int.add_carry(left, right, false)
                let difference: u16, borrow: bool = ezra.int.sub_borrow(left, right, true)
                let low: u16, high: u16 = ezra.int.full_mul(left, right)
                let p: ptr<u8> = &bytes[0]
                let q: ptr<u8> = &bytes[2]
                ezra.mem.copy_nonoverlapping(p, q, 2u24)
                ezra.mem.move(p, q, 4u24)
                ezra.mem.fill(p, 9, 4u24)
                let found_ptr: ptr<u8>, found: bool = ezra.mem.find_byte(p, 4u24, 9)
                let ordering: i8 = ezra.mem.compare(p, q, 4u24)
                let le16: u16 = ezra.mem.load_le16(p)
                let le24: u24 = ezra.mem.load_le24(p)
                let be16: u16 = ezra.mem.load_be16(p)
                let be24: u24 = ezra.mem.load_be24(p)
                ezra.mem.store_le16(p, le16)
                ezra.mem.store_le24(p, le24)
                ezra.mem.store_be16(p, be16)
                ezra.mem.store_be24(p, be24)
                let byte: u8 = ezra.mem.peek8(p)
                ezra.mem.poke8(p, byte)
                sink = cast<u32>(quotient + remainder + sum + difference + low + high + cast<u16>(carry) + cast<u16>(borrow) + cast<u16>(found) + cast<u16>(ordering) + le16 + cast<u16>(le24) + be16 + cast<u16>(be24) + cast<u16>(found_ptr) + cast<u16>(byte))
            }
        "#,
    );
    assert!(assembly.contains("ld r18, z+"), "{assembly}");
    assert!(assembly.contains("st x+, r18"), "{assembly}");
    assert!(assembly.contains("sbc"), "{assembly}");
}
