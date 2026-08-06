use std::path::Path;

use crate::{
    asm::AssemblyOptions,
    parser::parse_program,
    target::{Address24, AssemblerCpu, CpuFamily},
    vm::assemble_subset_with_symbols_at,
};

use super::*;

fn emit_result(source: &str) -> Result<String, Diagnostic> {
    let program = parse_program(Path::new("i8086-emitter-test.ezra"), source).unwrap();
    emit_i8086_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::I8086,
            ram_base: Address24::new(0x4000),
            rodata_base: Address24::new(0x6000),
            asset_base: Address24::new(0x7000),
            stack_top: Address24::new(0xfffe),
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        },
    )
}

fn emit(source: &str) -> String {
    emit_result(source).unwrap()
}

fn emit_error(source: &str) -> String {
    match emit_result(source) {
        Err(error) => error.message,
        Ok(_) => panic!("expected emission to fail for `{source}`"),
    }
}

fn emit_and_assemble(source: &str) -> String {
    let assembly = emit(source);
    assemble_subset_with_symbols_at(AssemblerCpu::I8086, &assembly, 0)
        .unwrap_or_else(|error| panic!("{error}\n--- assembly ---\n{assembly}"));
    assembly
}

#[test]
fn places_source_comments_inline_without_debug_anchors() {
    let assembly = emit_and_assemble(
        "fn main() {\n    let value: u8 = 1 // initialize value\n    value += 1 // increment value\n}",
    );

    assert!(assembly.contains("; initialize value"), "{assembly}");
    assert!(assembly.contains("; increment value"), "{assembly}");
    assert!(!assembly.contains("; source:"), "{assembly}");
    assert!(!assembly.contains(";   initialize value"), "{assembly}");
    assert!(!assembly.contains(";   increment value"), "{assembly}");
}

fn function_assembly<'a>(assembly: &'a str, name: &str) -> &'a str {
    assembly
        .split(&format!("_{name}:"))
        .nth(1)
        .unwrap_or_else(|| panic!("missing function `{name}`\n{assembly}"))
        .split("    ret\n")
        .next()
        .unwrap()
}

fn local_plan(source: &str, function_name: &str) -> FunctionLocals {
    let program = parse_program(Path::new("i8086-regalloc-test.ezra"), source).unwrap();
    let options = AssemblyOptions {
        cpu: CpuFamily::I8086,
        ram_base: Address24::new(0x4000),
        rodata_base: Address24::new(0x6000),
        asset_base: Address24::new(0x7000),
        stack_top: Address24::new(0xfffe),
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
    let function = tbir
        .lowered_program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.name == function_name => Some(function),
            _ => None,
        })
        .unwrap();
    plan_function_locals(function, &model).unwrap()
}

#[test]
fn i8086_register_model_tracks_ax_aliases_and_reserves_bp_for_words() {
    let target = i8086_local_target();
    assert!(target.registers_alias(PhysReg(0), PhysReg(1)));
    assert!(target.registers_alias(PhysReg(0), PhysReg(2)));
    assert!(!target.registers_alias(PhysReg(1), PhysReg(2)));
    assert!(target.registers_alias(PhysReg(3), PhysReg(4)));
    assert!(target.registers_alias(PhysReg(6), PhysReg(8)));
    assert!(target.registers_alias(PhysReg(9), PhysReg(11)));
    assert_eq!(
        target.register_classes[I8086_WORD_CLASS.0].registers,
        vec![I8086_BP_REGISTER]
    );
    assert!(
        target.register_classes[I8086_BYTE_CLASS.0]
            .registers
            .is_empty()
    );
    assert_eq!(
        target.spill_classes[I8086_STATIC_SPILL_CLASS.0].name,
        "static"
    );
}

#[test]
fn bp_word_local_avoids_local_memory_traffic_and_strictly_assembles() {
    let source = "fn value() -> u16 { let local: u16 = 42 local += 1 return local } fn main() { let result: u16 = value() }";
    let plan = local_plan(source, "value");
    assert!(matches!(
        plan.bindings["local"].location,
        PlannedLocation::Bp
    ));

    let assembly = emit_and_assemble(source);
    let value = function_assembly(&assembly, "value");
    assert!(value.contains("    mov bp,02Ah\n"), "{value}");
    assert!(value.contains("    mov ax,bp\n"), "{value}");
}

#[test]
fn calls_and_address_taking_force_word_locals_to_static_spills() {
    let call_plan = local_plan(
        "global sink: u16 = 0 fn helper() {} fn test(input: u16) { let live: u16 = input live += 1 helper() sink = live } fn main() { test(7) }",
        "test",
    );
    assert!(matches!(
        call_plan.bindings["live"].location,
        PlannedLocation::Spill(_)
    ));

    let address_plan = local_plan(
        "global sink: u16 = 0 fn main() { let addressed: u16 = 7 addressed += 1 let pointer: ptr<u16> = &addressed; *pointer = 9 sink = *pointer }",
        "main",
    );
    assert!(matches!(
        address_plan.bindings["addressed"].location,
        PlannedLocation::Spill(_)
    ));

    let asm_plan = local_plan(
        "global sink: u16 = 1 fn main() { let live: u16 = sink live += 1 asm volatile { \"nop\" } sink = live }",
        "main",
    );
    assert!(matches!(
        asm_plan.bindings["live"].location,
        PlannedLocation::Spill(_)
    ));

    emit_and_assemble(
        "global sink: u16 = 0 fn helper() {} fn test(input: u16) { let live: u16 = input live += 1 helper() sink = live } fn main() { let addressed: u16 = 9 addressed += 1 let pointer: ptr<u16> = &addressed; *pointer = 11 sink = *pointer test(sink) }",
    );
}

#[test]
fn nonoverlapping_static_locals_reuse_one_colored_spill_slot() {
    let source = "global sink: u8 = 0 fn test(input: u8) { let first: u8 = input first += 1 sink = first let second: u8 = input second += 2 sink = second } fn main() { test(1) }";
    let plan = local_plan(source, "test");
    let PlannedLocation::Spill(first) = plan.bindings["first"].location else {
        panic!("byte local must spill");
    };
    let PlannedLocation::Spill(second) = plan.bindings["second"].location else {
        panic!("byte local must spill");
    };
    assert_eq!(first, second);
    assert_eq!(plan.spill_sizes, vec![1]);

    emit_and_assemble(source);
}

#[test]
fn rejects_array_initializer_for_scalar_global_with_a_specific_message() {
    let error = emit_error(
        r#"
                global race_string: ptr<ptr<u8>> = ["NA", "HUMAN"]
                fn main() {}
            "#,
    );

    assert_eq!(
        error,
        "global `race_string` is declared `ptr<ptr<u8>>`, but its initializer is an array; use an array type such as `[ptr<u8>; 4]` for these values"
    );
}

#[test]
fn emits_typed_function_pointer_calls_through_a_trampoline() {
    let assembly = emit_and_assemble(
        r#"
                global callback: ptr<fn(u8, u8)u8> = &add
                global answer: u8 = 0

                fn add(left: u8, right: u8) -> u8 {
                    return left + right
                }

                fn main() {
                    answer = callback(20, 22)
                }
            "#,
    );

    assert!(assembly.contains("mov ax,__ezra_fn_ptr_add"), "{assembly}");
    assert!(assembly.contains("__ezra_fn_ptr_add:"), "{assembly}");
    assert!(assembly.contains("call bx"), "{assembly}");
    assert!(assembly.contains("call near _add"), "{assembly}");
}

#[test]
fn lowers_two_result_calls_with_a_hidden_pointer_and_preserves_both_values() {
    let assembly = emit_and_assemble(
        r#"
                global sink: u32 = 0
                fn pair(value: u16) -> u16, u32 {
                    return value, cast<u32>(value) + 1u32
                }
                fn main() {
                    let first: u16, second: u32 = pair(7)
                    sink = cast<u32>(first) + second
                }
            "#,
    );

    assert!(assembly.contains("call near _pair"), "{assembly}");
    assert!(assembly.contains("mov [bx],al"), "{assembly}");
    assert!(assembly.contains("mov bx,"), "{assembly}");
}

#[test]
fn rejects_one_destination_8086_calls_to_two_result_functions() {
    let error =
        emit_error("fn pair() -> u8, bool { return 1, true } fn main() { let value: u8 = pair() }");
    assert!(
        error.contains("may only be used in a two-place binding or returned directly"),
        "{error}"
    );
}

#[test]
fn preserves_value_returns_across_void_calls() {
    let assembly = emit_and_assemble(
        r#"
                fn clear() {}
                fn read() -> u8 {
                    let key: u8 = 13
                    clear()
                    return key
                }
                fn main() { let key: u8 = read() }
            "#,
    );
    let read = function_assembly(&assembly, "read");
    let call = read.find("call near _clear").expect("missing clear call");
    let returned = read[call..]
        .find("mov [04000h],al")
        .expect("missing return value store");
    assert!(
        !read[call..call + returned].contains("mov [04000h],ax"),
        "void call must not clear r0 before the enclosing value return:\n{read}"
    );
}

#[test]
fn emits_strict_dos_com_startup_and_return_termination_at_0100h() {
    let program = parse_program(
        Path::new("dos-com-test.ezra"),
        "global VALUE: u16 = 7\nfn main() {}",
    )
    .unwrap();
    let assembly = emit_i8086_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::I8086,
            dos_executable: true,
            entry_addr: Address24::new(0x0100),
            code_base: Address24::new(0x0100),
            ram_base: Address24::new(0xA000),
            rodata_base: Address24::new(0x8000),
            asset_base: Address24::new(0xC000),
            stack_top: Address24::new(0xFFFE),
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        },
    )
    .unwrap();
    let image = assemble_subset_with_symbols_at(AssemblerCpu::I8086, &assembly, 0x0100)
        .unwrap_or_else(|error| panic!("{error}\n--- assembly ---\n{assembly}"));
    let start = image
        .symbols
        .iter()
        .find(|symbol| symbol.name == "__ezra_start")
        .unwrap();

    assert_eq!(start.addr, 0x0100);
    assert!(assembly.contains("    mov ax,cs\n"));
    assert!(assembly.contains("    mov ds,ax\n"));
    assert!(assembly.contains("    mov es,ax\n"));
    assert!(!assembly.contains("    mov ss,ax\n"));
    assert!(!assembly.contains("    mov sp,0FFFEh\n"));
    assert!(assembly.contains("    mov es,ax\n    mov bx,[0002h]\n"));
    assert!(assembly.contains("    cmp bx,0F80h\n"));
    assert!(assembly.contains("    mov ax,0x4cff\n    int 0x21\n"));
    assert!(assembly.contains("__ezra_dos_memory_ready:\n    cld\n"));
    assert!(!assembly.contains("    sti\n"));
    assert!(assembly.contains("    call near _main\n"));
    assert!(assembly.contains("__ezra_exit:\n    mov ax,0x4c00\n    int 0x21\n"));
    assert!(!assembly.contains("    cli\n"));
    assert!(!assembly.contains("    jmp near __ezra_exit\n"));
}

#[test]
fn omits_unreachable_i8086_functions_without_opaque_assembly_references() {
    let assembly = emit("fn unused() {}\nfn main() {}");

    assert!(!assembly.contains("_unused:"));
    assert!(assembly.contains("_main:"));
}

#[test]
fn retains_root_functions_for_inline_assembly_symbol_references() {
    let assembly = emit("fn helper() {}\nfn main() { asm volatile { \"call _helper\" } }");

    assert!(assembly.contains("_helper:"));
    assert!(assembly.contains("    call _helper\n"));
}

#[test]
fn preserves_bare_i8086_startup_and_nonreturning_exit() {
    let assembly = emit("fn main() {}");

    assert!(assembly.contains("__ezra_start:\n    cli\n"));
    assert!(assembly.contains("__ezra_exit:\n    jmp short __ezra_exit\n"));
    assert!(!assembly.contains("    int 0x21\n"));
}

#[test]
fn assembles_scalars_calls_control_flow_and_all_operators() {
    let assembly = emit_and_assemble(
        r#"
            fn twice(value: u16) -> u16 { return value * 2 }
            fn main() {
                let a: u16 = 17
                let b: u16 = twice(a)
                let c: i16 = -3
                let bits: u16 = ((b + 1) - 2) & 0xff | 0x100 ^ 3
                let shifts: u16 = (bits << 2) >> 1
                let math: u16 = shifts / 3 + shifts % 3
                let inv: u16 = ~math
                let neg: i16 = -c
                let truth: bool = !(a == b) && (a != b || a < b) && a <= b && b > a && b >= a
                if truth { a += 1 } else { a -= 1 }
                while a != 0 { a -= 1 if a == 2 { break } else { continue } }
                loop { break }
            }
        "#,
    );
    assert!(assembly.contains("call near _twice"));
    assert!(!assembly.contains("jcxz"));
    assert!(assembly.contains("rcl al,1"));
}

#[test]
fn uses_native_wrapping_multiply_for_bytes_and_words() {
    let assembly = emit_and_assemble(
        r#"
            fn unsigned_byte(left: u8, right: u8) -> u8 { return left * right }
            fn signed_byte(left: i8, right: i8) -> i8 { return left * right }
            fn unsigned_word(left: u16, right: u16) -> u16 { return left * right }
            fn signed_word(left: i16, right: i16) -> i16 { return left * right }
            fn main() {
                let ub: u8 = unsigned_byte(255, 2)
                let sb: i8 = signed_byte(-128, 2)
                let uw: u16 = unsigned_word(65535, 2)
                let sw: i16 = signed_word(-32768, 2)
            }
        "#,
    );

    assert!(assembly.contains("    mul bl\n"), "{assembly}");
    assert!(assembly.contains("    imul bl\n"), "{assembly}");
    assert!(assembly.contains("    mul bx\n"), "{assembly}");
    assert!(assembly.contains("    imul bx\n"), "{assembly}");
    assert!(!assembly.contains("multiply_loop"), "{assembly}");
}

#[test]
fn specializes_one_bit_shifts_and_power_of_two_multiplication() {
    let assembly = emit_and_assemble(
        r#"
            fn left_one(value: u16) -> u16 { return value << 1 }
            fn right_one(value: i16) -> i16 { return value >> 1 }
            fn times_eight(value: u16) -> u16 { return value * 8 }
            fn main() {
                let left: u16 = left_one(3)
                let right: i16 = right_one(-4)
                let product: u16 = times_eight(7)
            }
        "#,
    );

    let left_one = assembly
        .split("_left_one:")
        .nth(1)
        .unwrap()
        .split("ret")
        .next()
        .unwrap();
    let right_one = assembly
        .split("_right_one:")
        .nth(1)
        .unwrap()
        .split("ret")
        .next()
        .unwrap();
    let times_eight = assembly
        .split("_times_eight:")
        .nth(1)
        .unwrap()
        .split("ret")
        .next()
        .unwrap();
    assert!(left_one.contains("shl al,1"), "{left_one}");
    assert!(!left_one.contains("shift_loop"), "{left_one}");
    assert!(right_one.contains("sar al,1"), "{right_one}");
    assert!(!right_one.contains("shift_loop"), "{right_one}");
    assert!(times_eight.contains("shl al,1"), "{times_eight}");
    assert!(!times_eight.contains("mul "), "{times_eight}");
}

#[test]
fn assembles_complete_u32_i32_scalar_support_as_little_endian_word_pairs() {
    let assembly = emit_and_assemble(
        r#"
            global ULEFT: u32 = 0xFEDCBA98u32
            global URIGHT: u32 = 0x12345678u32
            global SLEFT: i32 = -2147483648i32
            global SRIGHT: i32 = -3i32
            global URESULT: u32 = 0
            global SRESULT: i32 = 0
            global UBOOL: bool = false
            global SBOOL: bool = false
            fn round_trip(value: u32, signed: i32) -> u32 {
                let local: u32 = value
                let converted: i32 = cast<i32>(local)
                SRESULT = signed + converted
                return cast<u32>(SRESULT)
            }
            fn main() {
                URESULT = round_trip(ULEFT, SRIGHT)
                URESULT = ULEFT + URIGHT
                URESULT = ULEFT - URIGHT
                URESULT = ULEFT * URIGHT
                URESULT = ULEFT / URIGHT
                URESULT = ULEFT % URIGHT
                URESULT = ULEFT & URIGHT | (ULEFT ^ URIGHT)
                URESULT = ULEFT << 17
                URESULT = ULEFT >> 17
                SRESULT = SLEFT / SRIGHT
                SRESULT = SLEFT % SRIGHT
                SRESULT = SLEFT >> 31
                UBOOL = ULEFT > URIGHT
                SBOOL = SLEFT < SRIGHT
                URESULT += 1u32
                URESULT -= 1u32
                URESULT *= 3u32
                URESULT /= 3u32
                URESULT %= 5u32
                URESULT = ULEFT / 0u32
                URESULT = ULEFT % 0u32
            }
        "#,
    );

    assert!(assembly.contains("    mov [040"), "{assembly}");
    assert!(assembly.contains("    add ax,"), "{assembly}");
    assert!(assembly.contains("    adc ax,"), "{assembly}");
    assert!(assembly.contains("    sub ax,"), "{assembly}");
    assert!(assembly.contains("    sbb ax,"), "{assembly}");
    assert!(assembly.contains("    and ax,"), "{assembly}");
    assert!(assembly.contains("    mul bx\n"), "{assembly}");
    assert!(assembly.contains("divide_less"), "{assembly}");
    assert!(assembly.contains("xor al,80h"), "{assembly}");
    assert!(assembly.contains("sar ax,1"), "{assembly}");
}

#[test]
fn infers_32_bit_integer_literals_and_preserves_bit_patterns() {
    let assembly = emit_and_assemble(
        "global U: u32 = 0 global I: i32 = 0 global SAME: bool = false fn main() { U = 4294967295 I = 2147483648i32 I = -2147483648 SAME = U == 4294967295 }",
    );

    assert!(assembly.contains("    mov ax,0FFFFh"), "{assembly}");
    assert!(assembly.contains("    mov ax,08000h"), "{assembly}");
}

#[test]
fn optimizes_u32_immediates_identities_and_compound_assignments() {
    let assembly = emit_and_assemble(
        r#"
            fn immediate(value: u32) -> bool {
                let work: u32 = value + 0x12345678u32
                work -= 1u32
                work &= 0xFF00FF00u32
                work |= 0x00010001u32
                work ^= 0x80000000u32
                return work >= 0x10203040u32
            }
            fn identities(value: u32) -> u32 {
                let work: u32 = value + 0u32
                work = work * 1u32
                work = work | 0u32
                work = work & 0xFFFFFFFFu32
                return work
            }
            fn main() { let result: bool = immediate(7) let same: u32 = identities(9) }
        "#,
    );
    let immediate = function_assembly(&assembly, "immediate");
    assert!(immediate.contains("add ax,05678h"), "{immediate}");
    assert!(immediate.contains("adc ax,01234h"), "{immediate}");
    assert!(immediate.contains("sbb ax,00h"), "{immediate}");
    assert!(immediate.contains("and ax,0FF00h"), "{immediate}");
    assert!(immediate.contains("cmp ax,01020h"), "{immediate}");
    let identities = function_assembly(&assembly, "identities");
    assert!(!identities.contains(" mul "), "{identities}");
    assert!(!identities.contains("add ax,00h"), "{identities}");
}

#[test]
fn optimizes_all_fixed_u32_shift_ranges_with_word_pairs() {
    let assembly = emit_and_assemble(
        r#"
            fn s0(v: u32) -> u32 { return v << 0 }
            fn s1(v: u32) -> u32 { return v << 1 }
            fn s15(v: u32) -> u32 { return v >> 15 }
            fn s16(v: u32) -> u32 { return v << 16 }
            fn s17(v: u32) -> u32 { return v >> 17 }
            fn s31(v: i32) -> i32 { return v >> 31 }
            fn s32(v: u32) -> u32 { return v << 32 }
            fn s40(v: i32) -> i32 { return v >> 40 }
            fn main() { let x: u32 = s0(1) + s1(1) + s15(1) + s16(1) + s17(1) + cast<u32>(s31(-1)) + s32(1) + cast<u32>(s40(-1)) }
        "#,
    );
    assert!(!function_assembly(&assembly, "s0").contains("shift_loop"));
    assert!(function_assembly(&assembly, "s1").contains("shl ax,1"));
    assert!(function_assembly(&assembly, "s15").contains("rcr ax,1"));
    assert!(!function_assembly(&assembly, "s16").contains("shl ax,1"));
    assert!(function_assembly(&assembly, "s17").contains("shr ax,1"));
    assert!(function_assembly(&assembly, "s31").contains("sar ax,1"));
    assert!(function_assembly(&assembly, "s32").contains("mov ax,00h"));
    assert!(function_assembly(&assembly, "s40").contains("sar ax,1"));
}

#[test]
fn negates_u32_word_pairs_with_the_correct_low_word_carry_rule() {
    let assembly = emit_and_assemble(
        r#"
            global LOW_NONZERO: i32 = 0x00000001i32
            global LOW_ZERO: i32 = 0x00010000i32
            global MINIMUM: i32 = 0x80000000i32
            fn main() {
                LOW_NONZERO = -LOW_NONZERO
                LOW_ZERO = -LOW_ZERO
                MINIMUM = -MINIMUM
            }
        "#,
    );

    let main = function_assembly(&assembly, "main");
    assert_eq!(main.matches("negate_low_nonzero").count(), 6, "{main}");
    assert_eq!(main.matches("    neg ax\n").count(), 6, "{main}");
    assert_eq!(main.matches("    not ax\n").count(), 3, "{main}");
    assert!(!main.contains("    adc ax,0\n"), "{main}");
    assert_eq!(0x0000_0001u32.wrapping_neg(), 0xffff_ffff);
    assert_eq!(0x0001_0000u32.wrapping_neg(), 0xffff_0000);
    assert_eq!(0x8000_0000u32.wrapping_neg(), 0x8000_0000);
}

#[test]
fn negative_signed_power_of_two_divisors_use_general_division() {
    let assembly = emit_and_assemble(
        r#"
            fn divide_min(value: i32) -> i32 { return value / 0x80000000i32 }
            fn modulo_min(value: i32) -> i32 { return value % 0x80000000i32 }
            fn main() {
                let min_by_min: i32 = divide_min(0x80000000i32)
                let positive_by_min: i32 = divide_min(7i32)
                let min_mod_min: i32 = modulo_min(0x80000000i32)
                let positive_mod_min: i32 = modulo_min(7i32)
            }
        "#,
    );

    let divide = function_assembly(&assembly, "divide_min");
    let modulo = function_assembly(&assembly, "modulo_min");
    for function in [divide, modulo] {
        assert!(
            !function.contains("signed_power_two_nonnegative"),
            "{function}"
        );
        assert!(function.contains("divide_less"), "{function}");
        assert!(function.contains("test al,80h"), "{function}");
    }
}

#[test]
fn optimizes_u32_multiply_and_constant_and_u16_division() {
    let assembly = emit_and_assemble(
        r#"
            fn product(a: u32, b: u32) -> u32 { return a * b }
            fn unsigned_pow2(v: u32) -> u32 { return v / 16u32 + v % 16u32 }
            fn signed_pow2(v: i32) -> i32 { return v / 8i32 + v % 8i32 }
            fn narrow_divisor(v: u32, d: u32) -> u32 { return v / d + v % d }
            fn main() { let x: u32 = product(3, 5) + unsigned_pow2(33) + cast<u32>(signed_pow2(-33)) + narrow_divisor(100, 7) }
        "#,
    );
    let product = function_assembly(&assembly, "product");
    assert_eq!(product.matches("mul bx").count(), 3, "{product}");
    assert!(!product.contains("multiply_skip_add"), "{product}");
    let unsigned = function_assembly(&assembly, "unsigned_pow2");
    assert!(unsigned.contains("shr ax,1"), "{unsigned}");
    assert!(unsigned.contains("and ax,0Fh"), "{unsigned}");
    let signed = function_assembly(&assembly, "signed_pow2");
    assert!(signed.contains("signed_power_two_nonnegative"), "{signed}");
    assert!(signed.contains("add ax,07h"), "{signed}");
    let narrow = function_assembly(&assembly, "narrow_divisor");
    assert!(narrow.contains("divide_u32_generic"), "{narrow}");
    assert!(narrow.contains("div bx"), "{narrow}");
}

#[test]
fn assembles_aggregates_pointers_access_and_memory_helpers() {
    let assembly = emit_and_assemble(
        r#"
            struct Pair { lo: u8 hi: u16 }
            global DATA: [u8; 4] = [1, 2, 3, 4]
            global PAIR: Pair = Pair { lo: 7, hi: 0x1234 }
            fn main() {
                let local: [u8; 4] = DATA
                let pair: Pair = PAIR
                let p: ptr<u8> = &local[0]
                let q: ptr<u16> = &pair.hi
                local[1] = *p
                pair.hi = *q
                pair.hi += 1
                let x: u8 = local[2]
                let y: u16 = pair.hi
                mem.poke8(p, mem.peek8(p))
                mem.memcpy(&DATA[0], p, 4)
                mem.memset(p, x, 4)
            }
        "#,
    );
    assert!(assembly.contains("mov al,[bx]"));
    assert!(assembly.contains("    rep movsb\n"), "{assembly}");
    assert!(assembly.contains("    rep stosb\n"), "{assembly}");
    assert!(assembly.contains("copy_manual"), "{assembly}");
    assert!(assembly.contains("fill_manual"), "{assembly}");
    assert!(!assembly.contains("jcxz short"), "{assembly}");
}

#[test]
fn assembles_ports_inline_asm_interrupt_and_naked_functions() {
    let assembly = emit_and_assemble(
        r#"
            port UART: u8 = 0x20
            interrupt fn irq() { asm volatile(clobber memory) { "nop" } }
            naked fn raw() { asm volatile(clobber memory) { "ret" } }
            fn main() {
                let value: u8 = in UART
                let result: u8 = 0
                out UART, value
                asm volatile(in value: u8 as reg8, out result: u8 as reg8) { "mov {result},{value}" }
            }
        "#,
    );
    assert!(assembly.contains("in al,020h"));
    assert!(assembly.contains("out 020h,al"));
    assert!(assembly.contains("iret"));
}

#[test]
fn explicit_inline_assembles_typed_arguments_nested_helpers_and_safe_fallbacks() {
    let assembly = emit_and_assemble(
        r#"
            global sequence: u8 = 0
            fn next() -> u8 { sequence += 1 return sequence }
            inline fn decimal_pair(first: u8, second: u8) -> u8 {
                return first * 10 + second
            }
            inline fn nested_pair(first: u8, second: u8) -> u8 {
                return decimal_pair(first, second)
            }
            inline fn ready() -> bool {
                sequence += 1
                return sequence < 5
            }
            fn main() {
                let pair: u8 = nested_pair(next(), next())
                let short_circuit: bool = false && ready()
                while ready() { sequence += pair }
            }
        "#,
    );

    assert_eq!(assembly.matches("call near _next").count(), 2, "{assembly}");
    assert!(!assembly.contains("call near _decimal_pair"), "{assembly}");
    assert!(!assembly.contains("call near _nested_pair"), "{assembly}");
    assert!(!assembly.contains("_decimal_pair:"), "{assembly}");
    assert!(!assembly.contains("_nested_pair:"), "{assembly}");
    assert!(assembly.contains("_ready:"), "{assembly}");
    assert_eq!(
        assembly.matches("call near _ready").count(),
        1,
        "{assembly}"
    );
}

#[test]
fn recursive_calls_save_static_frames() {
    let assembly = emit_and_assemble(
        r#"
            fn gcd(a: u16, b: u16) -> u16 {
                if b == 0 { return a }
                let next: u16 = gcd(b, a % b)
                return next
            }
            fn main() { let value: u16 = gcd(48, 18) }
        "#,
    );
    assert!(assembly.contains("call near _gcd"));
    assert!(assembly.contains("mov bp,ax"), "{assembly}");
}

#[test]
fn non_tail_recursive_expression_calls_preserve_continuation_values() {
    let assembly = emit_and_assemble(
        r#"
            global result: u16 = 0
            fn accumulate(value: u16) -> u16 {
                if value == 0 { return 1 }
                return accumulate(value - 1) + value
            }
            fn main() { result = accumulate(3) }
        "#,
    );
    let accumulate = function_assembly(&assembly, "accumulate");
    assert!(accumulate.contains("call near _accumulate"), "{accumulate}");
    assert!(accumulate.contains("push ax"), "{accumulate}");
    assert!(accumulate.contains("pop ax"), "{accumulate}");
}

#[test]
fn interrupt_handlers_preserve_scratch_and_reject_unsafe_calls() {
    let assembly = emit_and_assemble(
        r#"
            interrupt fn irq() { asm volatile { "nop" } }
            fn main() {}
        "#,
    );
    let irq = assembly
        .split("_irq:")
        .nth(1)
        .unwrap()
        .split("iret")
        .next()
        .unwrap();
    assert_eq!(irq.matches("push ax").count(), 13, "{irq}");
    assert_eq!(irq.matches("pop ax").count(), 13, "{irq}");
    let save_ds = irq.find("push ds").unwrap();
    let establish_ds = irq.find("mov ds,ax").unwrap();
    let save_scratch = irq[establish_ds..].find("mov ax,[04000h]").unwrap() + establish_ds;
    assert!(
        save_ds < establish_ds && establish_ds < save_scratch,
        "{irq}"
    );
    let restore_scratch = irq
        .rfind("mov [04000h],ax")
        .or_else(|| irq.rfind("mov [04000h],al"))
        .unwrap();
    let restore_ds = irq.rfind("pop ds").unwrap();
    assert!(restore_scratch < restore_ds, "{irq}");

    let error = emit_error(
        r#"
            interrupt fn irq() {}
            fn main() { irq() }
        "#,
    );
    assert!(
        error.contains("cannot be called with ordinary `call`"),
        "{error}"
    );

    let error = emit_error(
        r#"
            fn helper() {}
            interrupt fn irq() { helper() }
            fn main() {}
        "#,
    );
    assert!(
        error.contains("static-frame ABI is not reentrant"),
        "{error}"
    );

    let error = emit_error("interrupt fn main() {}");
    assert!(
        error.contains("main` cannot be an interrupt function"),
        "{error}"
    );
}

#[test]
fn signed_aliases_drive_extension_comparison_shift_division_and_modulo() {
    let assembly = emit_and_assemble(
        r#"
            alias sbyte = i8
            global VALUE: sbyte = 0
            global WIDE: i16 = 0
            global BEFORE: bool = false
            global SHIFTED: sbyte = 0
            global QUOTIENT: sbyte = 0
            global REMAINDER: sbyte = 0
            fn negative() -> sbyte { return -1 }
            fn main() {
                VALUE = negative()
                WIDE = negative()
                BEFORE = 0 > VALUE
                SHIFTED = VALUE >> 1
                QUOTIENT = VALUE / 2
                REMAINDER = VALUE % 2
            }
        "#,
    );
    let sign_extension = "    sar al,1\n".repeat(7);
    assert!(assembly.contains(&sign_extension), "{assembly}");
    assert!(assembly.matches("xor al,80h").count() >= 2, "{assembly}");
    assert!(assembly.contains("test al,80h"), "{assembly}");

    for (source, expected) in [
        (
            "global result: bool = false fn main() { let a: i8 = -1 let b: u8 = 1 result = a < b }",
            "signed/unsigned mix without cast",
        ),
        (
            "global result: bool = false fn main() { let a: u8 = 1 let b: u16 = 2 result = a < b }",
            "same width without cast",
        ),
        (
            "global result: bool = false fn main() { let value: u8 = 1 result = 300 > value }",
            "literal 300 is outside type `u8`",
        ),
    ] {
        let error = emit_error(source);
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
}

#[test]
fn compound_indirect_lvalues_are_evaluated_once() {
    let assembly = emit_and_assemble(
        r#"
            global DATA: [u8; 4] = [1, 2, 3, 4]
            fn next() -> u16 { return 1 }
            fn main() { DATA[next()] += 7 }
        "#,
    );
    assert_eq!(assembly.matches("call near _next").count(), 1, "{assembly}");
}

#[test]
fn inline_asm_loads_inputs_substitutes_classes_and_writes_outputs() {
    let assembly = emit_and_assemble(
        r#"
            const PORT: u8 = 0x20
            alias Word = u16
            fn main() {
                let input: u8 = 4
                let output: u16 = 0
                let cell: u8 = 1
                let pointer: ptr<u8> = &cell
                let alias_input: Word = 0x1234
                asm volatile(
                    in input: u8 as reg8,
                    in PORT: u8 as imm,
                    out output: u16 as reg16,
                    clobber ax
                ) { "add {input},1" "mov dx,{PORT}" "xor ah,ah" }
                asm volatile(in cell: u8 as mem, clobber memory) { "mov al,{cell}" }
                asm volatile(in pointer: ptr<u8> as reg16, clobber ax) { "mov bx,{pointer}" }
                asm volatile(in alias_input: Word as reg16, clobber ax) { "mov bx,{alias_input}" }
            }
        "#,
    );
    assert!(assembly.contains("add al,1"), "{assembly}");
    assert!(assembly.contains("mov dx,020h"), "{assembly}");
    assert!(assembly.contains("mov [040"), "{assembly}");
    let add = assembly.find("add al,1").unwrap();
    assert!(assembly[..add].rfind("mov al,[").is_some(), "{assembly}");
    assert!(assembly[add..].contains(",ax"), "{assembly}");
}

#[test]
fn inline_asm_rejects_invalid_operands_and_critical_clobbers() {
    for (source, expected) in [
        (
            "fn main() { let x: u8 = 0 asm(in x: u8 as reg8, out x: u8 as reg8) { \"nop\" } }",
            "duplicate inline asm operand",
        ),
        (
            "fn main() { let x: u8 = 0 asm(out x: u8 as imm) { \"nop\" } }",
            "cannot use imm class",
        ),
        (
            "fn main() { let x: u24 = 0 asm(in x: u24 as reg24) { \"nop\" } }",
            "reg24` is not supported",
        ),
        (
            "fn main() { let x: u8 = 0 asm(in x: u16 as reg16) { \"nop\" } }",
            "does not match bound type",
        ),
        (
            "fn main() { let x: u8 = 0 asm(in x: u8 as imm) { \"nop\" } }",
            "must be a compile-time constant",
        ),
        (
            "fn main() { asm(clobber ds) { \"nop\" } }",
            "ABI-critical register `ds`",
        ),
        (
            "fn main() { asm(clobber ax, clobber al) { \"nop\" } }",
            "duplicate or overlapping inline asm clobber",
        ),
        (
            "fn main() { asm { \"mov al,{missing}\" } }",
            "unknown inline asm operand placeholder",
        ),
    ] {
        let error = emit_error(source);
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
}

#[test]
fn mmio_pointer_roots_and_constant_array_bounds_are_checked() {
    emit_and_assemble(
        r#"
            struct Registers { status: u8 data: [u8; 2] }
            volatile mmio REGS: ptr<Registers> = 0x2000
            volatile mmio BYTES: ptr<u8> = 0x2100
            fn main() {
                let direct: u8 = BYTES[0]
                BYTES[1] = direct
                BYTES[0] += 1
                let direct_pointer: ptr<u8> = &BYTES[1]
                let status: u8 = REGS.status
                REGS.data[1] = status
                let pointer: ptr<u8> = &REGS.data[0]
                let raw: ptr<u8> = cast<ptr<u8>>(0x3000)
                let unrestricted: u8 = raw[99]
            }
        "#,
    );

    for source in [
        "fn main() { let xs: [u8; 2] = [1, 2] let x: u8 = xs[2] }",
        "fn main() { let xs: [u8; 2] = [1, 2] xs[-1] = 0 }",
        "fn main() { let xs: [u8; 2] = [1, 2] let p: ptr<u8> = &xs[3] }",
        "struct Rows { values: [u8; 2] } fn main() { let rows: Rows = Rows { values: [1, 2] } let x: u8 = rows.values[2] }",
    ] {
        let error = emit_error(source);
        assert!(error.contains("out of bounds"), "{error}");
    }

    for source in [
        "fn main() { let xs: [u8; 2] = [1, 2] let x: u8 = xs[true] }",
        "fn main() { let p: ptr<u8> = cast<ptr<u8>>(0x3000) let i: i16 = 1 let x: u8 = p[i] }",
        "fn main() { let p: ptr<u8> = cast<ptr<u8>>(0x3000) let i: u24 = 1 let x: u8 = p[i] }",
    ] {
        let error = emit_error(source);
        assert!(
            error.contains("index must have type `u8` or `u16`"),
            "{error}"
        );
    }
}

#[test]
fn rejects_invalid_returns_ports_and_aggregate_call_signatures() {
    for (source, expected) in [
        (
            "fn value() -> u8 { return } fn main() {}",
            "must return 1 value",
        ),
        (
            "fn value(flag: bool) -> u8 { if flag { return 1 } } fn main() {}",
            "may fall through",
        ),
        ("port BAD: u16 = 1 fn main() {}", "must have type `u8`"),
        (
            "port BAD: u8 = true fn main() {}",
            "address must be an integer constant",
        ),
        (
            "const FLAG: bool = true port BAD: u8 = FLAG fn main() {}",
            "address must be an integer constant",
        ),
        (
            "const FLAG: bool = true port BAD: u8 = 1 + FLAG fn main() {}",
            "address must be an integer constant",
        ),
        (
            "const FLAG: bool = true port BAD: u8 = FLAG + 1 fn main() {}",
            "address must be an integer constant",
        ),
        (
            "fn take(values: [u8; 2]) {} fn main() {}",
            "pass it by pointer",
        ),
        (
            "struct Pair { value: u8 } fn make() -> Pair { return Pair { value: 1 } } fn main() {}",
            "pass it by pointer",
        ),
        (
            "alias Bytes = [u8; 2] extern asm fn take(values: Bytes) fn main() {}",
            "pass it by pointer",
        ),
    ] {
        let error = emit_error(source);
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
}

#[test]
fn divergent_value_functions_do_not_fall_through() {
    emit_and_assemble(
        r#"
            fn forever() -> u8 { loop {} }
            fn also_forever() -> u8 { while (true) {} }
            fn unreachable_break() -> u8 { loop { if (false) { break } } }
            fn break_after_return() -> u8 { loop { return 1; break } }
            fn main() {}
        "#,
    );
}

#[test]
fn operation_scratch_allocation_failures_are_diagnostics_not_panics() {
    let options = AssemblyOptions {
        cpu: CpuFamily::I8086,
        ram_base: Address24::new(0xffdf),
        rodata_base: Address24::new(0),
        asset_base: Address24::new(0),
        stack_top: Address24::new(0xfffe),
        default_sdk_symbols: false,
        ..AssemblyOptions::default()
    };
    let multiply = parse_program(
            Path::new("operation-scratch.ezra"),
            "volatile mmio OUTPUT: ptr<u16> = 0x1000 fn main() { let a: u16 = 2 let b: u16 = 3; *OUTPUT = a * b }",
        )
        .unwrap();
    emit_i8086_assembly_with_options(&multiply, options.clone()).unwrap();

    let divide = parse_program(
            Path::new("operation-scratch.ezra"),
            "volatile mmio OUTPUT: ptr<u16> = 0x1000 fn main() { let a: u16 = 6 let b: u16 = 3; *OUTPUT = a / b }",
        )
        .unwrap();
    let error = emit_i8086_assembly_with_options(&divide, options.clone()).unwrap_err();
    assert!(
        error
            .message
            .contains("storage exceeds target address space"),
        "{error}"
    );

    let pointer_add = parse_program(
            Path::new("operation-scratch.ezra"),
            "volatile mmio OUTPUT: ptr<u16> = 0x1000 fn main() { let p: ptr<u16> = cast<ptr<u16>>(0x1000) let n: u16 = 1 let q: ptr<u16> = p + n; *OUTPUT = *q }",
        )
        .unwrap();
    emit_i8086_assembly_with_options(&pointer_add, options).unwrap();
}

#[test]
fn scratch_allocation_failure_is_a_diagnostic_not_a_panic() {
    let program = parse_program(Path::new("scratch.ezra"), "fn main() {}").unwrap();
    let error = emit_i8086_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::I8086,
            ram_base: Address24::new(0xfff0),
            rodata_base: Address24::new(0),
            asset_base: Address24::new(0),
            stack_top: Address24::new(0xfffe),
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert!(
        error
            .message
            .contains("storage exceeds target address space")
    );
}

#[test]
fn lowers_catalog_scalar_bit_and_integer_families() {
    let assembly = emit_and_assemble(
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
    assert!(assembly.contains("    mul bl\n"), "{assembly}");
    assert!(
        assembly.contains("    imul") || assembly.contains("    mul bx"),
        "{assembly}"
    );
}

#[test]
fn lowers_catalog_paired_and_memory_families() {
    let assembly = emit_and_assemble(
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
    assert!(assembly.contains("    rep movsb\n"), "{assembly}");
    assert!(assembly.contains("    rep stosb\n"), "{assembly}");
    assert!(assembly.contains("copy_backward"), "{assembly}");
}
