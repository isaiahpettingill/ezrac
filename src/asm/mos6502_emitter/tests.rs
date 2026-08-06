use std::path::Path;

use crate::{asm::AssemblyOptions, parser::parse_program, target::CpuFamily};
use mos6502::{cpu::CPU, instruction::Nmos6502, memory::Bus, registers::StackPointer};

use super::*;

fn emit(source: &str) -> String {
    let program = parse_program(Path::new("test.ezra"), source).unwrap();
    emit_mos6502_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::Mos6502,
            load_addr: crate::target::Address24::new(0x0200),
            entry_addr: crate::target::Address24::new(0x0200),
            code_base: crate::target::Address24::new(0x0200),
            stack_top: crate::target::Address24::new(0x01FF),
            ram_base: crate::target::Address24::new(0xA000),
            rodata_base: crate::target::Address24::new(0x8000),
            asset_base: crate::target::Address24::new(0xC000),
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        },
    )
    .unwrap()
}

struct TestBus {
    bytes: Box<[u8; 0x1_0000]>,
}

impl TestBus {
    fn new() -> Self {
        Self {
            bytes: Box::new([0; 0x1_0000]),
        }
    }

    fn byte(&self, address: u16) -> u8 {
        self.bytes[usize::from(address)]
    }

    fn set_byte(&mut self, address: u16, value: u8) {
        self.bytes[usize::from(address)] = value;
    }
}

impl Bus for TestBus {
    fn get_byte(&mut self, address: u16) -> u8 {
        self.bytes[usize::from(address)]
    }

    fn set_byte(&mut self, address: u16, value: u8) {
        TestBus::set_byte(self, address, value);
    }
}

fn run_with_setup(
    source: &str,
    instruction_budget: usize,
    setup: impl FnOnce(&mut TestBus),
) -> (TestBus, String, usize) {
    let assembly = emit(source);
    let assembled = crate::vm::assemble_subset_with_symbols_at(
        crate::target::AssemblerCpu::Mos6502,
        &assembly,
        0x0200,
    )
    .unwrap();
    let exit = u16::try_from(
        assembled
            .symbols
            .iter()
            .find(|symbol| symbol.name == "__ezra_exit")
            .expect("emitter exit symbol")
            .addr,
    )
    .unwrap();
    let image_size = assembled.bytes.len();
    let mut bus = TestBus::new();
    for (offset, byte) in assembled.bytes.iter().copied().enumerate() {
        bus.set_byte(0x0200 + offset as u16, byte);
    }
    setup(&mut bus);
    let mut cpu = CPU::new(bus, Nmos6502);
    cpu.registers.program_counter = 0x0200;
    cpu.registers.stack_pointer = StackPointer(0xFF);
    for _ in 0..instruction_budget {
        if cpu.registers.program_counter == exit {
            return (cpu.memory, assembly, image_size);
        }
        assert!(
            cpu.single_step(),
            "6502 stopped at ${:04X}\n{assembly}",
            cpu.registers.program_counter
        );
    }
    panic!(
        "6502 execution exceeded {instruction_budget} instructions at ${:04X}\n{assembly}",
        cpu.registers.program_counter
    );
}

fn run(source: &str, instruction_budget: usize) -> TestBus {
    run_with_setup(source, instruction_budget, |_| {}).0
}

#[test]
fn overlapping_static_locals_keep_distinct_runtime_values() {
    let bus = run(
        "global result: u8 = 0; fn main() { let first: u8 = 1; let second: u8 = 2; result = first + second }",
        1_000,
    );
    assert_eq!(bus.byte(0xA000), 3);
}

#[test]
fn binary_search_uses_the_optimized_6502_slice_and_executes_edge_cases() {
    let source = r#"
            volatile mmio output: ptr<u8> = 0x2401
            volatile mmio search: ptr<u8> = 0x23FF
            volatile mmio input: ptr<u8> = 0x2301
            volatile mmio input_length: ptr<u8> = 0x2300

            fn main() {
                let target: u8 = *search
                let start: u8 = 0
                let length: u8 = *input_length
                loop {
                    if length == 0 {
                        *output = 0
                        return
                    }
                    let offset: u8 = length / 2
                    let mid: u8 = start + offset
                    let value: u8 = *(input + mid)
                    if value == target {
                        *output = 1
                        return
                    }
                    if value < target {
                        start = mid + 1
                        length = length - offset - 1
                    } else {
                        length = offset
                    }
                }
            }
        "#;

    fn search(source: &str, target: u8, values: &[u8]) -> (u8, String, usize) {
        let (bus, assembly, size) = run_with_setup(source, 10_000, |bus| {
            bus.set_byte(0x23FF, target);
            bus.set_byte(0x2300, values.len() as u8);
            for (index, value) in values.iter().copied().enumerate() {
                bus.set_byte(0x2301 + index as u16, value);
            }
        });
        (bus.byte(0x2401), assembly, size)
    }

    let (found, assembly, size) = search(source, 7, &[1, 3, 5, 7, 9]);
    assert_eq!(found, 1);
    assert_eq!(search(source, 4, &[1, 3, 5, 7, 9]).0, 0);
    assert_eq!(search(source, 9, &[9]).0, 1);
    assert_eq!(search(source, 9, &[]).0, 0);
    for instruction in ["lda $23FF", "lda $2300", "lda $2301,x", "sta $2401", "lsr "] {
        assert!(
            assembly.contains(instruction),
            "missing {instruction}\n{assembly}"
        );
    }
    for forbidden in ["div_loop", "compare_true", "($F0),y"] {
        assert!(
            !assembly.contains(forbidden),
            "found {forbidden}\n{assembly}"
        );
    }
    assert!(
        size <= 128,
        "binary search image is {size} bytes\n{assembly}"
    );
}

#[test]
fn explicit_inline_arguments_execute_once_left_to_right_with_typed_temps_and_nested_helpers() {
    let bus = run(
        r#"
                global trace: u8 = 0
                global result: u8 = 0
                global guarded_calls: u8 = 0
                fn first() -> u8 { trace = trace * 10 + 1; return 3 }
                fn second() -> u8 { trace = trace * 10 + 2; return 4 }
                fn add_one(value: u8) -> u8 { return value + 1 }
                inline fn nested(value: u8) -> u8 { return add_one(value) }
                inline fn pair(left: u8, right: u8) -> u8 {
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
        2_000,
    );

    assert_eq!(bus.byte(0xA000), 12);
    assert_eq!(bus.byte(0xA001), 44);
    assert_eq!(bus.byte(0xA002), 0);
}

#[test]
fn omits_unused_public_functions() {
    let assembly = emit(
        r#"
                pub fn unused_sdk_helper() { }
                fn used_helper() -> u8 { return 1 }
                fn main() { let value: u8 = used_helper() }
            "#,
    );

    assert!(assembly.contains("_used_helper:"), "{assembly}");
    assert!(!assembly.contains("_unused_sdk_helper:"), "{assembly}");
}

#[test]
fn emits_core_6502_source_constructs() {
    let assembly = emit(
        r#"
                volatile mmio SCREEN: ptr<u8> = 0xD020
                global counter: u16 = 1
                fn add(value: u16) -> u16 { return value + 2 }
                fn main() {
                    let i: u16 = add(counter)
                    *(SCREEN) = cast<u8>(counter)
                    while i < 10 {
                        counter += 1
                        i += 1
                    }
                }
            "#,
    );
    assert!(assembly.contains("; target: MOS 6502"), "{assembly}");
    assert!(assembly.contains("jsr _add"), "{assembly}");
    assert!(assembly.contains("sta $D020"), "{assembly}");
    assert!(!assembly.contains("ld hl"), "{assembly}");
    crate::vm::assemble_subset_with_symbols_at(
        crate::target::AssemblerCpu::Mos6502,
        &assembly,
        0x0200,
    )
    .unwrap();
}

#[test]
fn emitted_core_program_assembles() {
    let assembly = emit("global value: u8 = 1\nfn main() { value += 2 }");
    crate::vm::assemble_subset_with_symbols_at(
        crate::target::AssemblerCpu::Mos6502,
        &assembly,
        0x0200,
    )
    .unwrap();
}

#[test]
fn complete_language_surface_emits_and_assembles() {
    let assembly = emit(
        r#"
                const COUNT: u8 = 3
                alias Word = u16
                struct Point { x: u8 y: Word }
                volatile mmio DEVICE: ptr<u8> = 0xD000
                embed data: bytes = bytes [1, 2, 3]
                global points: [Point; COUNT] = [
                    Point { x: 1, y: 0x0203 },
                    Point { x: 4, y: 0x0506 }
                ]
                global total: u24 = 0

                fn calculate(a: u16, b: u16) -> u16 {
                    let value: u16 = (a + b) * 2
                    if value > 10 { return value / 2 }
                    return value % 3
                }

                fn factorial(n: u8) -> u8 {
                    if n <= 1 { return 1 }
                    return n * factorial(n - 1)
                }

                interrupt fn irq() { asm volatile { "nop" } }
                naked fn raw() { asm volatile { "nop" "rts" } }

                fn main() {
                    let local: [u8; 3] = [1, 2, 3]
                    let local_copy: [u8; 3] = local
                    let point: Point = Point { x: local[1], y: calculate(4, 8) }
                    let point_copy: Point = point
                    let pointer: ptr<u8> = &point.x
                    *pointer = point.x + 1
                    *(pointer + 1) = factorial(4)
                    total = cast<u24>(point.y)
                    total <<= 1
                    total ^= 0x0000FF
                    if point.x != 0 && total >= 1 {
                        *(DEVICE) = *pointer
                    }
                    let i: u8 = 0
                    while i < COUNT { i += 1 }
                    loop { break }
                    mem.memset(&local[0], 0, 3)
                    mem.memcpy(&local[0], &local_copy[0], 3)
                    asm volatile(clobber a, clobber flags) { "nop" }
                }
            "#,
    );
    crate::vm::assemble_subset_with_symbols_at(
        crate::target::AssemblerCpu::Mos6502,
        &assembly,
        0x0200,
    )
    .unwrap();
}

#[test]
fn executes_integer_arithmetic_and_signed_comparisons() {
    let memory = run(
        r#"
                volatile mmio RESULT0: ptr<u8> = 0xFF00
                volatile mmio RESULT1: ptr<u8> = 0xFF01
                volatile mmio RESULT2: ptr<u8> = 0xFF02
                volatile mmio RESULT3: ptr<u8> = 0xFF03
                fn main() {
                    let product: u16 = (7 + 5) * 3
                    *(RESULT0) = cast<u8>(product)
                    *(RESULT1) = cast<u8>(product / 5)
                    *(RESULT2) = cast<u8>(product % 5)
                    let negative: i16 = -20
                    *(RESULT3) = cast<u8>(negative < -3)
                }
            "#,
        50_000,
    );
    assert_eq!(memory.byte(0xFF00), 36);
    assert_eq!(memory.byte(0xFF01), 7);
    assert_eq!(memory.byte(0xFF02), 1);
    assert_eq!(memory.byte(0xFF03), 1);
}

#[test]
fn executes_bounded_u16_helpers_and_zero_divisors() {
    let memory = run(
        r#"
                volatile mmio RESULT0: ptr<u8> = 0xFF00
                volatile mmio RESULT1: ptr<u8> = 0xFF01
                volatile mmio RESULT2: ptr<u8> = 0xFF02
                volatile mmio RESULT3: ptr<u8> = 0xFF03
                volatile mmio RESULT4: ptr<u8> = 0xFF04
                volatile mmio RESULT5: ptr<u8> = 0xFF05
                fn main() {
                    let maximum: u16 = 0xFFFF
                    let product: u16 = maximum * maximum
                    let quotient: u16 = maximum / 1
                    let remainder: u16 = maximum % 251
                    let zero: u16 = 0
                    *(RESULT0) = cast<u8>(product)
                    *(RESULT1) = cast<u8>(quotient)
                    *(RESULT2) = cast<u8>(quotient >> 8)
                    *(RESULT3) = cast<u8>(remainder)
                    *(RESULT4) = cast<u8>(maximum / zero)
                    *(RESULT5) = cast<u8>(maximum % zero)
                }
            "#,
        30_000,
    );
    assert_eq!(memory.byte(0xFF00), 1);
    assert_eq!(memory.byte(0xFF01), 0xFF);
    assert_eq!(memory.byte(0xFF02), 0xFF);
    assert_eq!(memory.byte(0xFF03), 24);
    assert_eq!(memory.byte(0xFF04), 0);
    assert_eq!(memory.byte(0xFF05), 0);
}

#[test]
fn executes_small_constant_multiplication_with_wrapping() {
    let memory = run(
        r#"
                volatile mmio RESULT0: ptr<u8> = 0xFF00
                volatile mmio RESULT1: ptr<u8> = 0xFF01
                volatile mmio RESULT2: ptr<u8> = 0xFF02
                volatile mmio RESULT3: ptr<u8> = 0xFF03
                volatile mmio RESULT4: ptr<u8> = 0xFF04
                fn main() {
                    let byte: u8 = 250
                    let word: u16 = 0xF123
                    let compound: u8 = 51
                    compound *= 5
                    *(RESULT0) = byte * 3
                    *(RESULT1) = cast<u8>(word * 7)
                    *(RESULT2) = cast<u8>((word * 10) >> 8)
                    *(RESULT3) = cast<u8>(word * 8)
                    *(RESULT4) = compound
                }
            "#,
        2_000,
    );
    assert_eq!(memory.byte(0xFF00), 238);
    assert_eq!(memory.byte(0xFF01), 0xF5);
    assert_eq!(memory.byte(0xFF02), 0x6B);
    assert_eq!(memory.byte(0xFF03), 0x18);
    assert_eq!(memory.byte(0xFF04), 255);
}

#[test]
fn executes_immediate_bitwise_after_one_volatile_u16_read() {
    let source = r#"
            volatile mmio INPUT: ptr<u16> = 0xD000
            volatile mmio RESULT0: ptr<u8> = 0xFF00
            volatile mmio RESULT1: ptr<u8> = 0xFF01
            volatile mmio RESULT2: ptr<u8> = 0xFF02
            global calls: u8 = 0

            fn read_input() -> u16 {
                calls += 1
                return *(INPUT)
            }

            fn main() {
                *(INPUT) = 0x1234
                let masked: u16 = read_input() & 0x00FFu16
                *(RESULT0) = cast<u8>(masked)
                *(RESULT1) = cast<u8>(masked >> 8)
                *(RESULT2) = calls
            }
        "#;
    let assembly = emit(source);
    let read_input = assembly
        .split("_read_input:")
        .nth(1)
        .unwrap()
        .split("    rts")
        .next()
        .unwrap();
    assert_eq!(read_input.matches("    lda $D00").count(), 2, "{assembly}");
    assert!(!read_input.contains("($F0),y"), "{assembly}");
    assert_eq!(
        assembly.matches("    jsr _read_input").count(),
        1,
        "{assembly}"
    );

    let memory = run(source, 5_000);
    assert_eq!(memory.byte(0xFF00), 0x34);
    assert_eq!(memory.byte(0xFF01), 0x00);
    assert_eq!(memory.byte(0xFF02), 1);
}

#[test]
fn executes_signed_constant_multiplication_after_volatile_u16_load() {
    let source = r#"
            volatile mmio INPUT: ptr<u16> = 0xD000
            volatile mmio RESULT0: ptr<u8> = 0xFF00
            volatile mmio RESULT1: ptr<u8> = 0xFF01
            global calls: u8 = 0

            fn read_input() -> i16 {
                calls += 1
                return cast<i16>(*(INPUT))
            }

            fn main() {
                *(INPUT) = 3
                let product: i16 = read_input() * -7
                *(RESULT0) = cast<u8>(product)
                *(RESULT1) = calls
            }
        "#;
    let assembly = emit(source);
    assert!(assembly.contains("    jsr _read_input"), "{assembly}");
    assert!(assembly.contains("    lda $D000"), "{assembly}");
    assert!(assembly.contains("    lda $D001"), "{assembly}");
    assert!(!assembly.contains("($F0),y"), "{assembly}");

    let memory = run(source, 5_000);
    assert_eq!(memory.byte(0xFF00), 0xEB);
    assert_eq!(memory.byte(0xFF01), 1);
}

#[test]
fn executes_constant_shifts_across_bytes_and_width_boundaries() {
    let memory = run(
        r#"
                volatile mmio RESULT0: ptr<u8> = 0xFF00
                volatile mmio RESULT1: ptr<u8> = 0xFF01
                volatile mmio RESULT2: ptr<u8> = 0xFF02
                volatile mmio RESULT3: ptr<u8> = 0xFF03
                fn main() {
                    let value: u24 = 0x123456
                    *(RESULT0) = cast<u8>(value >> 12)
                    *(RESULT1) = cast<u8>((value << 12) >> 16)
                    let negative: i16 = -2
                    *(RESULT2) = cast<u8>(negative >> 16)
                    *(RESULT3) = cast<u8>(value << 24)
                }
            "#,
        2_000,
    );
    assert_eq!(memory.byte(0xFF00), 0x23);
    assert_eq!(memory.byte(0xFF01), 0x45);
    assert_eq!(memory.byte(0xFF02), 0xFF);
    assert_eq!(memory.byte(0xFF03), 0);
}

#[test]
fn lowers_single_bit_masked_conditions_without_re_evaluating_the_value() {
    let assembly = emit(
        r#"
                global calls: u8 = 0
                global result: u8 = 0
                fn sample() -> u8 { calls += 1; return calls }
                fn main() {
                    if (sample() & 1) == 1 { result += 1 }
                    if (sample() & 2) != 0 { result += 2 }
                    while (sample() & 4) == 4 { result += 4 }
                }
            "#,
    );
    assert_eq!(assembly.matches("    bit $").count(), 3, "{assembly}");
    assert!(!assembly.contains("    and $"), "{assembly}");

    let memory = run(
        r#"
                volatile mmio RESULT0: ptr<u8> = 0xFF00
                volatile mmio RESULT1: ptr<u8> = 0xFF01
                global calls: u8 = 0
                global result: u8 = 0
                fn sample() -> u8 { calls += 1; return calls }
                fn main() {
                    if (sample() & 1) == 1 { result += 1 }
                    if (sample() & 2) != 0 { result += 2 }
                    while (sample() & 4) == 4 { result += 4 }
                    *(RESULT0) = calls
                    *(RESULT1) = result
                }
            "#,
        20_000,
    );
    assert_eq!(memory.byte(0xFF00), 3);
    assert_eq!(memory.byte(0xFF01), 3);
}

#[test]
fn executes_calls_and_recursion() {
    let memory = run(
        r#"
                volatile mmio RESULT: ptr<u8> = 0xFF00
                fn factorial(n: u8) -> u8 {
                    if n <= 1 { return 1 }
                    return n * factorial(n - 1)
                }
                fn main() { *(RESULT) = factorial(5) }
            "#,
        100_000,
    );
    assert_eq!(memory.byte(0xFF00), 120);
}

#[test]
fn executes_mutual_recursion_with_preserved_static_locals() {
    let memory = run(
        r#"
                volatile mmio RESULT: ptr<u8> = 0xFF00
                fn left(value: u8) -> u8 {
                    let saved: u8 = value
                    if value == 0 { return saved }
                    let child: u8 = right(value - 1)
                    return saved + child
                }
                fn right(value: u8) -> u8 {
                    let saved: u8 = value
                    if value == 0 { return saved }
                    let child: u8 = left(value - 1)
                    return saved + child
                }
                fn main() { *(RESULT) = left(4) }
            "#,
        100_000,
    );
    assert_eq!(memory.byte(0xFF00), 10);
}

#[test]
fn executes_nested_calls_without_clobbering_outer_arguments() {
    let memory = run(
        r#"
                volatile mmio RESULT: ptr<u8> = 0xFF00
                fn sum(left: u8, right: u8) -> u8 { return left + right }
                fn main() { *(RESULT) = sum(5, sum(2, 3)) }
            "#,
        20_000,
    );
    assert_eq!(memory.byte(0xFF00), 10);
}

#[test]
fn executes_aggregates_pointers_and_memory_builtins() {
    let memory = run(
        r#"
                struct Pair { left: u8 right: u16 }
                volatile mmio RESULT0: ptr<u8> = 0xFF00
                volatile mmio RESULT1: ptr<u8> = 0xFF01
                fn main() {
                    let bytes: [u8; 4] = [1, 2, 3, 4]
                    let copy: [u8; 4] = bytes
                    let pair: Pair = Pair { left: copy[2], right: 0x1234 }
                    let pair_copy: Pair = pair
                    let pointer: ptr<u8> = &bytes[0]
                    *(pointer + 1) = pair_copy.left + 6
                    mem.memset(&copy[0], 0, 4)
                    mem.memcpy(&copy[0], &bytes[0], 4)
                    *(RESULT0) = copy[1]
                    *(RESULT1) = cast<u8>(pair_copy.right)
                }
            "#,
        100_000,
    );
    assert_eq!(memory.byte(0xFF00), 9);
    assert_eq!(memory.byte(0xFF01), 0x34);
}

#[test]
fn executes_short_circuit_boolean_expressions() {
    let memory = run(
        r#"
                volatile mmio RESULT: ptr<u8> = 0xFF00
                global calls: u8 = 0
                fn called() -> bool { calls += 1 return true }
                fn main() {
                    let first: bool = false && called()
                    let second: bool = true || called()
                    *(RESULT) = calls
                }
            "#,
        20_000,
    );
    assert_eq!(memory.byte(0xFF00), 0);
}

#[test]
fn executes_wide_and_signed_arithmetic() {
    let memory = run(
        r#"
                volatile mmio RESULT0: ptr<u8> = 0xFF00
                volatile mmio RESULT1: ptr<u8> = 0xFF01
                volatile mmio RESULT2: ptr<u8> = 0xFF02
                volatile mmio RESULT3: ptr<u8> = 0xFF03
                volatile mmio RESULT4: ptr<u8> = 0xFF04
                fn main() {
                    let wide: u24 = 0x010203 + 0x000102
                    *(RESULT0) = cast<u8>(wide)
                    *(RESULT1) = cast<u8>(wide >> 8)
                    *(RESULT2) = cast<u8>(wide >> 16)
                    let dividend: i16 = -20
                    let divisor: i16 = 3
                    *(RESULT3) = cast<u8>(dividend / divisor)
                    *(RESULT4) = cast<u8>(dividend % divisor)
                }
            "#,
        100_000,
    );
    assert_eq!(memory.byte(0xFF00), 0x05);
    assert_eq!(memory.byte(0xFF01), 0x03);
    assert_eq!(memory.byte(0xFF02), 0x01);
    assert_eq!(memory.byte(0xFF03), 0xFA);
    assert_eq!(memory.byte(0xFF04), 0xFE);
}

#[test]
fn executes_wrapping_scaled_indexes_above_255() {
    let source = r#"
            volatile mmio INDEX: ptr<u16> = 0xFF10
            volatile mmio RESULT0: ptr<u8> = 0xFF00
            volatile mmio RESULT1: ptr<u8> = 0xFF01
            volatile mmio RESULT2: ptr<u8> = 0xFF02
            fn main() {
                let index: u16 = *INDEX
                let bytes: ptr<u8> = 0xFFFE
                let words: ptr<u16> = 0xFE80
                let word: u16 = words[index]
                *(RESULT0) = bytes[index]
                *(RESULT1) = cast<u8>(word)
                *(RESULT2) = cast<u8>(word >> 8)
            }
        "#;
    let (memory, assembly, _) = run_with_setup(source, 20_000, |bus| {
        bus.set_byte(0xFF10, 0x01);
        bus.set_byte(0xFF11, 0x01);
        bus.set_byte(0x00FF, 0xA1);
        bus.set_byte(0x0082, 0xC3);
        bus.set_byte(0x0083, 0xB2);
    });

    assert_eq!(memory.byte(0xFF00), 0xA1);
    assert_eq!(memory.byte(0xFF01), 0xC3);
    assert_eq!(memory.byte(0xFF02), 0xB2);
    assert_eq!(assembly.matches("    lda $FF10").count(), 1, "{assembly}");
    assert_eq!(assembly.matches("    lda $FF11").count(), 1, "{assembly}");
    assert!(!assembly.contains("index_scale"), "{assembly}");
    assert!(!assembly.contains("mul_loop"), "{assembly}");
}

#[test]
fn executes_dynamic_indexes_and_loop_control() {
    let memory = run(
        r#"
                struct Point { x: u8 y: u16 }
                volatile mmio RESULT0: ptr<u8> = 0xFF00
                volatile mmio RESULT1: ptr<u8> = 0xFF01
                fn main() {
                    let points: [Point; 2] = [
                        Point { x: 1, y: 0x0203 },
                        Point { x: 4, y: 0x0506 }
                    ]
                    let index: u8 = 1
                    let i: u8 = 0
                    let sum: u8 = 0
                    while i < 6 {
                        i += 1
                        if i == 2 { continue }
                        if i == 5 { break }
                        sum += i
                    }
                    mem.poke8(RESULT0, points[index].x)
                    mem.poke8(RESULT1, sum)
                }
            "#,
        100_000,
    );
    assert_eq!(memory.byte(0xFF00), 4);
    assert_eq!(memory.byte(0xFF01), 8);
}

#[test]
fn executes_global_embed_and_string_initialization() {
    let memory = run(
        r#"
                embed data: bytes = bytes [0x11, 0x22, 0x33]
                global values: [u8; 3] = [4, 5, 6]
                volatile mmio RESULT0: ptr<u8> = 0xFF00
                volatile mmio RESULT1: ptr<u8> = 0xFF01
                volatile mmio RESULT2: ptr<u8> = 0xFF02
                fn main() {
                    let text: ptr<u8> = "OK"
                    *(RESULT0) = values[1]
                    *(RESULT1) = *(data.ptr + 1)
                    *(RESULT2) = *text
                }
            "#,
        20_000,
    );
    assert_eq!(memory.byte(0xFF00), 5);
    assert_eq!(memory.byte(0xFF01), 0x22);
    assert_eq!(memory.byte(0xFF02), b'O');
}

#[test]
fn executes_direct_generic_two_result_user_function_returns() {
    let memory = run(
        r#"
            volatile mmio RESULT0: ptr<u8> = 0xFF00
            volatile mmio RESULT1: ptr<u8> = 0xFF01
            fn pair(value: u8) -> u8, bool { return value + 1, value == 4 }
            fn main() {
                let first: u8, second: bool = pair(4)
                *(RESULT0) = first
                *(RESULT1) = cast<u8>(second)
            }
        "#,
        20_000,
    );
    assert_eq!(memory.byte(0xFF00), 5);
    assert_eq!(memory.byte(0xFF01), 1);
}

#[test]
fn executes_forwarded_and_recursive_two_result_returns() {
    let (memory, assembly, _) = run_with_setup(
        r#"
            volatile mmio RESULT0: ptr<u8> = 0xFF00
            volatile mmio RESULT1: ptr<u8> = 0xFF01
            volatile mmio RESULT2: ptr<u8> = 0xFF02
            volatile mmio RESULT3: ptr<u8> = 0xFF03
            fn base(value: u8) -> u8, bool { return value + 1, value == 6 }
            fn forward(value: u8) -> u8, bool { return base(value) }
            fn recurse(value: u8) -> u8, bool {
                if value == 0 { return 1, true }
                let first: u8, second: bool = recurse(value - 1)
                return first + 1, second
            }
            fn main() {
                let first: u8, second: bool = forward(6)
                let recursive_first: u8, recursive_second: bool = recurse(2)
                *(RESULT0) = first
                *(RESULT1) = cast<u8>(second)
                *(RESULT2) = recursive_first
                *(RESULT3) = cast<u8>(recursive_second)
            }
        "#,
        100_000,
        |_| {},
    );
    assert_eq!(memory.byte(0xFF00), 7, "{assembly}");
    assert_eq!(memory.byte(0xFF01), 1, "{assembly}");
    assert_eq!(memory.byte(0xFF02), 3, "{assembly}");
    assert_eq!(memory.byte(0xFF03), 1, "{assembly}");
}

#[test]
fn executes_void_one_and_two_result_user_functions() {
    let memory = run(
        r#"
            volatile mmio RESULT0: ptr<u8> = 0xFF00
            volatile mmio RESULT1: ptr<u8> = 0xFF01
            volatile mmio RESULT2: ptr<u8> = 0xFF02
            volatile mmio RESULT3: ptr<u8> = 0xFF03
            global void_calls: u8 = 0

            fn zero() { void_calls += 1 }
            fn one(value: u8) -> u8 { return value + 2 }
            fn pair(value: u8) -> u8, u8 { return one(value), value + 3 }

            fn main() {
                zero()
                let scalar: u8 = one(3)
                let first: u8, second: u8 = pair(scalar)
                *(RESULT0) = void_calls
                *(RESULT1) = scalar
                *(RESULT2) = first
                *(RESULT3) = second
            }
        "#,
        20_000,
    );
    assert_eq!(memory.byte(0xFF00), 1);
    assert_eq!(memory.byte(0xFF01), 5);
    assert_eq!(memory.byte(0xFF02), 7);
    assert_eq!(memory.byte(0xFF03), 8);
}

#[test]
fn lowers_catalog_intrinsics_and_direct_two_result_bindings() {
    let assembly = emit(
        r#"
            global result: u24 = 0
            global source: [u8; 4] = [1, 2, 3, 4]
            global destination: [u8; 4] = [0, 0, 0, 0]
            fn main() {
                let rotated: u8 = bits.rotate_right(1u8, 1u8)
                let tested: bool = bits.test(rotated, 0u8)
                let updated: u8 = bits.toggle(bits.set(rotated, 2u8), 1u8)
                let quotient: u16, remainder: u16 = int.divmod(0x1234u16, 0x0011u16)
                let sum: u16, carry: bool = int.add_carry(0xFFFFu16, 1u16, false)
                let difference: u16, borrow: bool = int.sub_borrow(0u16, 1u16, false)
                let low: u16, high: u16 = int.full_mul(0x1234u16, 3u16)
                let found: ptr<u8>, present: bool = mem.find_byte(&source[0], 4u24, 3u8)
                let loaded: u24 = mem.load_be24(&source[0])
                mem.copy_nonoverlapping(&destination[0], &source[0], 4u24)
                mem.store_le24(&destination[0], loaded)
                result = cast<u24>(quotient) + cast<u24>(remainder) + cast<u24>(sum)
                    + cast<u24>(difference) + cast<u24>(low) + cast<u24>(high)
                    + cast<u24>(found) + cast<u24>(tested) + cast<u24>(carry)
                    + cast<u24>(borrow) + cast<u24>(present) + cast<u24>(updated)
            }
        "#,
    );
    assert!(assembly.contains("    bit $"), "{assembly}");
    assert!(assembly.contains("    rol "), "{assembly}");
    assert!(assembly.contains("    ror "), "{assembly}");
    assert!(assembly.contains("find_byte"), "{assembly}");
    assert!(assembly.contains("mem_copy_forward"), "{assembly}");
}

#[test]
fn rejects_volatile_memory_for_nonvolatile_intrinsics() {
    let program = parse_program(
        Path::new("mos6502-intrinsic-volatile.ezra"),
        r#"
            volatile mmio register: ptr<u8> = 0xD000
            fn main() { let value: u16 = mem.load_le16(register) }
        "#,
    )
    .unwrap();
    let error = emit_mos6502_assembly_with_options(
        &program,
        AssemblyOptions {
            cpu: CpuFamily::Mos6502,
            load_addr: crate::target::Address24::new(0x0200),
            entry_addr: crate::target::Address24::new(0x0200),
            code_base: crate::target::Address24::new(0x0200),
            stack_top: crate::target::Address24::new(0x01FF),
            ram_base: crate::target::Address24::new(0xA000),
            rodata_base: crate::target::Address24::new(0x8000),
            asset_base: crate::target::Address24::new(0xC000),
            default_sdk_symbols: false,
            ..AssemblyOptions::default()
        },
    )
    .unwrap_err();
    assert!(error.message.contains("volatile memory"), "{error}");
}
