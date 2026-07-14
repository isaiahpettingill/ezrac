use std::panic::{AssertUnwindSafe, catch_unwind};

use super::*;

const IO_BASE: u16 = 0xFF00;
const DEBUG_ADDR: u16 = IO_BASE + 0xF0;
const RESULT_ADDR: u16 = IO_BASE + 0xF1;
const HALT_ADDR: u16 = IO_BASE + 0xF2;

#[cfg(feature = "m6800")]
struct M6800TestEmulator;

#[cfg(feature = "m6800")]
struct TestBus {
    data: Vec<u8>,
    pub ports: [u8; 256],
    pub halted: bool,
    pub result_code: u8,
    pub debug_output: Vec<u8>,
}

#[cfg(feature = "m6800")]
impl TestBus {
    fn new() -> Self {
        Self {
            data: vec![0; 0x10000],
            ports: [0; 256],
            halted: false,
            result_code: 0,
            debug_output: Vec::new(),
        }
    }
}

#[cfg(feature = "m6800")]
impl M6800MemoryBus for TestBus {
    fn read(&self, address: u16) -> u8 {
        self.data[address as usize]
    }

    fn write(&mut self, address: u16, value: u8) {
        self.data[address as usize] = value;
        match address {
            DEBUG_ADDR => self.debug_output.push(value),
            RESULT_ADDR => self.result_code = value,
            HALT_ADDR => self.halted = value != 0,
            _ => {}
        }
        if address >= IO_BASE {
            self.ports[(address - IO_BASE) as usize] = value;
        }
    }
}

#[cfg(feature = "m6800")]
impl EmulatorBackend for M6800TestEmulator {
    fn supports(&self, cpu_family: CpuFamily) -> bool {
        cpu_family == CpuFamily::M6800
    }

    fn run(&self, image: &TestImage, options: &TestRunOptions) -> Result<TestRun, Diagnostic> {
        let end = image
            .base_addr
            .checked_add(image.bytes.len() as u32)
            .filter(|end| *end <= 0x1_0000)
            .ok_or_else(|| Diagnostic::new("M6800 test image exceeds 16-bit address space"))?;
        let base = u16::try_from(image.base_addr)
            .map_err(|_| Diagnostic::new("M6800 test image base is outside address space"))?;
        let mut bus = TestBus::new();
        for (port, value) in &options.initial_ports {
            bus.write(IO_BASE + u16::from(*port), *value);
        }
        for (address, value) in &options.initial_memory {
            let address = u16::try_from(*address).map_err(|_| {
                Diagnostic::new("M6800 initial memory address is outside address space")
            })?;
            bus.write(address, *value);
        }
        for (offset, byte) in image.bytes.iter().copied().enumerate() {
            bus.write(base + offset as u16, byte);
        }

        let mut cpu = M6800Cpu::new(bus);
        cpu.reg.pc = base;
        cpu.reg.sp = options.stack_top as u16;
        cpu.reset = false;

        let mut instructions = 0;
        let failure = loop {
            if cpu.halt || cpu.memory.halted {
                break None;
            }
            if instructions >= options.instruction_budget {
                break Some(TestRunFailure::Timeout);
            }
            let pc = u32::from(cpu.reg.pc);
            if pc < image.base_addr || pc >= end {
                break Some(TestRunFailure::ExecutionOutsideMappedMemory { pc });
            }
            let step = catch_unwind(AssertUnwindSafe(|| cpu.step()));
            match step {
                Ok(_) => instructions += 1,
                Err(_) => {
                    break Some(TestRunFailure::IllegalInstruction { pc });
                }
            }
        };

        Ok(TestRun {
            halted: cpu.halt || cpu.memory.halted,
            result_code: cpu.memory.result_code,
            instructions,
            debug_output: cpu.memory.debug_output,
            ports: cpu.memory.ports,
            failure,
        })
    }
}

#[test]
fn m6800_backend_runs_through_test_runner() {
    let assembly = r#"
        ldaa #$48
        staa $FFF0
        ldaa #$69
        staa $FFF0
        ldaa #$00
        staa $FFF1
        ldaa #$01
        staa $FFF2
    "#;
    let bytes = assemble_subset_at(CpuFamily::M6800, assembly, 0x0200).unwrap();
    let runner = TestRunner::new(vec![Box::new(M6800TestEmulator)]);
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::M6800,
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
fn m6800_backend_reports_timeout() {
    let bytes = assemble_subset_at(CpuFamily::M6800, "start:\n    bra start\n", 0x0200).unwrap();
    let runner = TestRunner::new(vec![Box::new(M6800TestEmulator)]);
    let run = runner
        .run(
            &TestImage {
                cpu_family: CpuFamily::M6800,
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
