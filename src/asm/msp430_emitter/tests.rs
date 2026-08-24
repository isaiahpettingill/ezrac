use std::path::Path;

use super::*;
use crate::{
    asm::AssemblyOptions,
    parser::parse_program,
    target::{Address24, AssemblerCpu, CpuFamily},
    vm::assemble_subset_with_symbols_at,
};

fn options(cpu: CpuFamily) -> AssemblyOptions {
    AssemblyOptions {
        cpu,
        load_addr: Address24::new(0x0100),
        entry_addr: Address24::new(0x0100),
        code_base: Address24::new(0x0100),
        stack_top: Address24::new(0xFFFE),
        ram_base: Address24::new(0xA000),
        rodata_base: Address24::new(0x8000),
        asset_base: Address24::new(0xC000),
        default_sdk_symbols: false,
        ..AssemblyOptions::default()
    }
}

fn emit(source: &str, cpu: CpuFamily) -> String {
    let program = parse_program(Path::new("msp430.ezra"), source).unwrap();
    let assembly = emit_msp430_assembly_with_options(&program, options(cpu)).unwrap();
    assemble_subset_with_symbols_at(AssemblerCpu::from(cpu), &assembly, 0x0100)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    assembly
}

#[test]
fn emits_and_assembles_scalar_globals_locals_calls_and_control_flow() {
    let assembly = emit(
        r#"
            global result: u16 = 0

            fn add(left: u16, right: u16) -> u16 {
                return left + right
            }

            fn main() {
                let value: u16 = add(20, 22)
                if value == 42 { result = value }
                while result < 43 { result += 1 }
            }
        "#,
        CpuFamily::Msp430,
    );

    assert!(assembly.contains("; target: MSP430"), "{assembly}");
    assert!(assembly.contains("mov r4,&0xA000"), "{assembly}");
    assert!(assembly.contains("call #_add"), "{assembly}");
    assert!(assembly.contains("cmp r5,r4"), "{assembly}");
    assert!(assembly.contains("ret"), "{assembly}");
}

#[test]
fn emits_typed_function_pointer_calls() {
    let assembly = emit(
        "global callback: ptr<fn(u16, u16)u16> = &add global result: u16 = 0 fn add(left: u16, right: u16) -> u16 { return left + right } fn main() { let local: ptr<fn(u16, u16)u16> = &add; result = callback(20, 22); result = local(20, 22) }",
        CpuFamily::Msp430,
    );
    assert_eq!(assembly.matches("call r4").count(), 2, "{assembly}");
    assert!(assembly.contains("_add:"), "{assembly}");
}

#[test]
fn emits_all_scalar_comparison_forms() {
    let assembly = emit(
        r#"
            global result: bool = false
            fn main() {
                if 1u16 <= 2u16 { result = true }
                if 2u16 > 1u16 { result = true }
                if -1i16 < 1i16 { result = true }
                if -1i16 >= -2i16 { result = true }
            }
        "#,
        CpuFamily::Msp430,
    );
    assert!(assembly.contains("jlo"), "{assembly}");
    assert!(assembly.contains("jhs"), "{assembly}");
    assert!(assembly.contains("jge"), "{assembly}");
    assert!(assembly.contains("jl"), "{assembly}");
}

#[test]
fn emits_software_arithmetic_without_non_msp430_instructions() {
    let assembly = emit(
        r#"
            global product: u16 = 0
            global quotient: u16 = 0
            global remainder: u16 = 0
            fn main() {
                product = 7 * 6
                quotient = 43 / 5
                remainder = 43 % 5
            }
        "#,
        CpuFamily::Msp430,
    );
    assert!(assembly.contains("mul_loop"), "{assembly}");
    assert!(assembly.contains("div_loop"), "{assembly}");
    assert!(!assembly.contains("    mpy "), "{assembly}");
    assert!(!assembly.contains("    div "), "{assembly}");
}

#[test]
fn accepts_msp430x_source_options() {
    let assembly = emit("fn main() { let value: u16 = 7 }", CpuFamily::Msp430X);
    assert!(assembly.contains("; target: MSP430"), "{assembly}");
}

#[test]
fn emits_native_twenty_bit_values_on_msp430x() {
    let assembly = emit(
        "global value: u20 = 0 fn main() { value = 0xABCDEu20; value = value + 1u20 }",
        CpuFamily::Msp430X,
    );
    assert!(assembly.contains("and.a #0xFFFFF"), "{assembly}");
    assemble_subset_with_symbols_at(AssemblerCpu::Msp430X, &assembly, 0x1000)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

#[test]
fn rejects_non_msp430_targets() {
    let program = parse_program(Path::new("msp430-error.ezra"), "fn main() {}").unwrap();
    let error =
        emit_msp430_assembly_with_options(&program, options(CpuFamily::Tms9900)).unwrap_err();
    assert!(error.message.contains("MSP430"), "{error}");
}

#[test]
fn preserves_mutual_and_callback_reentry_frames() {
    let assembly = emit(
        r#"
            global result: u16 = 0
            global callback: ptr<fn(u16)u16> = &callback_step

            fn direct(value: u16) -> u16 {
                let local: u16 = value + 1
                if value == 0 { return local }
                return local + direct(value - 1)
            }

            fn even(value: u16) -> u16 {
                if value == 0 { return 1 }
                return odd(value - 1)
            }

            fn odd(value: u16) -> u16 {
                if value == 0 { return 0 }
                return even(value - 1)
            }

            fn callback_step(value: u16) -> u16 {
                let local: u16 = value + 1
                if value == 0 { return local }
                return local + callback(value - 1)
            }

            fn main() {
                let direct_value: u16 = direct(4)
                let mutual_value: u16 = even(6)
                let callback_value: u16 = callback(3)
                result = direct_value + mutual_value + callback_value
            }
            "#,
        CpuFamily::Msp430,
    );

    assert!(assembly.contains("_direct:"), "{assembly}");
    assert!(assembly.contains("_even:"), "{assembly}");
    assert!(assembly.contains("_odd:"), "{assembly}");
    assert!(assembly.contains("_callback_step:"), "{assembly}");
    assert!(assembly.contains("call r4"), "{assembly}");
    assert!(assembly.contains("0xFFFE(r13)"), "{assembly}");
}

#[test]
fn keeps_address_taken_locals_nested_calls_and_spills_on_the_stack() {
    let assembly = emit(
        r#"
            global input: u16 = 7
            global result: u16 = 0

            fn leaf(value: u16) -> u16 { return value + 1 }
            fn combine(left: u16, right: u16) -> u16 { return left + right }

            fn main() {
                let addressable: u16 = 5
                let pointer: ptr<u16> = &addressable
                let first: u16 = input
                let second: u16 = input
                let third: u16 = input
                let fourth: u16 = input
                result = combine(leaf(*pointer), leaf(first)) + second + third + fourth
            }
            "#,
        CpuFamily::Msp430,
    );

    assert!(assembly.contains("mov r1,r13"), "{assembly}");
    assert!(assembly.contains("0xFFFE(r13)"), "{assembly}");
    assert!(assembly.contains("call #_combine"), "{assembly}");
    assert_eq!(assembly.matches("call #_leaf").count(), 2, "{assembly}");
}

#[test]
fn uses_configured_stack_top_and_rejects_msp430_interrupts() {
    let program = parse_program(Path::new("msp430.ezra"), "fn main() {}").unwrap();
    let mut custom = options(CpuFamily::Msp430);
    custom.stack_top = Address24::new(0xEFFE);
    let assembly = emit_msp430_assembly_with_options(&program, custom).unwrap();
    assert!(assembly.contains("mov #0xEFFE,r1"), "{assembly}");

    let interrupt = parse_program(Path::new("msp430.ezra"), "interrupt fn main() {}").unwrap();
    let error =
        emit_msp430_assembly_with_options(&interrupt, options(CpuFamily::Msp430)).unwrap_err();
    assert!(error.message.contains("interrupt"), "{error}");
}
