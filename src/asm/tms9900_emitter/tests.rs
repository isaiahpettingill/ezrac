use std::path::Path;

use libre99_core::{
    bus::{Bus, FlatRam},
    cpu::Cpu,
};

use super::*;
use crate::{parser::parse_program, target::AssemblerCpu};

fn emit(source: &str, options: AssemblyOptions) -> String {
    let program = parse_program(Path::new("test.ezra"), source).unwrap();
    emit_tms9900_assembly_with_options(&program, options).unwrap()
}

fn test_options() -> AssemblyOptions {
    AssemblyOptions {
        cpu: CpuFamily::Tms9900,
        load_addr: crate::target::Address24::new(0x0100),
        entry_addr: crate::target::Address24::new(0x0100),
        code_base: crate::target::Address24::new(0x0100),
        stack_top: crate::target::Address24::new(0xFFFE),
        ram_base: crate::target::Address24::new(0xA000),
        ..AssemblyOptions::default()
    }
}

fn execute(assembly: &str, steps: usize) -> FlatRam {
    let image = crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, assembly, 0x0100)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..steps {
        cpu.step(&mut ram);
    }
    ram
}

fn planned_main_frame(source: &str) -> FunctionFrame {
    let options = test_options();
    let program = parse_program(Path::new("test.ezra"), source).unwrap();
    let model = SemanticModel::from_program(
        &program,
        16,
        options.ram_base.get(),
        options.rodata_base.get(),
        options.asset_base.get(),
    )
    .unwrap();
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match unwrapped_declaration(declaration) {
            Declaration::Function(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .unwrap();
    plan_function_frame(function, &model).unwrap()
}

#[test]
fn allocates_straight_scalar_locals_to_r6_through_r8() {
    let assembly = emit(
        r#"
                global input: u8 = 1
                global result: u8 = 0
                fn main() {
                    let first: u8 = input
                    let second: u8 = input
                    let third: u8 = input
                    first += second
                    result = first + third
                }
            "#,
        test_options(),
    );

    for register in 6..=8 {
        assert!(
            assembly.contains(&format!("    mov r0, r{register}")),
            "{assembly}"
        );
    }
    assert!(
        assembly.contains("    mov r10, r9\n    clr r0"),
        "local register allocation should not adjust the frame\n{assembly}"
    );
    assert_eq!(execute(&assembly, 200).read_byte(0xA001), 3);
}

#[test]
fn spills_overlapping_excess_locals_to_one_word_of_frame_storage() {
    let assembly = emit(
        r#"
                global input: u16 = 1
                global result: u16 = 0
                fn main() {
                    let first: u16 = input
                    let second: u16 = input
                    let third: u16 = input
                    let fourth: u16 = input
                    first += second
                    third += fourth
                    result = first + third
                }
            "#,
        test_options(),
    );

    assert!(
        assembly.contains("    mov r10, r9\n    ai r10, >FFFE"),
        "{assembly}"
    );
    assert!(assembly.contains("@>FFFE(r9)"), "{assembly}");
    assert_eq!(execute(&assembly, 250).read_word(0xA002), 4);
}

#[test]
fn spills_values_live_across_calls_and_inline_assembly() {
    let assembly = emit(
        r#"
                global result: u16 = 0
                fn identity(value: u16) -> u16 { return value }
                fn main() {
                    let across_call: u16 = 40
                    let called: u16 = identity(2)
                    let across_asm: u16 = called
                    asm volatile { "nop" }
                    result = across_call + across_asm
                }
            "#,
        test_options(),
    );

    let frame = planned_main_frame(
        r#"
                global result: u16 = 0
                fn identity(value: u16) -> u16 { return value }
                fn main() {
                    let across_call: u16 = 40
                    let called: u16 = identity(2)
                    let across_asm: u16 = called
                    asm volatile { "nop" }
                    result = across_call + across_asm
                }
            "#,
    );
    assert!(matches!(
        frame.locals["across_call"].location,
        BindingLocation::Frame(_)
    ));
    assert!(matches!(
        frame.locals["across_asm"].location,
        BindingLocation::Frame(_)
    ));
    assert_eq!(execute(&assembly, 300).read_word(0xA000), 42);
}

#[test]
fn keeps_address_taken_and_aggregate_locals_in_aligned_frame_slots() {
    let frame = planned_main_frame(
        r#"
                fn main() {
                    let scalar: u16 = 7
                    let pointer: ptr<u16> = &scalar
                    let values: [u16; 2] = [1, 2]
                }
            "#,
    );

    assert!(matches!(
        frame.locals["scalar"].location,
        BindingLocation::Frame(offset) if offset % 2 == 0
    ));
    assert!(matches!(
        frame.locals["values"].location,
        BindingLocation::Frame(offset) if offset % 2 == 0
    ));
    assert_eq!(frame.local_bytes % 2, 0);

    let assembly = emit(
        "global result: u16 = 0; fn main() { let value: u16 = 42; let pointer: ptr<u16> = &value; result = *pointer }",
        test_options(),
    );
    assert!(assembly.contains("    ai r0, >FFFE"), "{assembly}");
    assert_eq!(execute(&assembly, 150).read_word(0xA000), 42);
}

#[test]
fn reuses_spill_slots_for_nonoverlapping_scalar_locals() {
    let frame = planned_main_frame(
        r#"
                global sink: u16 = 0
                fn main() {
                    let first: u16 = 1
                    let second: u16 = 2
                    let third: u16 = 3
                    let first_spill: u16 = 4
                    sink = first + second + third + first_spill
                    let fifth: u16 = 5
                    let sixth: u16 = 6
                    let seventh: u16 = 7
                    let second_spill: u16 = 8
                    sink = fifth + sixth + seventh + second_spill
                }
            "#,
    );

    assert_eq!(frame.local_bytes, 2);
    let BindingLocation::Frame(first_offset) = frame.locals["first_spill"].location else {
        panic!("first excess local should spill");
    };
    let BindingLocation::Frame(second_offset) = frame.locals["second_spill"].location else {
        panic!("second excess local should spill");
    };
    assert_eq!(first_offset, second_offset);
}

#[test]
fn emits_and_executes_scalar_source_on_libre99() {
    let assembly = emit(
        "global result: u16 = 0; fn main() { let count: u16 = 41; result = count + 1 }",
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..100 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0xA000), 42);
}

#[test]
fn emits_native_constant_shifts_and_tbir_power_of_two_multiply() {
    let assembly = emit(
        r#"
                global input: u16 = 7
                global left: u16 = 0
                global logical: u16 = 0
                global arithmetic: i16 = 0
                global product: u16 = 0
                fn main() {
                    left = input << 3
                    logical = 0x8000 >> 4
                    arithmetic = cast<i16>(0x8000) >> 4
                    product = input * 8
                }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );

    assert_eq!(assembly.matches("    sla r1, 3").count(), 2, "{assembly}");
    assert!(assembly.contains("    srl r1, 4"), "{assembly}");
    assert!(assembly.contains("    sra r1, 4"), "{assembly}");
    assert!(!assembly.contains("    mpy "), "{assembly}");

    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..250 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0xA002), 56);
    assert_eq!(ram.read_word(0xA004), 0x0800);
    assert_eq!(ram.read_word(0xA006), 0xF800);
    assert_eq!(ram.read_word(0xA008), 56);
}

#[test]
fn executes_safe_variable_and_byte_shifts_on_libre99() {
    let assembly = emit(
        r#"
                global left: u16 = 0
                global logical: u16 = 0
                global arithmetic: i16 = 0
                global wide_count: u16 = 16
                global signed_byte: i8 = 0
                fn variable_left(value: u16, count: u16) -> u16 { return value << count }
                fn variable_right(value: u16, count: u16) -> u16 { return value >> count }
                fn variable_signed(value: i16, count: i16) -> i16 { return value >> count }
                fn main() {
                    left = variable_left(3, 4)
                    logical = variable_right(0x8000, wide_count)
                    arithmetic = variable_signed(cast<i16>(0x8000), wide_count)
                    signed_byte = cast<i8>(0x80) >> 3
                }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            stack_top: crate::target::Address24::new(0xFFFE),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );

    assert!(assembly.contains("    sla r1, 0"), "{assembly}");
    assert!(assembly.contains("    srl r1, 0"), "{assembly}");
    assert!(assembly.contains("    sra r1, 0"), "{assembly}");
    assert!(assembly.contains("shift_overflow"), "{assembly}");

    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..700 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0xA000), 48);
    assert_eq!(ram.read_word(0xA002), 0);
    assert_eq!(ram.read_word(0xA004), 0xFFFF);
    assert_eq!(ram.read_byte(0xA008), 0xF0);
}

#[test]
fn emits_and_executes_unsigned_multiply_on_libre99() {
    let assembly = emit(
        "global result: u16 = 0; fn main() { result = 123 * 456 }",
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..100 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0xA000), 123 * 456);
}

#[test]
fn emits_and_executes_signed_multiply_on_libre99() {
    let assembly = emit(
        "global result: i16 = 0; fn main() { result = -123 * 456 }",
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..100 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0xA000), (123u16 * 456).wrapping_neg());
}

#[test]
fn preserves_stack_frames_for_recursive_calls() {
    let assembly = emit(
        r#"
                global recursive_result: u16 = 0

                fn factorial(value: u16) -> u16 {
                    if value == 1 { return 1 }
                    return value * factorial(value - 1)
                }

                fn main() {
                    recursive_result = factorial(6)
                }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            stack_top: crate::target::Address24::new(0xFFFE),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..2000 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0xA000), 720);
}

#[test]
fn emits_and_executes_byte_pointer_access_on_libre99() {
    let assembly = emit(
        r#"
                global result: u8 = 0
                fn main() {
                    let pointer: ptr<u8> = cast<ptr<u8>>(0x9001)
                    *pointer = 0x5A
                    result = *pointer
                }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..50 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_byte(0x9001), 0x5A);
    assert_eq!(ram.read_byte(0xA000), 0x5A);
}

#[test]
fn emits_and_executes_divide_and_remainder_on_libre99() {
    let assembly = emit(
        r#"
                global quotient: u16 = 0
                global remainder: u16 = 0
                global signed_quotient: i16 = 0
                global signed_remainder: i16 = 0
                fn main() {
                    quotient = 1000 / 37
                    remainder = 1000 % 37
                    signed_quotient = -1000 / 37
                    signed_remainder = -1000 % 37
                }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..200 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0xA000), 1000 / 37);
    assert_eq!(ram.read_word(0xA002), 1000 % 37);
    assert_eq!(ram.read_word(0xA004), (-1000i16 / 37) as u16);
    assert_eq!(ram.read_word(0xA006), (-1000i16 % 37) as u16);
}

#[test]
fn passes_naked_wrapper_arguments_in_registers() {
    let assembly = emit(
        r#"
                naked fn capture(first: u8, second: u8) {
                    asm volatile {
                        "mov r0, @>9000"
                        "mov r1, @>9002"
                        "b *r11"
                    }
                }
                fn main() { capture(0x12, 0x34) }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..40 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0x9000), 0x0012);
    assert_eq!(ram.read_word(0x9002), 0x0034);
}

#[test]
fn explicit_inline_arguments_execute_once_left_to_right_and_keep_nested_helpers_reachable() {
    let assembly = emit(
        r#"
                global trace: u16 = 0
                global result: u16 = 0
                global guarded_calls: u16 = 0
                fn first() -> u16 { trace = trace * 10 + 1; return 3 }
                fn second() -> u16 { trace = trace * 10 + 2; return 4 }
                fn add_one(value: u16) -> u16 { return value + 1 }
                inline fn nested(value: u16) -> u16 { return add_one(value) }
                inline fn pair(left: u16, right: u16) -> u16 {
                    return nested(left) * 10 + right
                }
                inline fn guarded(value: bool) -> bool {
                    guarded_calls += 1
                    return value
                }
                fn main() {
                    result = pair(first(), second())
                    let flag: bool = false
                    let skipped: bool = flag && guarded(true)
                }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            stack_top: crate::target::Address24::new(0xFFFE),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );

    assert!(!assembly.contains("_nested:"), "{assembly}");
    assert!(!assembly.contains("_pair:"), "{assembly}");
    assert!(assembly.contains("_add_one:"), "{assembly}");
    assert!(!assembly.contains("_guarded:"), "{assembly}");
    assert!(!assembly.contains("    bl @_guarded"), "{assembly}");

    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..1_500 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0xA000), 12);
    assert_eq!(ram.read_word(0xA002), 44);
    assert_eq!(ram.read_word(0xA004), 0);
}

#[test]
fn omits_unreachable_functions_and_inlines_compact_wrappers() {
    let assembly = emit(
        r#"
                naked fn unused_sdk_wrapper() {
                    asm volatile { "b *r11" }
                }
                fn used() -> u16 { return 7 }
                fn sink() {}
                fn automatic_wrapper() { sink() }
                @inline fn explicit_wrapper() { sink(); sink() }
                fn retained_wrapper() { sink(); sink() }
                @inline fn recursive_wrapper() { recursive_wrapper() }
                fn main() {
                    let value: u16 = used()
                    automatic_wrapper()
                    explicit_wrapper()
                    retained_wrapper()
                    recursive_wrapper()
                }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );

    assert!(assembly.contains("_used:"), "{assembly}");
    assert!(!assembly.contains("_unused_sdk_wrapper:"), "{assembly}");
    assert!(!assembly.contains("_sink:"), "{assembly}");
    assert!(!assembly.contains("_automatic_wrapper:"), "{assembly}");
    assert!(!assembly.contains("_explicit_wrapper:"), "{assembly}");
    assert!(assembly.contains("_retained_wrapper:"), "{assembly}");
    assert!(assembly.contains("    bl @_retained_wrapper"), "{assembly}");
    assert!(assembly.contains("_recursive_wrapper:"), "{assembly}");
    assert!(
        assembly.contains("    bl @_recursive_wrapper"),
        "{assembly}"
    );
}

#[test]
fn executes_strings_nested_arguments_and_restores_stack_frames() {
    let assembly = emit(
        r#"
                global first_byte: u16 = 0
                global nested_result: u16 = 0
                fn first(text: ptr<u8>) -> u8 { return *text }
                fn pair(left: u16, right: u16) -> u16 { return left * 100 + right }
                fn main() {
                    first_byte = first("Z")
                    nested_result = pair(1, pair(2, 3))
                }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            stack_top: crate::target::Address24::new(0xFFFE),
            ram_base: crate::target::Address24::new(0xA000),
            rodata_base: crate::target::Address24::new(0x8000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap();
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..2000 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0xA000), u16::from(b'Z'));
    assert_eq!(ram.read_word(0xA002), 303);
    assert_eq!(ram.read_word(0x8312), 0);
    assert_eq!(ram.read_word(0x8314), 0xFFFE);
}

#[test]
fn bare_tms_bob_preserves_outer_operands_in_nested_expressions() {
    let assembly = emit(
        r#"
                global bob: u16 = 7
                global result: u16 = 0
                fn main() { result = (bob + 1) * (bob + 2) }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            stack_top: crate::target::Address24::new(0xFFFE),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..300 {
        cpu.step(&mut ram);
    }
    assert_eq!(ram.read_word(0xA002), 72);
}

#[test]
fn uses_operand_signedness_for_relational_boundaries() {
    let assembly = emit(
        r#"
                global flags: u16 = 0
                fn main() {
                    if cast<u16>(0xFFFF) > 0x7FFF { flags += 1 }
                    if cast<u16>(0x8000) >= 0x8000 { flags += 2 }
                    if cast<i16>(0xFFFF) < 0 { flags += 4 }
                    if cast<i16>(0x8000) <= -1 { flags += 8 }
                }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            stack_top: crate::target::Address24::new(0xFFFE),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    assert!(assembly.contains("    jh "), "{assembly}");
    assert!(assembly.contains("    jhe "), "{assembly}");
    assert!(assembly.contains("    jlt "), "{assembly}");
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..700 {
        cpu.step(&mut ram);
    }
    assert_eq!(ram.read_word(0xA000), 15);
}

#[test]
fn logical_operators_short_circuit_side_effects() {
    let assembly = emit(
        r#"
                global calls: u16 = 0
                fn mark() -> bool { calls += 1; return true }
                fn main() {
                    let first: bool = false && mark()
                    let second: bool = true || mark()
                    let third: bool = true && mark()
                    let fourth: bool = false || mark()
                }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x0100),
            entry_addr: crate::target::Address24::new(0x0100),
            code_base: crate::target::Address24::new(0x0100),
            stack_top: crate::target::Address24::new(0xFFFE),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..700 {
        cpu.step(&mut ram);
    }
    assert_eq!(ram.read_word(0xA000), 2);
}

#[test]
fn ti_embeds_are_rom_bytes_with_linked_pointer_labels() {
    let assembly = emit(
        r#"
                embed blob: bytes = bytes [0xCA, 0xFE, 0x42]
                global first: u8 = 0
                fn main() { first = *blob.ptr }
            "#,
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x6000),
            entry_addr: crate::target::Address24::new(0x6000),
            code_base: crate::target::Address24::new(0x6000),
            ram_base: crate::target::Address24::new(0xA000),
            rodata_base: crate::target::Address24::new(0x6000),
            asset_base: crate::target::Address24::new(0x6000),
            ..AssemblyOptions::default()
        },
    );
    assert!(assembly.contains("section .assets\n"), "{assembly}");
    assert!(assembly.contains("__ezra_embed_blob:\n"), "{assembly}");
    assert!(assembly.contains("    db >CA, >FE, >42"), "{assembly}");
    assert!(
        assembly.contains("    li r0, __ezra_embed_blob"),
        "{assembly}"
    );
    assert!(!assembly.contains("    movb r0, @>6000"), "{assembly}");
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x6000)
            .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let address = image
        .symbols
        .iter()
        .find(|symbol| symbol.name == "__ezra_embed_blob")
        .expect("embed label")
        .addr;
    let offset = usize::try_from(address - 0x6000).unwrap();
    assert!(address > 0x601A);
    assert_eq!(&image.bytes[offset..offset + 3], &[0xCA, 0xFE, 0x42]);
}

#[test]
fn emits_a_bootable_ti99_cartridge_header() {
    let assembly = emit(
        "fn main() {}",
        AssemblyOptions {
            cpu: CpuFamily::Tms9900,
            load_addr: crate::target::Address24::new(0x6000),
            entry_addr: crate::target::Address24::new(0x6000),
            code_base: crate::target::Address24::new(0x6000),
            ram_base: crate::target::Address24::new(0xA000),
            ..AssemblyOptions::default()
        },
    );
    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x6000)
            .unwrap();

    assert_eq!(
        &image.bytes[..16],
        &[0xAA, 1, 1, 0, 0, 0, 0x60, 0x10, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        &image.bytes[16..26],
        &[0, 0, 0x60, 0x1A, 4, b'E', b'Z', b'R', b'A', 0]
    );
}

#[test]
fn lowers_catalog_bit_integer_and_memory_intrinsics() {
    let assembly = emit(
        r#"
            fn main() {
                let rotated: u16 = bits.rotate_left(0x1234u16, 4u8)
                let product: u16 = int.widening_mul(0x12u8, 3u8)
                let value: u16 = mem.load_le16(cast<ptr<u8>>(0x9000))
                mem.fill(cast<ptr<u8>>(0x9000), 0x55u8, 2u24)
            }
        "#,
        test_options(),
    );
    assert!(assembly.contains("intrinsic_rotate_loop"), "{assembly}");
    assert!(assembly.contains("    mpy "), "{assembly}");
    assert!(assembly.contains("    movb *r3"), "{assembly}");
    assert!(assembly.contains("intrinsic_fill_loop"), "{assembly}");
}

#[test]
fn lowers_catalog_paired_division_and_rejects_volatile_wide_access() {
    let assembly = emit(
        r#"
            global quotient: u16 = 0
            global remainder: u16 = 0
            fn main() {
                let q: u16, r: u16 = int.divmod(1000u16, 37u16)
                quotient = q
                remainder = r
            }
        "#,
        test_options(),
    );
    assert!(assembly.contains("    div "), "{assembly}");
    assert!(assembly.contains("    mov r3, r2"), "{assembly}");

    let program = parse_program(
        Path::new("test.ezra"),
        "volatile mmio DEVICE: ptr<u8> = 0x9000 fn main() { let value: u16 = mem.load_be16(DEVICE) }",
    )
    .unwrap();
    let error = emit_tms9900_assembly_with_options(&program, test_options()).unwrap_err();
    assert!(error.message.contains("volatile"), "{}", error.message);
}

#[test]
fn rejects_catalog_24_bit_scalar_results() {
    let program = parse_program(
        Path::new("test.ezra"),
        "fn main() { let value: u24 = mem.load_le24(cast<ptr<u8>>(0x9000)) }",
    )
    .unwrap();
    let error = emit_tms9900_assembly_with_options(&program, test_options()).unwrap_err();
    assert!(
        error.message.contains("must fit in 16 bits"),
        "{}",
        error.message
    );
}

#[test]
fn preserves_two_results_through_user_function_calls() {
    let assembly = emit(
        r#"
            global result: u16 = 0
            fn pair(seed: u16) -> u16, u8 {
                return seed + 1, cast<u8>(seed + 3)
            }
            fn forward(seed: u16) -> u16, u8 {
                return pair(seed)
            }
            fn main() {
                let first: u16, second: u8 = forward(40)
                result = first + cast<u16>(second)
            }
            "#,
        test_options(),
    );
    assert!(assembly.contains("    mov r0, r2"), "{assembly}");
    assert!(assembly.contains("    mov r2, r1"), "{assembly}");

    let image =
        crate::vm::assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0100)
            .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let mut ram = FlatRam::new();
    ram.load(0x0100, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0100);
    for _ in 0..500 {
        cpu.step(&mut ram);
    }

    assert_eq!(ram.read_word(0xA000), 84, "{assembly}");
}
