use std::panic::{AssertUnwindSafe, catch_unwind};

use mos6502::{cpu::CPU, instruction::Nmos6502, memory::Bus, registers::StackPointer};

use super::*;

const IO_BASE: u16 = 0xFF00;
const DEBUG_ADDR: u16 = IO_BASE + 0x0C;
const RESULT_ADDR: u16 = IO_BASE + 0x0D;
const HALT_ADDR: u16 = IO_BASE + 0x0E;

struct Mos6502TestEmulator;

struct TestBus {
    bytes: Box<[u8; 0x1_0000]>,
    halted: bool,
    result_code: u8,
    debug_output: Vec<u8>,
    ports: [u8; 256],
}

impl TestBus {
    fn new() -> Self {
        Self {
            bytes: Box::new([0; 0x1_0000]),
            halted: false,
            result_code: 0,
            debug_output: Vec::new(),
            ports: [0; 256],
        }
    }
}

impl Bus for TestBus {
    fn get_byte(&mut self, address: u16) -> u8 {
        self.bytes[usize::from(address)]
    }

    fn set_byte(&mut self, address: u16, value: u8) {
        self.bytes[usize::from(address)] = value;
        match address {
            DEBUG_ADDR => self.debug_output.push(value),
            RESULT_ADDR => self.result_code = value,
            HALT_ADDR => self.halted = value != 0,
            _ => {}
        }
        if address >= IO_BASE {
            self.ports[usize::from(address - IO_BASE)] = value;
        }
    }
}

impl EmulatorBackend for Mos6502TestEmulator {
    fn supports(&self, cpu_family: CpuFamily) -> bool {
        cpu_family == CpuFamily::Mos6502
    }

    fn run(&self, image: &TestImage, options: &TestRunOptions) -> Result<TestRun, Diagnostic> {
        let end = image
            .base_addr
            .checked_add(image.bytes.len() as u32)
            .filter(|end| *end <= 0x1_0000)
            .ok_or_else(|| Diagnostic::new("6502 test image exceeds 16-bit address space"))?;
        if !(0x0100..=0x01FF).contains(&options.stack_top) {
            return Err(Diagnostic::new(
                "6502 test stack must be inside hardware stack page $0100-$01FF",
            ));
        }
        let base = u16::try_from(image.base_addr)
            .map_err(|_| Diagnostic::new("6502 test image base is outside address space"))?;
        let mut bus = TestBus::new();
        for (port, value) in &options.initial_ports {
            bus.set_byte(IO_BASE + u16::from(*port), *value);
        }
        for (address, value) in &options.initial_memory {
            let address = u16::try_from(*address).map_err(|_| {
                Diagnostic::new("6502 initial memory address is outside address space")
            })?;
            bus.set_byte(address, *value);
        }
        for (offset, byte) in image.bytes.iter().copied().enumerate() {
            bus.set_byte(base + offset as u16, byte);
        }

        let mut cpu = CPU::new(bus, Nmos6502);
        cpu.registers.program_counter = base;
        cpu.registers.stack_pointer = StackPointer(options.stack_top as u8);
        let mut instructions = 0;
        let failure = loop {
            if cpu.memory.halted {
                break None;
            }
            if instructions >= options.instruction_budget {
                break Some(TestRunFailure::Timeout);
            }
            let pc = u32::from(cpu.registers.program_counter);
            if pc < image.base_addr || pc >= end {
                break Some(TestRunFailure::ExecutionOutsideMappedMemory { pc });
            }
            let step = catch_unwind(AssertUnwindSafe(|| cpu.single_step()));
            match step {
                Ok(true) => instructions += 1,
                Ok(false) | Err(_) => {
                    break Some(TestRunFailure::IllegalInstruction { pc });
                }
            }
        };

        Ok(TestRun {
            halted: cpu.memory.halted,
            result_code: cpu.memory.result_code,
            instructions,
            debug_output: cpu.memory.debug_output,
            ports: cpu.memory.ports,
            failure,
        })
    }
}

#[test]
fn mos6502_backend_runs_through_test_runner() {
    let assembly = r#"
        lda #$48
        sta $FF0C
        lda #$69
        sta $FF0C
        lda #$00
        sta $FF0D
        lda #$01
        sta $FF0E
    "#;
    let bytes = assemble_subset_at(CpuFamily::Mos6502, assembly, 0x0200).unwrap();
    let runner = TestRunner::new(vec![Box::new(Mos6502TestEmulator)]);
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::Mos6502,
                base_addr: 0x0200,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x01FF,
            },
        )
        .unwrap();

    assert!(run.halted);
    assert_eq!(run.result_code, 0);
    assert_eq!(run.debug_output, b"Hi");
    assert_eq!(run.failure, None);
}

#[test]
fn mos6502_backend_reports_timeout() {
    let bytes = assemble_subset_at(CpuFamily::Mos6502, "loop:\n    jmp loop\n", 0x0200).unwrap();
    let runner = TestRunner::new(vec![Box::new(Mos6502TestEmulator)]);
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::Mos6502,
                base_addr: 0x0200,
                bytes,
            },
            &TestRunOptions {
                instruction_budget: 3,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x01FF,
            },
        )
        .unwrap();

    assert!(!run.halted);
    assert_eq!(run.instructions, 3);
    assert_eq!(run.failure, Some(TestRunFailure::Timeout));
}
