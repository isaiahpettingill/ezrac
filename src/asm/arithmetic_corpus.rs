use std::path::Path;

use crate::{
    asm::AssemblyOptions,
    parser::parse_program,
    target::{Address24, AssemblerCpu, CpuFamily},
    vm::assemble_subset_with_symbols_at,
};

// Keep operands in globals so the target emitters, rather than TBIR constant folding,
// define every result. The corpus uses the scalar widths shared by every source backend.
const SOURCE: &str = r#"
    global umax: u8 = 0xFF
    global uthree: u8 = 3
    global uzero: u8 = 0
    global smin: i8 = -128
    global sneg: i8 = -123
    global sseven: i8 = 7
    global sminus_one: i8 = -1
    global shift_zero: u8 = 0
    global shift_width: u8 = 8
    global shift_above: u8 = 11

    global add_wrap: u8 = 0
    global mul_wrap: u8 = 0
    global signed_shift: i8 = 0
    global shift_at_width: u8 = 1
    global shift_above_width: u8 = 1
    global signed_shift_at_width: i8 = 0
    global unsigned_div: u8 = 0
    global unsigned_rem: u8 = 0
    global signed_div: i8 = 0
    global signed_rem: i8 = 0
    global overflow_div: i8 = 0
    global overflow_rem: i8 = 0
    global zero_div: u8 = 1
    global zero_rem: u8 = 1
    global mul_power_two: u8 = 0
    global mul_non_power_two: u8 = 0

    fn main() {
        add_wrap = umax + uthree
        mul_wrap = umax * uthree
        signed_shift = sneg >> uthree
        shift_at_width = umax << shift_width
        shift_above_width = umax >> shift_above
        signed_shift_at_width = sneg >> shift_width
        unsigned_div = umax / uthree
        unsigned_rem = umax % uthree
        signed_div = sneg / sseven
        signed_rem = sneg % sseven
        overflow_div = smin / sminus_one
        overflow_rem = smin % sminus_one
        zero_div = umax / uzero
        zero_rem = umax % uzero
        mul_power_two = uthree * 8
        mul_non_power_two = uthree * 10
        shift_at_width += umax << shift_zero
    }
"#;

#[cfg(feature = "dcpu")]
const DCPU_SOURCE: &str = r#"
    fn main() {
        let unsigned: u16 = 0xFFFF
        let unsigned_rhs: u16 = 3
        let signed: i16 = cast<i16>(0xCFC7)
        let signed_rhs: i16 = 7
        let result: u16 = unsigned + unsigned_rhs
        result = unsigned * unsigned_rhs
        result = cast<u16>(signed >> unsigned_rhs)
        result = unsigned << 16
        result = unsigned >> 19
        result = unsigned / unsigned_rhs
        result = unsigned % unsigned_rhs
        result = cast<u16>(signed / signed_rhs)
        result = cast<u16>(signed % signed_rhs)
        result = cast<u16>(cast<i16>(-32768) / cast<i16>(-1))
        result = cast<u16>(cast<i16>(-32768) % cast<i16>(-1))
        result = unsigned / 0
        result = unsigned % 0
        result = unsigned_rhs * 8
        result = unsigned_rhs * 10
    }
"#;

#[cfg(feature = "m68k")]
const M68K_SOURCE: &str = r#"
    global unsigned: u16 = 0xFFFF
    global unsigned_rhs: u16 = 3
    global signed: i16 = -12345
    global signed_rhs: i16 = 7
    global zero: u16 = 0
    global result: u16 = 0
    fn main() {
        result = unsigned + unsigned_rhs
        result = unsigned * unsigned_rhs
        result = cast<u16>(signed >> unsigned_rhs)
        result = unsigned << 16
        result = unsigned >> 19
        result = unsigned / unsigned_rhs
        result = unsigned % unsigned_rhs
        result = cast<u16>(signed / signed_rhs)
        result = cast<u16>(signed % signed_rhs)
        result = cast<u16>(cast<i16>(-32768) / cast<i16>(-1))
        result = cast<u16>(cast<i16>(-32768) % cast<i16>(-1))
        result = unsigned / zero
        result = unsigned % zero
        result = unsigned_rhs * 8
        result = unsigned_rhs * 10
    }
"#;

#[cfg(feature = "m6800")]
const M6800_SOURCE: &str = r#"
    global value: u8 = 0xFF
    global rhs: u8 = 3
    global signed: i8 = -123
    global add_wrap: u8 = 0
    global signed_shift: i8 = 0
    global shift_at_width: u8 = 1
    global shift_above_width: u8 = 1
    global mul_power_two: u8 = 0
    fn main() {
        add_wrap = value + rhs
        signed_shift = signed >> 3
        shift_at_width = value << 8
        shift_above_width = value >> 11
        mul_power_two = rhs * 8
    }
"#;

fn options(cpu: CpuFamily) -> AssemblyOptions {
    AssemblyOptions {
        cpu,
        load_addr: Address24::new(0x0200),
        entry_addr: Address24::new(0x0200),
        code_base: Address24::new(0x0200),
        stack_top: Address24::new(0xFFFE),
        ram_base: Address24::new(0xA000),
        rodata_base: Address24::new(0x8000),
        asset_base: Address24::new(0xC000),
        default_sdk_symbols: false,
        ..AssemblyOptions::default()
    }
}

fn parse(source: &str) -> crate::ast::Program {
    parse_program(Path::new("differential-arithmetic.ezra"), source).unwrap()
}

fn program() -> crate::ast::Program {
    parse(SOURCE)
}

fn validate(cpu: AssemblerCpu, assembly: &str) {
    assemble_subset_with_symbols_at(cpu, assembly, 0x0200)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
}

const RESULT_NAMES: [&str; 16] = [
    "add_wrap",
    "mul_wrap",
    "signed_shift",
    "shift_at_width",
    "shift_above_width",
    "signed_shift_at_width",
    "unsigned_div",
    "unsigned_rem",
    "signed_div",
    "signed_rem",
    "overflow_div",
    "overflow_rem",
    "zero_div",
    "zero_rem",
    "mul_power_two",
    "mul_non_power_two",
];

const EXPECTED_RESULTS: [u8; 16] = [
    2, 253, 0xF0, 0xFF, 0, 0xFF, 85, 0, 0xEF, 0xFC, 0x80, 0, 0, 0, 24, 30,
];

#[cfg(feature = "test-runner")]
fn run_image(cpu_family: CpuFamily, assembly: &str, instruction_budget: u64) -> crate::vm::TestRun {
    use crate::vm::{TestImage, TestRunOptions, TestRunner, assemble_subset_at};

    let bytes = assemble_subset_at(cpu_family, assembly, 0x0200).unwrap();
    TestRunner::default()
        .run(
            &TestImage {
                cpu_family,
                base_addr: 0x0200,
                bytes,
            },
            &TestRunOptions {
                instruction_budget,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: if cpu_family == CpuFamily::Mos6502 {
                    0x01FF
                } else {
                    0xFFFE
                },
            },
        )
        .unwrap()
}

fn debug_writes(instruction: impl Fn(u16) -> String) -> String {
    (0..RESULT_NAMES.len())
        .map(|offset| instruction(0xA00A + offset as u16))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ez80_differential_arithmetic_corpus_assembles() {
    let assembly =
        super::emit_ez80_assembly_with_options(&program(), options(CpuFamily::Ez80)).unwrap();
    validate(AssemblerCpu::Ez80, &assembly);

    #[cfg(feature = "test-runner")]
    {
        let writes =
            debug_writes(|address| format!("    ld a, ({address:04X}h)\n    out (0Ch), a"));
        let instrumented = assembly.replace(
            "__ezra_exit:\n    jp __ezra_exit\n",
            &format!("__ezra_exit:\n{writes}\n    ld a, 1\n    out (0Eh), a\n    jp __ezra_exit\n"),
        );
        assert_ne!(instrumented, assembly, "missing eZ80 exit loop\n{assembly}");
        let run = run_image(CpuFamily::Ez80, &instrumented, 100_000);
        assert!(run.halted, "{run:?}\n{instrumented}");
        assert_eq!(run.debug_output, EXPECTED_RESULTS, "{instrumented}");
    }
}

#[cfg(feature = "lr35902")]
#[test]
fn lr35902_differential_arithmetic_corpus_assembles() {
    let assembly =
        super::emit_lr35902_assembly_with_options(&program(), options(CpuFamily::Lr35902)).unwrap();
    validate(AssemblerCpu::Lr35902, &assembly);
}

#[cfg(feature = "mos6502")]
#[test]
fn mos6502_differential_arithmetic_corpus_assembles() {
    let assembly =
        super::emit_mos6502_assembly_with_options(&program(), options(CpuFamily::Mos6502)).unwrap();
    validate(AssemblerCpu::Mos6502, &assembly);

    #[cfg(all(feature = "test-runner", feature = "mos6502-emulator"))]
    {
        let writes = debug_writes(|address| format!("    lda ${address:04X}\n    sta $FF0C"));
        let instrumented = assembly.replace(
            "__ezra_exit:\n    jmp __ezra_exit\n",
            &format!("__ezra_exit:\n{writes}\n    lda #$01\n    sta $FF0E\n    jmp __ezra_exit\n"),
        );
        assert_ne!(instrumented, assembly, "missing 6502 exit loop\n{assembly}");
        let run = run_image(CpuFamily::Mos6502, &instrumented, 100_000);
        assert!(run.halted, "{run:?}\n{instrumented}");
        assert_eq!(run.debug_output, EXPECTED_RESULTS, "{instrumented}");
    }
}

#[cfg(feature = "tms9900")]
#[test]
fn tms9900_differential_arithmetic_corpus_assembles() {
    use libre99_core::{
        bus::{Bus, FlatRam},
        cpu::Cpu,
    };

    let assembly =
        super::emit_tms9900_assembly_with_options(&program(), options(CpuFamily::Tms9900)).unwrap();
    let image = assemble_subset_with_symbols_at(AssemblerCpu::Tms9900, &assembly, 0x0200)
        .unwrap_or_else(|error| panic!("{error}\n{assembly}"));
    let mut ram = FlatRam::new();
    ram.load(0x0200, &image.bytes);
    let mut cpu = Cpu::new();
    cpu.set_pc(0x0200);
    for _ in 0..5_000 {
        cpu.step(&mut ram);
    }

    let actual = core::array::from_fn(|offset| ram.read_byte(0xA00A + offset as u16));
    assert_eq!(actual, EXPECTED_RESULTS, "{assembly}");
}

#[cfg(feature = "m6800")]
#[test]
fn m6800_differential_arithmetic_corpus_assembles() {
    let unsupported =
        super::emit_m6800_assembly_with_options(&program(), options(CpuFamily::M6800)).unwrap_err();
    assert!(
        unsupported
            .message
            .contains("does not support this binary operation"),
        "{unsupported}"
    );

    let assembly =
        super::emit_m6800_assembly_with_options(&parse(M6800_SOURCE), options(CpuFamily::M6800))
            .unwrap();
    validate(AssemblerCpu::M6800, &assembly);

    #[cfg(feature = "test-runner")]
    {
        const NAMES: [&str; 5] = [
            "add_wrap",
            "signed_shift",
            "shift_at_width",
            "shift_above_width",
            "mul_power_two",
        ];
        const EXPECTED: [u8; 5] = [2, 0xF0, 0, 0, 24];
        let writes = NAMES
            .iter()
            .enumerate()
            .map(|(offset, _)| format!("    ldaa >{:04X}h\n    staa >FFF0h", 0xA003 + offset))
            .collect::<Vec<_>>()
            .join("\n");
        let instrumented = assembly.replace(
            "__ezra_exit:\n    bra __ezra_exit\n",
            &format!(
                "__ezra_exit:\n{writes}\n    ldaa #01h\n    staa >FFF2h\n    bra __ezra_exit\n"
            ),
        );
        assert_ne!(
            instrumented, assembly,
            "missing M6800 exit loop\n{assembly}"
        );
        let run = run_image(CpuFamily::M6800, &instrumented, 10_000);
        assert!(run.halted, "{run:?}\n{instrumented}");
        assert_eq!(run.debug_output, EXPECTED, "{instrumented}");
    }
}

#[cfg(feature = "dcpu")]
#[test]
fn dcpu_differential_arithmetic_corpus_assembles() {
    let assembly =
        super::emit_dcpu_assembly_with_options(&parse(DCPU_SOURCE), options(CpuFamily::Dcpu))
            .unwrap();
    validate(AssemblerCpu::Dcpu, &assembly);
}

#[cfg(feature = "m68k")]
#[test]
fn m68k_differential_arithmetic_corpus_assembles() {
    let assembly =
        super::emit_m68k_assembly_with_options(&parse(M68K_SOURCE), options(CpuFamily::M68k))
            .unwrap();
    validate(AssemblerCpu::M68k, &assembly);
}

#[cfg(feature = "avr")]
#[test]
fn avr_differential_arithmetic_corpus_assembles() {
    let assembly =
        super::emit_avr_assembly_with_options(&program(), options(CpuFamily::Avr)).unwrap();
    validate(AssemblerCpu::Avr, &assembly);
}

#[cfg(feature = "i8086")]
#[test]
fn i8086_differential_arithmetic_corpus_assembles() {
    let assembly =
        super::emit_i8086_assembly_with_options(&program(), options(CpuFamily::I8086)).unwrap();
    validate(AssemblerCpu::I8086, &assembly);
}
