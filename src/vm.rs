use crate::compat::{SourcePathBuf, prelude::*, source_path_text};
#[cfg(feature = "test-runner")]
use std::{
    cell::Cell,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[cfg(feature = "test-runner")]
use ez80::{Cpu, CpuMode, Machine, Reg8, Reg16};

#[cfg(feature = "m6800")]
use ::m6800::{Cpu as M6800Cpu, MemoryBus as M6800MemoryBus};
#[cfg(feature = "m68k")]
use ::m68000::{M68000, MemoryAccess as M68kMemoryAccess, cpu_details::Mc68000};

#[cfg(feature = "dcpu")]
use crate::asm::dcpu;
#[cfg(feature = "mos6502-emulator")]
use mos6502::{cpu::CPU, memory::Bus as _, registers::StackPointer};

#[cfg(any(feature = "std", feature = "avr"))]
use crate::asm::avr;
use crate::asm::ez80 as asm_meta;
use crate::asm::frontend::{
    AssemblyBinaryOperator, AssemblyDataValue, AssemblyExpression, AssemblyInstruction,
    AssemblyItem, AssemblyProgram, AssemblyUnaryOperator, DataWidth, LocatedAssemblyItem,
    LocatedParsedAssemblyItem, ParsedAssembly, ParsedAssemblyItem, lower_parsed_assembly,
    parse_assembly_expression, parse_assembly_syntax,
};
use crate::asm::grammar::{ArchitectureInstruction, parse_instruction};
#[cfg(feature = "m68k")]
use crate::asm::m68k as asm_m68k;
#[cfg(any(feature = "std", feature = "m6800"))]
use crate::asm::m6800;
#[cfg(any(feature = "std", feature = "mos6502"))]
use crate::asm::mos6502::Mos6502Variant;
#[cfg(feature = "tms9900")]
use crate::asm::tms9900;
use crate::diagnostic::{Diagnostic, SourceLocation};
use crate::target::{Address24, AssemblerCpu, CpuFamily};
#[cfg(feature = "test-runner")]
use crate::target::{EZRA_LOAD_ADDR, EZRA_STACK_TOP};
#[cfg(feature = "dcpu")]
use ::dcpu::emulator::{Cpu as DcpuCpu, cpu::OnDecodeError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssembledProgram {
    pub bytes: Vec<u8>,
    pub symbols: Vec<AssemblySymbol>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssemblySymbol {
    pub name: String,
    pub addr: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AssemblerSourceOptions {
    pub source_path: Option<SourcePathBuf>,
    pub symbols: Vec<AssemblySymbol>,
    pub section_bases: Vec<AssemblySymbol>,
    pub line_origins: Vec<SourceLocation>,
}

#[cfg(feature = "test-runner")]
mod runner {
    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct TestRun {
        pub halted: bool,
        pub result_code: u8,
        pub instructions: u64,
        pub debug_output: Vec<u8>,
        pub ports: [u8; 256],
        pub failure: Option<TestRunFailure>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum TestRunFailure {
        Timeout,
        ExecutionOutsideMappedMemory { pc: u32 },
        IllegalInstruction { pc: u32 },
        StackOverflow { sp: u32 },
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct TestRunOptions {
        pub instruction_budget: u64,
        pub initial_ports: Vec<(u8, u8)>,
        pub initial_memory: Vec<(u32, u8)>,
        pub stack_top: u32,
    }

    /// A fully assembled test image that an emulator backend can load and execute.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct TestImage {
        pub cpu_family: CpuFamily,
        pub base_addr: u32,
        pub bytes: Vec<u8>,
    }

    /// Extensible execution backend for target test images.
    ///
    /// The built-in backend uses the `ez80` crate for eZ80, Z80, Z80N, Z180,
    /// i8080, and i8085. Other CPU families can supply their own backend without
    /// changing the compiler test command.
    pub trait EmulatorBackend: Send + Sync {
        fn supports(&self, cpu_family: CpuFamily) -> bool;
        fn run(&self, image: &TestImage, options: &TestRunOptions) -> Result<TestRun, Diagnostic>;
    }

    pub struct TestRunner {
        backends: Vec<Box<dyn EmulatorBackend>>,
    }

    impl Default for TestRunner {
        fn default() -> Self {
            #[allow(unused_mut)]
            let mut backends: Vec<Box<dyn EmulatorBackend>> = vec![Box::new(Ez80Emulator)];

            #[cfg(feature = "dcpu")]
            backends.push(Box::new(DcpuEmulator));
            #[cfg(feature = "m6800")]
            backends.push(Box::new(M6800Emulator));
            #[cfg(feature = "m68k")]
            backends.push(Box::new(M68kEmulator));

            #[cfg(feature = "mos6502-emulator")]
            backends.push(Box::new(Mos6502Emulator));

            Self::new(backends)
        }
    }

    impl TestRunner {
        pub fn new(backends: Vec<Box<dyn EmulatorBackend>>) -> Self {
            Self { backends }
        }

        pub fn run(
            &self,
            image: &TestImage,
            options: &TestRunOptions,
        ) -> Result<TestRun, Diagnostic> {
            let backend = self
                .backends
                .iter()
                .find(|backend| backend.supports(image.cpu_family))
                .ok_or_else(|| {
                    Diagnostic::new(format!(
                        "no test emulator is registered for CPU `{}`",
                        image.cpu_family.as_str()
                    ))
                })?;
            backend.run(image, options)
        }
    }

    pub struct Ez80Emulator;

    const TEST_STACK_BYTES: u32 = 0x010000;

    pub fn run_assembly_test(
        assembly: &str,
        instruction_budget: u64,
    ) -> Result<TestRun, Diagnostic> {
        run_assembly_test_with_options(
            assembly,
            &TestRunOptions {
                instruction_budget,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
    }

    pub fn run_assembly_test_with_options(
        assembly: &str,
        options: &TestRunOptions,
    ) -> Result<TestRun, Diagnostic> {
        run_assembly_test_with_options_at(assembly, options, EZRA_LOAD_ADDR.get())
    }

    pub fn run_assembly_test_with_options_at(
        assembly: &str,
        options: &TestRunOptions,
        base_addr: u32,
    ) -> Result<TestRun, Diagnostic> {
        run_assembly_test_with_cpu_options_at(CpuFamily::Ez80, assembly, options, base_addr)
    }

    pub fn run_assembly_test_with_cpu_options_at(
        cpu_family: CpuFamily,
        assembly: &str,
        options: &TestRunOptions,
        base_addr: u32,
    ) -> Result<TestRun, Diagnostic> {
        let code = assemble_subset_at(cpu_family, assembly, base_addr)?;
        TestRunner::default().run(
            &TestImage {
                cpu_family,
                base_addr,
                bytes: code,
            },
            options,
        )
    }

    impl EmulatorBackend for Ez80Emulator {
        fn supports(&self, cpu_family: CpuFamily) -> bool {
            matches!(
                cpu_family,
                CpuFamily::Ez80
                    | CpuFamily::Z80
                    | CpuFamily::Z80N
                    | CpuFamily::Z180
                    | CpuFamily::I8080
                    | CpuFamily::I8085
                    | CpuFamily::Lr35902
            )
        }

        fn run(&self, image: &TestImage, options: &TestRunOptions) -> Result<TestRun, Diagnostic> {
            let address_limit = address_limit_for_family(image.cpu_family);
            if image.base_addr > address_limit {
                return Err(Diagnostic::new(format!(
                    "test image base address 0x{:X} is outside the {}-bit address space",
                    image.base_addr,
                    address_width_for_family(image.cpu_family)
                )));
            }
            if options.stack_top > address_limit {
                return Err(Diagnostic::new(format!(
                    "test stack top 0x{:X} is outside the {}-bit address space",
                    options.stack_top,
                    address_width_for_family(image.cpu_family)
                )));
            }
            for (address, _) in &options.initial_memory {
                if *address > address_limit {
                    return Err(Diagnostic::new(format!(
                        "test memory address 0x{address:X} is outside the {}-bit address space",
                        address_width_for_family(image.cpu_family)
                    )));
                }
            }

            let code_start = image.base_addr;
            let code_end =
                checked_code_end_for_family(code_start, image.bytes.len(), image.cpu_family)?;
            let mut machine =
                TestMachine::new(address_limit, image.cpu_family == CpuFamily::Lr35902);
            for (port, value) in &options.initial_ports {
                machine.ports[*port as usize] = *value;
            }
            for (address, value) in &options.initial_memory {
                machine.poke(*address, *value);
            }
            for (address, byte) in image.bytes.iter().copied().enumerate() {
                machine.poke(image.base_addr + address as u32, byte);
            }

            let mut cpu = Cpu::new_for_mode(cpu_mode_for_family(image.cpu_family));
            cpu.state.reg.adl = image.cpu_family == CpuFamily::Ez80;
            cpu.state.set_pc(image.base_addr);
            if image.cpu_family != CpuFamily::Ez80 {
                cpu.state.reg.set16(Reg16::SP, options.stack_top as u16);
            } else {
                cpu.state.reg.set24(Reg16::SP, options.stack_top);
            }
            if std::env::var_os("EZRA_TRACE_VM").is_some() {
                cpu.set_trace(true);
            }

            for instruction in 0..options.instruction_budget {
                let pc = cpu.state.pc();
                if is_cpm_cpu(image.cpu_family) && handle_cpm_bdos_call(&mut cpu, &mut machine)? {
                    if machine.halted {
                        return Ok(TestRun {
                            halted: true,
                            result_code: machine.result_code,
                            instructions: instruction + 1,
                            debug_output: machine.debug_output,
                            ports: machine.ports,
                            failure: None,
                        });
                    }
                    continue;
                }
                if pc < code_start || pc >= code_end {
                    return Ok(TestRun {
                        halted: false,
                        result_code: machine.result_code,
                        instructions: instruction,
                        debug_output: machine.debug_output,
                        ports: machine.ports,
                        failure: Some(TestRunFailure::ExecutionOutsideMappedMemory { pc }),
                    });
                }
                if catch_unwind(AssertUnwindSafe(|| cpu.execute_instruction(&mut machine))).is_err()
                {
                    return Ok(TestRun {
                        halted: false,
                        result_code: machine.result_code,
                        instructions: instruction,
                        debug_output: machine.debug_output,
                        ports: machine.ports,
                        failure: Some(TestRunFailure::IllegalInstruction { pc }),
                    });
                }
                let sp = if image.cpu_family == CpuFamily::Ez80 {
                    cpu.state.reg.get24(Reg16::SP)
                } else {
                    cpu.state.reg.get16(Reg16::SP) as u32
                };
                if !stack_pointer_in_bounds(sp, options.stack_top) {
                    return Ok(TestRun {
                        halted: false,
                        result_code: machine.result_code,
                        instructions: instruction + 1,
                        debug_output: machine.debug_output,
                        ports: machine.ports,
                        failure: Some(TestRunFailure::StackOverflow { sp }),
                    });
                }
                if machine.halted {
                    return Ok(TestRun {
                        halted: true,
                        result_code: machine.result_code,
                        instructions: instruction + 1,
                        debug_output: machine.debug_output,
                        ports: machine.ports,
                        failure: None,
                    });
                }
            }

            Ok(TestRun {
                halted: false,
                result_code: machine.result_code,
                instructions: options.instruction_budget,
                debug_output: machine.debug_output,
                ports: machine.ports,
                failure: Some(TestRunFailure::Timeout),
            })
        }
    }

    #[cfg(feature = "dcpu")]
    pub struct DcpuEmulator;

    /// DCPU test ABI word addresses. A changed debug sequence emits the low byte
    /// of the debug value word; result and halt use the low byte of their words.
    #[cfg(feature = "dcpu")]
    const DCPU_DEBUG_SEQUENCE: u16 = 0xFFF0;
    #[cfg(feature = "dcpu")]
    const DCPU_DEBUG_VALUE: u16 = 0xFFF1;
    #[cfg(feature = "dcpu")]
    const DCPU_RESULT_CODE: u16 = 0xFFF2;
    #[cfg(feature = "dcpu")]
    const DCPU_HALT: u16 = 0xFFF3;
    #[cfg(feature = "dcpu")]
    const DCPU_ADDRESS_SPACE_BYTES: u32 = 0x2_0000;

    #[cfg(feature = "dcpu")]
    impl EmulatorBackend for DcpuEmulator {
        fn supports(&self, cpu_family: CpuFamily) -> bool {
            cpu_family == CpuFamily::Dcpu
        }

        fn run(&self, image: &TestImage, options: &TestRunOptions) -> Result<TestRun, Diagnostic> {
            let code_end =
                validate_dcpu_byte_range(image.base_addr, image.bytes.len(), "test image")?;
            let stack_top = dcpu_aligned_byte_address_to_word(options.stack_top, "test stack top")?;
            for (address, _) in &options.initial_memory {
                dcpu_byte_address_to_word(*address, "test memory address")?;
            }

            let words = image
                .bytes
                .chunks_exact(2)
                .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
                .collect::<Vec<_>>();
            let mut cpu = DcpuCpu::new(OnDecodeError::Fail);
            cpu.sp = std::num::Wrapping(stack_top);
            cpu.ram[DCPU_DEBUG_SEQUENCE] = 0;
            cpu.ram[DCPU_DEBUG_VALUE] = 0;
            cpu.ram[DCPU_RESULT_CODE] = 0;
            cpu.ram[DCPU_HALT] = 0;

            for (address, value) in &options.initial_memory {
                let word_address = dcpu_byte_address_to_word(*address, "test memory address")?;
                let word = &mut cpu.ram[word_address];
                if address & 1 == 0 {
                    *word = (*word & 0xFF00) | u16::from(*value);
                } else {
                    *word = (*word & 0x00FF) | (u16::from(*value) << 8);
                }
            }
            cpu.load(
                &words,
                dcpu_byte_address_to_word(image.base_addr, "test image base address")?,
            );

            let mut debug_sequence = cpu.ram[DCPU_DEBUG_SEQUENCE];
            let mut debug_output = Vec::new();
            for instruction in 0..options.instruction_budget {
                let pc = u32::from(cpu.pc.0) * 2;
                if pc < image.base_addr || pc >= code_end {
                    return Ok(dcpu_test_run(
                        false,
                        cpu.ram[DCPU_RESULT_CODE] as u8,
                        instruction,
                        debug_output,
                        Some(TestRunFailure::ExecutionOutsideMappedMemory { pc }),
                    ));
                }
                if !matches!(
                    catch_unwind(AssertUnwindSafe(|| cpu.tick(&mut []))),
                    Ok(Ok(_))
                ) {
                    return Ok(dcpu_test_run(
                        false,
                        cpu.ram[DCPU_RESULT_CODE] as u8,
                        instruction,
                        debug_output,
                        Some(TestRunFailure::IllegalInstruction { pc }),
                    ));
                }

                let sequence = cpu.ram[DCPU_DEBUG_SEQUENCE];
                if sequence != debug_sequence {
                    debug_sequence = sequence;
                    debug_output.push(cpu.ram[DCPU_DEBUG_VALUE] as u8);
                }
                if cpu.ram[DCPU_HALT] != 0 || cpu.halted {
                    return Ok(dcpu_test_run(
                        true,
                        cpu.ram[DCPU_RESULT_CODE] as u8,
                        instruction + 1,
                        debug_output,
                        None,
                    ));
                }
            }

            Ok(dcpu_test_run(
                false,
                cpu.ram[DCPU_RESULT_CODE] as u8,
                options.instruction_budget,
                debug_output,
                Some(TestRunFailure::Timeout),
            ))
        }
    }

    #[cfg(feature = "dcpu")]
    fn validate_dcpu_byte_range(base: u32, len: usize, subject: &str) -> Result<u32, Diagnostic> {
        dcpu_aligned_byte_address_to_word(base, &format!("{subject} base address"))?;
        if len % 2 != 0 {
            return Err(Diagnostic::new(format!(
                "{subject} must contain an even number of bytes for DCPU word memory"
            )));
        }
        let end = base
            .checked_add(
                u32::try_from(len).map_err(|_| Diagnostic::new("test image is too large"))?,
            )
            .filter(|end| *end <= DCPU_ADDRESS_SPACE_BYTES)
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "{subject} exceeds the DCPU 16-bit word address space"
                ))
            })?;
        Ok(end)
    }

    #[cfg(feature = "dcpu")]
    fn dcpu_byte_address_to_word(address: u32, subject: &str) -> Result<u16, Diagnostic> {
        if address >= DCPU_ADDRESS_SPACE_BYTES {
            return Err(Diagnostic::new(format!(
                "{subject} 0x{address:X} is outside the DCPU 16-bit word address space"
            )));
        }
        Ok((address / 2) as u16)
    }

    #[cfg(feature = "dcpu")]
    fn dcpu_aligned_byte_address_to_word(address: u32, subject: &str) -> Result<u16, Diagnostic> {
        if address % 2 != 0 {
            return Err(Diagnostic::new(format!(
                "{subject} 0x{address:X} is not an aligned byte address in the DCPU 16-bit word address space"
            )));
        }
        dcpu_byte_address_to_word(address, subject)
    }

    #[cfg(feature = "dcpu")]
    fn dcpu_test_run(
        halted: bool,
        result_code: u8,
        instructions: u64,
        debug_output: Vec<u8>,
        failure: Option<TestRunFailure>,
    ) -> TestRun {
        TestRun {
            halted,
            result_code,
            instructions,
            debug_output,
            ports: [0; 256],
            failure,
        }
    }

    #[cfg(feature = "m68k")]
    pub struct M68kEmulator;

    #[cfg(feature = "m68k")]
    struct M68kTestMemory {
        data: HashMap<u32, u8>,
        ports: [u8; 256],
        halted: bool,
        result_code: u8,
        debug_output: Vec<u8>,
    }

    #[cfg(feature = "m68k")]
    impl M68kTestMemory {
        const DEBUG_OUTPUT: u32 = 0xFFFFF0;
        const RESULT_CODE: u32 = 0xFFFFF1;
        const HALT: u32 = 0xFFFFF2;

        fn new() -> Self {
            Self {
                data: HashMap::new(),
                ports: [0; 256],
                halted: false,
                result_code: 0,
                debug_output: Vec::new(),
            }
        }

        fn write_byte(&mut self, address: u32, value: u8) -> Option<()> {
            if address > Address24::MAX {
                return None;
            }
            self.data.insert(address, value);
            match address {
                Self::DEBUG_OUTPUT => self.debug_output.push(value),
                Self::RESULT_CODE => self.result_code = value,
                Self::HALT if value != 0 => self.halted = true,
                _ => {}
            }
            Some(())
        }
    }

    #[cfg(feature = "m68k")]
    impl M68kMemoryAccess for M68kTestMemory {
        fn get_byte(&mut self, address: u32) -> Option<u8> {
            (address <= Address24::MAX).then(|| self.data.get(&address).copied().unwrap_or(0))
        }

        fn get_word(&mut self, address: u32) -> Option<u16> {
            Some(
                (self.get_byte(address)? as u16) << 8
                    | self.get_byte(address.checked_add(1)?)? as u16,
            )
        }

        fn set_byte(&mut self, address: u32, value: u8) -> Option<()> {
            self.write_byte(address, value)
        }

        fn set_word(&mut self, address: u32, value: u16) -> Option<()> {
            self.write_byte(address, (value >> 8) as u8)?;
            self.write_byte(address.checked_add(1)?, value as u8)
        }

        fn reset_instruction(&mut self) {}
    }

    #[cfg(feature = "m68k")]
    impl EmulatorBackend for M68kEmulator {
        fn supports(&self, cpu_family: CpuFamily) -> bool {
            cpu_family == CpuFamily::M68k
        }

        fn run(&self, image: &TestImage, options: &TestRunOptions) -> Result<TestRun, Diagnostic> {
            if image.base_addr > Address24::MAX {
                return Err(Diagnostic::new(format!(
                    "test image base address 0x{:X} is outside the 24-bit address space",
                    image.base_addr
                )));
            }
            if options.stack_top > Address24::MAX {
                return Err(Diagnostic::new(format!(
                    "test stack top 0x{:X} is outside the 24-bit address space",
                    options.stack_top
                )));
            }
            for (address, _) in &options.initial_memory {
                if *address > Address24::MAX {
                    return Err(Diagnostic::new(format!(
                        "test memory address 0x{address:X} is outside the 24-bit address space"
                    )));
                }
            }

            let code_start = image.base_addr;
            let code_end = checked_code_end(code_start, image.bytes.len())?;
            let mut memory = M68kTestMemory::new();
            for (port, value) in &options.initial_ports {
                memory.ports[*port as usize] = *value;
            }
            for (address, value) in &options.initial_memory {
                memory.write_byte(*address, *value);
            }
            for (offset, byte) in image.bytes.iter().copied().enumerate() {
                memory.write_byte(code_start + offset as u32, byte);
            }

            let mut cpu = M68000::<Mc68000>::new_no_reset();
            cpu.regs.pc.0 = code_start;
            cpu.regs.sp_mut().0 = options.stack_top;

            for instruction in 0..options.instruction_budget {
                let pc = cpu.regs.pc.0;
                if pc > Address24::MAX || pc < code_start || pc >= code_end {
                    return Ok(TestRun {
                        halted: false,
                        result_code: memory.result_code,
                        instructions: instruction,
                        debug_output: memory.debug_output,
                        ports: memory.ports,
                        failure: Some(TestRunFailure::ExecutionOutsideMappedMemory { pc }),
                    });
                }
                let result =
                    catch_unwind(AssertUnwindSafe(|| cpu.interpreter_exception(&mut memory)));
                let Ok((_, exception)) = result else {
                    return Ok(TestRun {
                        halted: false,
                        result_code: memory.result_code,
                        instructions: instruction,
                        debug_output: memory.debug_output,
                        ports: memory.ports,
                        failure: Some(TestRunFailure::IllegalInstruction { pc }),
                    });
                };
                if exception.is_some() {
                    return Ok(TestRun {
                        halted: false,
                        result_code: memory.result_code,
                        instructions: instruction,
                        debug_output: memory.debug_output,
                        ports: memory.ports,
                        failure: Some(TestRunFailure::IllegalInstruction { pc }),
                    });
                }
                let sp = cpu.regs.sp();
                if !stack_pointer_in_bounds(sp, options.stack_top) {
                    return Ok(TestRun {
                        halted: false,
                        result_code: memory.result_code,
                        instructions: instruction + 1,
                        debug_output: memory.debug_output,
                        ports: memory.ports,
                        failure: Some(TestRunFailure::StackOverflow { sp }),
                    });
                }
                if memory.halted || cpu.stop {
                    return Ok(TestRun {
                        halted: true,
                        result_code: memory.result_code,
                        instructions: instruction + 1,
                        debug_output: memory.debug_output,
                        ports: memory.ports,
                        failure: None,
                    });
                }
            }

            Ok(TestRun {
                halted: false,
                result_code: memory.result_code,
                instructions: options.instruction_budget,
                debug_output: memory.debug_output,
                ports: memory.ports,
                failure: Some(TestRunFailure::Timeout),
            })
        }
    }

    #[cfg(feature = "m6800")]
    pub struct M6800Emulator;

    #[cfg(feature = "m6800")]
    struct M6800TestMemory {
        data: Vec<u8>,
        pub ports: [u8; 256],
        pub halted: bool,
        pub result_code: u8,
        pub debug_output: Vec<u8>,
    }

    #[cfg(feature = "m6800")]
    impl M6800TestMemory {
        fn new() -> Self {
            Self {
                data: vec![0; 0x10000],
                ports: [0; 256],
                halted: false,
                result_code: 0,
                debug_output: Vec::new(),
            }
        }

        fn load(&mut self, base: u16, bytes: &[u8]) {
            for (i, &b) in bytes.iter().enumerate() {
                let addr = base.wrapping_add(i as u16);
                self.data[addr as usize] = b;
            }
        }
    }

    #[cfg(feature = "m6800")]
    impl M6800MemoryBus for M6800TestMemory {
        fn read(&self, address: u16) -> u8 {
            self.data[address as usize]
        }

        fn write(&mut self, address: u16, value: u8) {
            match address {
                0xFFF0 => self.debug_output.push(value),
                0xFFF1 => self.result_code = value,
                0xFFF2 if value == 1 => self.halted = true,
                _ => self.data[address as usize] = value,
            }
        }
    }

    #[cfg(feature = "m6800")]
    impl EmulatorBackend for M6800Emulator {
        fn supports(&self, cpu_family: CpuFamily) -> bool {
            cpu_family == CpuFamily::M6800
        }

        fn run(&self, image: &TestImage, options: &TestRunOptions) -> Result<TestRun, Diagnostic> {
            if image.base_addr > 0xFFFF {
                return Err(Diagnostic::new(format!(
                    "test image base address 0x{:X} is outside the 16-bit address space",
                    image.base_addr
                )));
            }
            if options.stack_top > 0xFFFF {
                return Err(Diagnostic::new(format!(
                    "test stack top 0x{:X} is outside the 16-bit address space",
                    options.stack_top
                )));
            }

            let code_start = image.base_addr as u16;
            let mut memory = M6800TestMemory::new();
            for (port, value) in &options.initial_ports {
                memory.ports[*port as usize] = *value;
            }
            for (address, value) in &options.initial_memory {
                if *address <= 0xFFFF {
                    memory.data[*address as usize] = *value;
                }
            }
            memory.load(code_start, &image.bytes);

            let mut cpu = M6800Cpu::new(memory);
            cpu.reg.pc = code_start;
            cpu.reg.sp = options.stack_top as u16;
            cpu.reset = false;

            for _instruction in 0..options.instruction_budget {
                cpu.step();

                if cpu.halt || cpu.memory.halted {
                    return Ok(TestRun {
                        halted: true,
                        result_code: cpu.memory.result_code,
                        instructions: _instruction + 1,
                        debug_output: cpu.memory.debug_output.clone(),
                        ports: cpu.memory.ports,
                        failure: None,
                    });
                }
            }

            Ok(TestRun {
                halted: false,
                result_code: cpu.memory.result_code,
                instructions: options.instruction_budget,
                debug_output: cpu.memory.debug_output,
                ports: cpu.memory.ports,
                failure: Some(TestRunFailure::Timeout),
            })
        }
    }

    #[cfg(feature = "mos6502-emulator")]
    mod mos6502_backend {
        use super::*;

        #[cfg(feature = "mos6502")]
        pub struct Mos6502Emulator;

        #[cfg(feature = "mos6502")]
        const MOS6502_IO_BASE: u16 = 0xFF00;

        #[cfg(feature = "mos6502")]
        const MOS6502_DEBUG_ADDR: u16 = MOS6502_IO_BASE + 0x0C;

        #[cfg(feature = "mos6502")]
        const MOS6502_RESULT_ADDR: u16 = MOS6502_IO_BASE + 0x0D;

        #[cfg(feature = "mos6502")]
        const MOS6502_HALT_ADDR: u16 = MOS6502_IO_BASE + 0x0E;

        #[cfg(feature = "mos6502")]
        struct Mos6502Bus {
            bytes: Box<[u8; 0x1_0000]>,
            halted: bool,
            result_code: u8,
            debug_output: Vec<u8>,
            ports: [u8; 256],
        }

        #[cfg(feature = "mos6502")]
        impl Mos6502Bus {
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

        #[cfg(feature = "mos6502")]
        impl mos6502::memory::Bus for Mos6502Bus {
            fn get_byte(&mut self, address: u16) -> u8 {
                self.bytes[usize::from(address)]
            }

            fn set_byte(&mut self, address: u16, value: u8) {
                self.bytes[usize::from(address)] = value;
                match address {
                    MOS6502_DEBUG_ADDR => self.debug_output.push(value),
                    MOS6502_RESULT_ADDR => self.result_code = value,
                    MOS6502_HALT_ADDR => self.halted = value != 0,
                    _ => {}
                }
                if address >= MOS6502_IO_BASE {
                    self.ports[usize::from(address - MOS6502_IO_BASE)] = value;
                }
            }
        }

        #[cfg(feature = "mos6502")]
        fn run_mos6502_with_variant<V: mos6502::Variant + Default>(
            image: &TestImage,
            options: &TestRunOptions,
        ) -> Result<TestRun, Diagnostic> {
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
            let mut bus = Mos6502Bus::new();
            for (port, value) in &options.initial_ports {
                bus.set_byte(MOS6502_IO_BASE + u16::from(*port), *value);
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

            let mut cpu = CPU::new(bus, V::default());
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

        #[cfg(feature = "mos6502")]
        impl EmulatorBackend for Mos6502Emulator {
            fn supports(&self, cpu_family: CpuFamily) -> bool {
                matches!(
                    cpu_family,
                    CpuFamily::Mos6502 | CpuFamily::Cmos65C02 | CpuFamily::Ricoh2A03
                )
            }

            fn run(
                &self,
                image: &TestImage,
                options: &TestRunOptions,
            ) -> Result<TestRun, Diagnostic> {
                match image.cpu_family {
                    CpuFamily::Mos6502 => {
                        run_mos6502_with_variant::<mos6502::instruction::Nmos6502>(image, options)
                    }
                    CpuFamily::Cmos65C02 => {
                        run_mos6502_with_variant::<mos6502::instruction::Cmos6502>(image, options)
                    }
                    CpuFamily::Ricoh2A03 => {
                        run_mos6502_with_variant::<mos6502::instruction::Ricoh2a03>(image, options)
                    }
                    _ => Err(Diagnostic::new(format!(
                        "no test emulator is registered for CPU `{}`",
                        image.cpu_family.as_str()
                    ))),
                }
            }
        }
    }

    #[cfg(feature = "mos6502-emulator")]
    pub use mos6502_backend::Mos6502Emulator;

    fn address_width_for_family(cpu_family: CpuFamily) -> u8 {
        if cpu_family == CpuFamily::Ez80 {
            24
        } else {
            16
        }
    }

    fn address_limit_for_family(cpu_family: CpuFamily) -> u32 {
        match cpu_family {
            CpuFamily::Ez80 | CpuFamily::Wdc65C816 => Address24::MAX,
            _ => u16::MAX as u32,
        }
    }

    fn checked_code_end_for_family(
        code_start: u32,
        code_len: usize,
        cpu_family: CpuFamily,
    ) -> Result<u32, Diagnostic> {
        let end = checked_code_end(code_start, code_len)?;
        if end > address_limit_for_family(cpu_family) {
            return Err(Diagnostic::new(format!(
                "test program exceeds the {}-bit address space",
                address_width_for_family(cpu_family)
            )));
        }
        Ok(end)
    }

    fn is_cpm_cpu(cpu_family: CpuFamily) -> bool {
        matches!(
            cpu_family,
            CpuFamily::Z80 | CpuFamily::I8080 | CpuFamily::I8085
        )
    }

    fn cpu_mode_for_family(cpu: CpuFamily) -> CpuMode {
        match cpu {
            CpuFamily::Ez80 => CpuMode::EZ80,
            CpuFamily::Z80 => CpuMode::Z80,
            CpuFamily::Z80N => CpuMode::Z80N,
            CpuFamily::Z180 => CpuMode::Z180,
            CpuFamily::I8080 => CpuMode::I8080,
            CpuFamily::I8085 => CpuMode::I8085,
            CpuFamily::Lr35902 => CpuMode::GameBoy,
            CpuFamily::M68k
            | CpuFamily::M6800
            | CpuFamily::Mos6502
            | CpuFamily::Cmos65C02
            | CpuFamily::Wdc65C816
            | CpuFamily::Ricoh2A03
            | CpuFamily::Tms9900
            | CpuFamily::Avr
            | CpuFamily::Dcpu => CpuMode::Z80,
        }
    }

    fn handle_cpm_bdos_call(cpu: &mut Cpu, machine: &mut TestMachine) -> Result<bool, Diagnostic> {
        if cpu.state.pc() != 0x0005 {
            return Ok(false);
        }

        match cpu.state.reg.get8(Reg8::C) {
            0 => machine.halted = true,
            2 => machine.debug_output.push(cpu.state.reg.get8(Reg8::E)),
            9 => {
                let mut address = cpu.state.reg.get16(Reg16::DE) as u32;
                for _ in 0..0x1_0000 {
                    let byte = machine.peek(address);
                    if byte == b'$' {
                        break;
                    }
                    machine.debug_output.push(byte);
                    address = address.wrapping_add(1) & 0xFFFF;
                }
            }
            function => {
                return Err(Diagnostic::new(format!(
                    "CP/M BDOS function {function} is not implemented by the test runner"
                )));
            }
        }

        let sp = cpu.state.reg.get16(Reg16::SP) as u32;
        let return_addr =
            machine.peek(sp) as u32 | ((machine.peek(sp.wrapping_add(1)) as u32) << 8);
        cpu.state.reg.set16(Reg16::SP, sp.wrapping_add(2) as u16);
        cpu.state.set_pc(return_addr);
        Ok(true)
    }

    fn stack_pointer_in_bounds(sp: u32, stack_top: u32) -> bool {
        let floor = stack_top.saturating_sub(TEST_STACK_BYTES);
        (floor..=stack_top).contains(&sp)
    }
}

#[cfg(feature = "test-runner")]
pub use runner::*;

pub fn assemble_ez80_subset_at(assembly: &str, base_addr: u32) -> Result<Vec<u8>, Diagnostic> {
    Ok(assemble_ez80_subset_with_symbols_at(assembly, base_addr)?.bytes)
}

pub fn assemble_ez80_subset_with_symbols_at(
    assembly: &str,
    base_addr: u32,
) -> Result<AssembledProgram, Diagnostic> {
    assemble_subset_with_symbols_at(AssemblerCpu::Ez80, assembly, base_addr)
}

pub fn assemble_subset_at(
    cpu_family: CpuFamily,
    assembly: &str,
    base_addr: u32,
) -> Result<Vec<u8>, Diagnostic> {
    Ok(assemble_subset_with_symbols_at(AssemblerCpu::from(cpu_family), assembly, base_addr)?.bytes)
}

pub fn assemble_subset_with_symbols_at(
    cpu: AssemblerCpu,
    assembly: &str,
    base_addr: u32,
) -> Result<AssembledProgram, Diagnostic> {
    assemble_subset_with_options_at(cpu, assembly, base_addr, &AssemblerSourceOptions::default())
}

pub fn assemble_subset_with_options_at(
    cpu: AssemblerCpu,
    assembly: &str,
    base_addr: u32,
    options: &AssemblerSourceOptions,
) -> Result<AssembledProgram, Diagnostic> {
    let program = parse_assembly_program(cpu, assembly, options)?;
    assemble_program_with_options_at(cpu, &program, base_addr, options)
}

pub fn assemble_program_at(
    cpu: AssemblerCpu,
    program: &AssemblyProgram,
    base_addr: u32,
) -> Result<AssembledProgram, Diagnostic> {
    assemble_program_with_options_at(cpu, program, base_addr, &AssemblerSourceOptions::default())
}

pub fn assemble_program_with_options_at(
    cpu: AssemblerCpu,
    program: &AssemblyProgram,
    base_addr: u32,
    options: &AssemblerSourceOptions,
) -> Result<AssembledProgram, Diagnostic> {
    if !cpu.is_enabled() {
        return Err(Diagnostic::new(format!(
            "assembler CPU `{}` requires the `{}` Cargo feature",
            cpu.as_str(),
            cpu.feature_name()
        )));
    }
    if base_addr > Address24::MAX {
        return Err(Diagnostic::new(format!(
            "assembly base address 0x{base_addr:X} is outside the 24-bit address space"
        )));
    }

    let architecture_instructions = parse_program_instructions(cpu, program)?;
    let mut instruction_lengths = vec![None; program.items.len()];
    let mut labels = BTreeMap::new();
    let mut declared_names = HashSet::new();
    for symbol in &options.symbols {
        labels.insert(symbol.name.clone(), symbol.addr);
        declared_names.insert(symbol.name.clone());
    }
    let mut pending_equates = Vec::new();
    let begins_with_section = program
        .items
        .first()
        .is_some_and(|item| matches!(&item.kind, AssemblyItem::Section(_)));
    let default_pc = if begins_with_section {
        base_addr
    } else {
        section_base(options, ".text").unwrap_or(base_addr)
    } & 0xFF_FFFF;
    let mut pc = default_pc;

    for (item_index, item) in program.items.iter().enumerate() {
        match &item.kind {
            AssemblyItem::Label(name) => {
                if pc > Address24::MAX {
                    return Err(item_diagnostic(
                        item,
                        format!(
                            "assembly label `{name}` address 0x{pc:X} is outside the 24-bit address space"
                        ),
                    ));
                }
                if !declared_names.insert(name.clone()) {
                    return Err(item_diagnostic(
                        item,
                        format!("duplicate assembly label `{name}`"),
                    ));
                }
                labels.insert(name.clone(), pc);
            }
            AssemblyItem::Equ { name, value } => {
                if !declared_names.insert(name.clone()) {
                    return Err(item_diagnostic(
                        item,
                        format!("duplicate assembly symbol `{name}`"),
                    ));
                }
                pending_equates.push((item.clone(), name.clone(), value.clone(), pc));
            }
            AssemblyItem::Section(name) => {
                if let Some(base) = section_base(options, name) {
                    pc = base;
                }
            }
            AssemblyItem::Org(expression) => {
                let known = labels.clone().into_iter().collect::<HashMap<_, _>>();
                pc = eval_assembly_expression(expression, &known, pc)
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
            }
            AssemblyItem::Data { width, values } => {
                pc = checked_assembly_pc_advance(pc, data_len(*width, values) as u32)
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
            }
            AssemblyItem::Instruction(instruction) => {
                let architecture = architecture_instructions[item_index]
                    .as_ref()
                    .expect("architecture parsing follows program items");
                let len = instruction_len(cpu, architecture, &instruction.to_text())
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
                instruction_lengths[item_index] = Some(len);
                pc = checked_assembly_pc_advance(pc, len as u32)
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
            }
        }
    }

    while !pending_equates.is_empty() {
        let known = labels.clone().into_iter().collect::<HashMap<_, _>>();
        let mut unresolved = Vec::new();
        let mut progress = false;
        for (item, name, expression, equ_pc) in pending_equates {
            match eval_assembly_expression(&expression, &known, equ_pc) {
                Ok(value) => {
                    labels.insert(name, value);
                    progress = true;
                }
                Err(_) => unresolved.push((item, name, expression, equ_pc)),
            }
        }
        if !progress {
            let (item, _, expression, equ_pc) = &unresolved[0];
            return Err(eval_assembly_expression(expression, &known, *equ_pc)
                .unwrap_err()
                .with_location_if_missing(item.location.clone()));
        }
        pending_equates = unresolved;
    }

    let symbols = labels
        .iter()
        .map(|(name, addr)| AssemblySymbol {
            name: name.clone(),
            addr: *addr,
        })
        .collect();
    let labels = labels.into_iter().collect::<HashMap<_, _>>();
    let mut bytes = Vec::new();
    let mut pc = base_addr & 0xFF_FFFF;
    if default_pc != pc {
        append_org_padding(&mut bytes, pc, default_pc)?;
        pc = default_pc;
    }
    for (item_index, item) in program.items.iter().enumerate() {
        match &item.kind {
            AssemblyItem::Label(_) | AssemblyItem::Equ { .. } => {}
            AssemblyItem::Section(name) => {
                if let Some(base) = section_base(options, name) {
                    append_org_padding(&mut bytes, pc, base)
                        .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
                    pc = base;
                }
            }
            AssemblyItem::Org(expression) => {
                let new_pc = eval_assembly_expression(expression, &labels, pc)
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
                append_org_padding(&mut bytes, pc, new_pc)
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
                pc = new_pc;
            }
            AssemblyItem::Data { width, values } => {
                emit_data(cpu, *width, values, &labels, pc, &mut bytes)
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
                pc = checked_assembly_pc_advance(pc, data_len(*width, values) as u32)
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
            }
            AssemblyItem::Instruction(instruction) => {
                let architecture = architecture_instructions[item_index]
                    .as_ref()
                    .expect("architecture parsing follows program items");
                emit_instruction(
                    cpu,
                    architecture,
                    &instruction.to_text(),
                    &labels,
                    pc,
                    &mut bytes,
                )
                .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
                let len = instruction_lengths[item_index]
                    .expect("instruction length was computed during the first pass");
                pc = checked_assembly_pc_advance(pc, len as u32)
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
            }
        }
    }
    Ok(AssembledProgram { bytes, symbols })
}

fn parse_assembly_program(
    cpu: AssemblerCpu,
    assembly: &str,
    options: &AssemblerSourceOptions,
) -> Result<AssemblyProgram, Diagnostic> {
    let source_name = options
        .source_path
        .as_ref()
        .map(|path| source_path_text(path))
        .unwrap_or_else(|| "<assembly>".to_owned());
    let (mut parsed, restore_z80_alt_af) = match parse_assembly_syntax(&source_name, assembly) {
        Ok(parsed) => (parsed, false),
        Err(original_error) if cpu.supports_z80_syntax() => {
            let Some(normalized) = normalize_z80_alt_af(assembly) else {
                return Err(original_error);
            };
            (parse_assembly_syntax(&source_name, &normalized)?, true)
        }
        Err(error) => return Err(error),
    };
    if !options.line_origins.is_empty() {
        remap_parsed_locations(&mut parsed, &options.line_origins);
    }
    let mut program = lower_parsed_assembly(parsed)?;
    if restore_z80_alt_af {
        restore_z80_alt_af_operands(&mut program);
    }
    Ok(program)
}

fn normalize_z80_alt_af(source: &str) -> Option<String> {
    let mut normalized = String::with_capacity(source.len());
    let mut index = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut changed = false;

    while index < source.len() {
        let remaining = &source[index..];
        let ch = remaining.chars().next()?;
        if let Some(delimiter) = quote {
            normalized.push(ch);
            index += ch.len_utf8();
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            normalized.push(ch);
            index += ch.len_utf8();
            continue;
        }

        let is_alt_af = remaining
            .get(..3)
            .is_some_and(|token| token.eq_ignore_ascii_case("af'"));
        let previous = source[..index].chars().next_back();
        let next = source.get(index + 3..).and_then(|tail| tail.chars().next());
        if is_alt_af
            && previous.is_none_or(|ch| !is_assembly_symbol_char(ch))
            && next.is_none_or(|ch| !is_assembly_symbol_char(ch))
        {
            normalized.push_str("af?");
            index += 3;
            changed = true;
            continue;
        }

        normalized.push(ch);
        index += ch.len_utf8();
    }

    changed.then_some(normalized)
}

fn is_assembly_symbol_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '@' | '?' | '%')
}

fn restore_z80_alt_af_operands(program: &mut AssemblyProgram) {
    for item in &mut program.items {
        let AssemblyItem::Instruction(instruction) = &mut item.kind else {
            continue;
        };
        for operand in &mut instruction.operands {
            if operand.eq_ignore_ascii_case("af?") {
                *operand = "af'".to_owned();
            }
        }
    }
}

fn remap_parsed_locations(parsed: &mut ParsedAssembly, origins: &[SourceLocation]) {
    fn remap_items(items: &mut [LocatedParsedAssemblyItem], origins: &[SourceLocation]) {
        for item in items {
            let parsed_location = item.location.clone();
            if let Some(origin) = origins.get(parsed_location.line.saturating_sub(1)) {
                item.location = SourceLocation {
                    file: origin.file.clone(),
                    line: origin.line,
                    column: origin
                        .column
                        .saturating_add(parsed_location.column.saturating_sub(1)),
                };
            }
            match &mut item.kind {
                ParsedAssemblyItem::MacroDefinition { body, .. } => remap_items(body, origins),
                ParsedAssemblyItem::Conditional {
                    then_items,
                    else_items,
                    ..
                } => {
                    remap_items(then_items, origins);
                    remap_items(else_items, origins);
                }
                _ => {}
            }
        }
    }

    remap_items(&mut parsed.items, origins);
}

fn item_diagnostic(item: &LocatedAssemblyItem, message: impl Into<String>) -> Diagnostic {
    Diagnostic::at(item.location.clone(), message)
}

fn parse_program_instructions(
    cpu: AssemblerCpu,
    program: &AssemblyProgram,
) -> Result<Vec<Option<ArchitectureInstruction>>, Diagnostic> {
    program
        .items
        .iter()
        .map(|item| match &item.kind {
            AssemblyItem::Instruction(instruction) => parse_instruction(cpu, instruction)
                .map(Some)
                .map_err(|error| error.with_location_if_missing(item.location.clone())),
            _ => Ok(None),
        })
        .collect()
}

fn section_base(options: &AssemblerSourceOptions, name: &str) -> Option<u32> {
    options
        .section_bases
        .iter()
        .find(|section| section.name == name)
        .map(|section| section.addr)
}

pub fn measure_assembly(cpu: AssemblerCpu, assembly: &str) -> Result<usize, Diagnostic> {
    measure_assembly_with_options(cpu, assembly, &AssemblerSourceOptions::default())
}

pub fn measure_assembly_with_options(
    cpu: AssemblerCpu,
    assembly: &str,
    options: &AssemblerSourceOptions,
) -> Result<usize, Diagnostic> {
    let program = parse_assembly_program(cpu, assembly, options)?;
    measure_assembly_program_with_options(cpu, &program, options)
}

pub fn measure_assembly_program(
    cpu: AssemblerCpu,
    program: &AssemblyProgram,
) -> Result<usize, Diagnostic> {
    measure_assembly_program_with_options(cpu, program, &AssemblerSourceOptions::default())
}

pub fn measure_assembly_program_with_options(
    cpu: AssemblerCpu,
    program: &AssemblyProgram,
    _options: &AssemblerSourceOptions,
) -> Result<usize, Diagnostic> {
    let mut len = 0usize;
    for item in &program.items {
        let item_len = match &item.kind {
            AssemblyItem::Data { width, values } => data_len(*width, values),
            AssemblyItem::Instruction(instruction) => {
                let architecture = parse_instruction(cpu, instruction)
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?;
                instruction_len(cpu, &architecture, &instruction.to_text())
                    .map_err(|error| error.with_location_if_missing(item.location.clone()))?
            }
            AssemblyItem::Label(_)
            | AssemblyItem::Equ { .. }
            | AssemblyItem::Section(_)
            | AssemblyItem::Org(_) => 0,
        };
        len = len
            .checked_add(item_len)
            .ok_or_else(|| Diagnostic::new("assembly size exceeds host addressable memory"))?;
    }
    Ok(len)
}

fn checked_assembly_pc_advance(pc: u32, len: u32) -> Result<u32, Diagnostic> {
    let end = pc
        .checked_add(len)
        .ok_or_else(|| Diagnostic::new("assembly exceeds the 24-bit address space"))?;
    if end > Address24::MAX + 1 {
        return Err(Diagnostic::new(format!(
            "assembly instruction at 0x{pc:06X} with length 0x{len:X} exceeds the 24-bit address space"
        )));
    }
    Ok(end)
}

#[cfg(feature = "test-runner")]
fn checked_code_end(base_addr: u32, len: usize) -> Result<u32, Diagnostic> {
    let len = u32::try_from(len)
        .map_err(|_| Diagnostic::new("test program exceeds the 24-bit address space"))?;
    let end = base_addr
        .checked_add(len)
        .ok_or_else(|| Diagnostic::new("test program exceeds the 24-bit address space"))?;
    if end > Address24::MAX + 1 {
        return Err(Diagnostic::new(format!(
            "test program at 0x{base_addr:06X} with length 0x{len:X} exceeds the 24-bit address space"
        )));
    }
    Ok(end)
}

fn data_len(width: DataWidth, values: &[AssemblyDataValue]) -> usize {
    values
        .iter()
        .map(|value| match value {
            AssemblyDataValue::Expression(_) => width.bytes(),
            AssemblyDataValue::Bytes(bytes) => bytes.len(),
        })
        .sum()
}

fn emit_data(
    cpu: AssemblerCpu,
    width: DataWidth,
    values: &[AssemblyDataValue],
    labels: &HashMap<String, u32>,
    pc: u32,
    bytes: &mut Vec<u8>,
) -> Result<(), Diagnostic> {
    for value in values {
        match value {
            AssemblyDataValue::Bytes(raw) => bytes.extend(raw),
            AssemblyDataValue::Expression(expression) => {
                let value = eval_assembly_expression(expression, labels, pc)?;
                match width {
                    DataWidth::Byte => {
                        let value = u8::try_from(value).map_err(|_| {
                            Diagnostic::new(format!(
                                "value {} is outside u8 range",
                                expression_text(expression)
                            ))
                        })?;
                        bytes.push(value);
                    }
                    DataWidth::Word if cpu == AssemblerCpu::Tms9900 => {
                        let value = u16::try_from(value).map_err(|_| {
                            Diagnostic::new(format!(
                                "TMS9900 word value 0x{value:X} is outside the 16-bit address space"
                            ))
                        })?;
                        bytes.extend(value.to_be_bytes());
                    }
                    DataWidth::Word => push16(bytes, value)?,
                }
            }
        }
    }
    Ok(())
}

fn append_org_padding(bytes: &mut Vec<u8>, pc: u32, new_pc: u32) -> Result<(), Diagnostic> {
    if new_pc < pc {
        return Err(Diagnostic::new(format!(
            "org target 0x{new_pc:06X} is before current address 0x{pc:06X}"
        )));
    }
    let padding = usize::try_from(new_pc - pc)
        .map_err(|_| Diagnostic::new("org padding exceeds host addressable memory"))?;
    bytes.resize(bytes.len() + padding, 0);
    Ok(())
}

fn eval_instruction_expression(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<u32, Diagnostic> {
    let expression = match parse_assembly_expression(text) {
        Ok(expression) => expression,
        Err(error) => {
            let text = text.trim();
            if looks_like_label_ref(text)
                && let Some(value) = labels.get(text).copied().or_else(|| {
                    labels
                        .iter()
                        .find_map(|(name, value)| name.eq_ignore_ascii_case(text).then_some(*value))
                })
            {
                return Ok(value);
            }
            if text.starts_with(|ch: char| ch.is_ascii_digit())
                && let Err(error) = parse_number(text)
            {
                return Err(error);
            }
            return Err(error);
        }
    };
    eval_assembly_expression(&expression, labels, pc)
}

fn eval_assembly_expression(
    expression: &AssemblyExpression,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<u32, Diagnostic> {
    let value = eval_expression_value(expression, labels, pc)?;
    if !(0..=i128::from(Address24::MAX)).contains(&value) {
        return Err(Diagnostic::new(format!(
            "assembly expression `{}` is outside the 24-bit address space",
            expression_text(expression)
        )));
    }
    Ok(value as u32)
}

fn eval_expression_value(
    expression: &AssemblyExpression,
    labels: &HashMap<String, u32>,
    pc: u32,
) -> Result<i128, Diagnostic> {
    match expression {
        AssemblyExpression::Symbol(name) => labels
            .get(name)
            .copied()
            .or_else(|| {
                labels
                    .iter()
                    .find_map(|(known, value)| known.eq_ignore_ascii_case(name).then_some(*value))
            })
            .map(i128::from)
            .ok_or_else(|| Diagnostic::new(format!("unknown assembly symbol `{name}`"))),
        AssemblyExpression::Current => Ok(i128::from(pc & 0xFF_FFFF)),
        AssemblyExpression::Number(value) => Ok(i128::from(*value)),
        AssemblyExpression::Unary {
            operator,
            expression,
        } => {
            let value = eval_expression_value(expression, labels, pc)?;
            match operator {
                AssemblyUnaryOperator::Plus => Ok(value),
                AssemblyUnaryOperator::Negate => value
                    .checked_neg()
                    .ok_or_else(|| expression_range_diagnostic(expression)),
            }
        }
        AssemblyExpression::Binary {
            operator,
            left,
            right,
        } => {
            let left_value = eval_expression_value(left, labels, pc)?;
            let right_value = eval_expression_value(right, labels, pc)?;
            let value = match operator {
                AssemblyBinaryOperator::Add => left_value.checked_add(right_value),
                AssemblyBinaryOperator::Subtract => left_value.checked_sub(right_value),
                AssemblyBinaryOperator::Multiply => left_value.checked_mul(right_value),
                AssemblyBinaryOperator::BitAnd => Some(left_value & right_value),
                AssemblyBinaryOperator::BitOr => Some(left_value | right_value),
                AssemblyBinaryOperator::BitXor => Some(left_value ^ right_value),
            };
            value.ok_or_else(|| expression_range_diagnostic(expression))
        }
    }
}

fn expression_range_diagnostic(expression: &AssemblyExpression) -> Diagnostic {
    Diagnostic::new(format!(
        "assembly expression `{}` is outside the 24-bit address space",
        expression_text(expression)
    ))
}

fn expression_text(expression: &AssemblyExpression) -> String {
    match expression {
        AssemblyExpression::Symbol(name) => name.clone(),
        AssemblyExpression::Current => "$".to_owned(),
        AssemblyExpression::Number(value) => value.to_string(),
        AssemblyExpression::Unary {
            operator,
            expression,
        } => format!(
            "{}{}",
            match operator {
                AssemblyUnaryOperator::Plus => "+",
                AssemblyUnaryOperator::Negate => "-",
            },
            expression_text(expression)
        ),
        AssemblyExpression::Binary {
            operator,
            left,
            right,
        } => format!(
            "{} {} {}",
            expression_text(left),
            match operator {
                AssemblyBinaryOperator::Add => "+",
                AssemblyBinaryOperator::Subtract => "-",
                AssemblyBinaryOperator::Multiply => "*",
                AssemblyBinaryOperator::BitAnd => "&",
                AssemblyBinaryOperator::BitOr => "|",
                AssemblyBinaryOperator::BitXor => "^",
            },
            expression_text(right)
        ),
    }
}

fn instruction_len(
    cpu: AssemblerCpu,
    architecture: &ArchitectureInstruction,
    diagnostic_text: &str,
) -> Result<usize, Diagnostic> {
    let instruction = architecture.instruction();
    if cpu == AssemblerCpu::Lr35902 {
        return Ok(encode_lr35902(instruction, &HashMap::new(), 0, false)?.len());
    }
    let text = architecture.encoder_text();
    #[cfg(any(feature = "std", feature = "avr"))]
    if cpu == AssemblerCpu::Avr {
        return avr::instruction_len(text);
    }
    #[cfg(any(feature = "std", feature = "m6800"))]
    if cpu == AssemblerCpu::M6800 {
        return m6800::instruction_len(text)?.ok_or_else(|| {
            Diagnostic::new(format!(
                "assembler does not support M6800 instruction `{diagnostic_text}`"
            ))
        });
    }
    #[cfg(feature = "m68k")]
    if cpu == AssemblerCpu::M68k {
        return asm_m68k::instruction_len(text);
    }
    #[cfg(any(feature = "std", feature = "mos6502"))]
    if let Some(variant) = mos6502_variant(cpu) {
        return crate::asm::mos6502::instruction_len_for_variant(text, variant);
    }
    #[cfg(feature = "dcpu")]
    if cpu == AssemblerCpu::Dcpu {
        return dcpu::instruction_len(text);
    }
    #[cfg(feature = "tms9900")]
    if cpu == AssemblerCpu::Tms9900 {
        return tms9900::instruction_len(text);
    }
    let normalized = asm_meta::normalize_z80_instruction_text(text);
    let text = normalized.as_str();
    if let Some((opcode, _)) = z80_imm16_load(cpu, instruction) {
        let prefix_len = usize::from(opcode == 0xDD || opcode == 0xFD);
        return Ok(prefix_len + 3);
    }
    asm_meta::generated_instruction_len(cpu, text)?.ok_or_else(|| {
        Diagnostic::new(format!(
            "test assembler does not support instruction `{diagnostic_text}`"
        ))
    })
}

fn emit_instruction(
    cpu: AssemblerCpu,
    architecture: &ArchitectureInstruction,
    diagnostic_text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    bytes: &mut Vec<u8>,
) -> Result<(), Diagnostic> {
    let instruction = architecture.instruction();
    if cpu == AssemblerCpu::Lr35902 {
        bytes.extend(encode_lr35902(instruction, labels, pc, true)?);
        return Ok(());
    }
    let text = architecture.encoder_text();
    #[cfg(any(feature = "std", feature = "avr"))]
    if cpu == AssemblerCpu::Avr {
        bytes.extend(avr::encode_instruction(text, labels, pc)?);
        return Ok(());
    }
    #[cfg(any(feature = "std", feature = "m6800"))]
    if cpu == AssemblerCpu::M6800 {
        let Some(encoded) = m6800::emit_instruction(text, labels, pc)? else {
            return Err(Diagnostic::new(format!(
                "assembler does not support M6800 instruction `{diagnostic_text}`"
            )));
        };
        bytes.extend(encoded);
        return Ok(());
    }
    #[cfg(feature = "m68k")]
    if cpu == AssemblerCpu::M68k {
        bytes.extend(asm_m68k::encode(text, labels, pc, true)?);
        return Ok(());
    }
    #[cfg(any(feature = "std", feature = "mos6502"))]
    if let Some(variant) = mos6502_variant(cpu) {
        bytes.extend(crate::asm::mos6502::encode_instruction_for_variant(
            text, labels, pc, true, variant,
        )?);
        return Ok(());
    }
    #[cfg(feature = "dcpu")]
    if cpu == AssemblerCpu::Dcpu {
        bytes.extend(dcpu::encode_instruction(text, labels, pc)?);
        return Ok(());
    }
    #[cfg(feature = "tms9900")]
    if cpu == AssemblerCpu::Tms9900 {
        bytes.extend(tms9900::encode_instruction(text, labels, pc)?);
        return Ok(());
    }
    let normalized = asm_meta::normalize_z80_instruction_text(text);
    let text = normalized.as_str();
    if let Some((opcode, value)) = z80_imm16_load(cpu, instruction) {
        if opcode == 0xDD || opcode == 0xFD {
            bytes.push(opcode);
            bytes.push(0x21);
        } else {
            bytes.push(opcode);
        }
        push16(bytes, parse_addr(value, labels, pc)?)?;
    } else if let Some((prefix, _)) = asm_meta::ez80_mode_suffixed_instruction(cpu, text) {
        let (mnemonic, _) = instruction.mnemonic.rsplit_once('.').ok_or_else(|| {
            Diagnostic::new(format!("invalid eZ80 mode-suffixed instruction `{text}`"))
        })?;
        let base = architecture.with_mnemonic(mnemonic);
        bytes.push(prefix);
        emit_instruction(cpu, &base, diagnostic_text, labels, pc + 1, bytes)?;
    } else if let Some(generated) = asm_meta::encode_generated_instruction(cpu, text)? {
        bytes.extend(generated);
    } else if let Some(branch) = asm_meta::branch_instruction(cpu, text) {
        bytes.push(branch.opcode);
        let target = parse_addr(branch.target, labels, pc)?;
        match branch.width {
            asm_meta::BranchWidth::Relative8 => bytes.push(relative_offset(pc, target)?),
            asm_meta::BranchWidth::Absolute16 => push16(bytes, target)?,
            asm_meta::BranchWidth::Absolute24 => push24(bytes, target),
        }
    } else if !cpu.supports_z80_syntax() {
        return Err(Diagnostic::new(format!(
            "test assembler does not support instruction `{diagnostic_text}`"
        )));
    } else if let Some(direct) = asm_meta::direct24_instruction(cpu, text) {
        bytes.extend_from_slice(direct.prefix);
        push24(bytes, parse_addr(direct.addr, labels, pc)?);
    } else if let Some(load) = asm_meta::imm24_load_instruction(cpu, text) {
        bytes.extend_from_slice(load.prefix);
        push24(bytes, parse_addr(load.value, labels, pc)?);
    } else {
        return Err(Diagnostic::new(format!(
            "test assembler does not support instruction `{diagnostic_text}`"
        )));
    }
    Ok(())
}

fn z80_imm16_load(cpu: AssemblerCpu, instruction: &AssemblyInstruction) -> Option<(u8, &str)> {
    if !matches!(
        cpu,
        AssemblerCpu::Z80 | AssemblerCpu::Z80N | AssemblerCpu::Z180
    ) || !instruction.mnemonic.eq_ignore_ascii_case("ld")
        || instruction.operands.len() != 2
    {
        return None;
    }
    let destination = instruction.operands[0].trim();
    let opcode = if destination.eq_ignore_ascii_case("bc") {
        0x01
    } else if destination.eq_ignore_ascii_case("de") {
        0x11
    } else if destination.eq_ignore_ascii_case("hl") {
        0x21
    } else if destination.eq_ignore_ascii_case("sp") {
        0x31
    } else if destination.eq_ignore_ascii_case("ix") {
        0xDD
    } else if destination.eq_ignore_ascii_case("iy") {
        0xFD
    } else {
        return None;
    };
    let value = instruction.operands[1].trim();
    let normalized_value = value.to_ascii_lowercase();
    if value.starts_with('(')
        || matches!(
            normalized_value.as_str(),
            "a" | "b" | "c" | "d" | "e" | "h" | "l" | "bc" | "de" | "hl" | "sp" | "ix" | "iy"
        )
    {
        return None;
    }
    Some((opcode, value))
}

#[cfg(any(feature = "std", feature = "mos6502"))]
fn mos6502_variant(cpu: AssemblerCpu) -> Option<Mos6502Variant> {
    match cpu {
        AssemblerCpu::Mos6502 => Some(Mos6502Variant::Nmos6502),
        AssemblerCpu::Cmos65C02 => Some(Mos6502Variant::Cmos65C02),
        AssemblerCpu::Wdc65C816 => Some(Mos6502Variant::Wdc65C816),
        AssemblerCpu::Ricoh2A03 => Some(Mos6502Variant::Ricoh2A03),
        _ => None,
    }
}
fn encode_lr35902(
    instruction: &AssemblyInstruction,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let text = instruction.to_text().trim().to_ascii_lowercase();
    let operation = instruction.mnemonic.trim().to_ascii_lowercase();
    let operand = instruction
        .operands
        .iter()
        .map(|operand| operand.trim().to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(", ");
    let text = text.as_str();
    let operation = operation.as_str();
    let operand = operand.as_str();
    let fixed: Option<&[u8]> = match text {
        "nop" => Some(&[0x00][..]),
        "rlca" => Some(&[0x07]),
        "rrca" => Some(&[0x0F]),
        "stop" => Some(&[0x10, 0x00]),
        "rla" => Some(&[0x17]),
        "rra" => Some(&[0x1F]),
        "daa" => Some(&[0x27]),
        "cpl" => Some(&[0x2F]),
        "scf" => Some(&[0x37]),
        "ccf" => Some(&[0x3F]),
        "halt" => Some(&[0x76]),
        "ret" => Some(&[0xC9]),
        "reti" => Some(&[0xD9]),
        "jp hl" | "jp (hl)" => Some(&[0xE9]),
        "di" => Some(&[0xF3]),
        "ld sp, hl" => Some(&[0xF9]),
        "ei" => Some(&[0xFB]),
        _ => None,
    };
    if let Some(bytes) = fixed {
        return Ok(bytes.to_vec());
    }

    if !operand.is_empty() {
        if let Some(code) = lr_cb_operation(operation, operand)? {
            return Ok(vec![0xCB, code]);
        }
        if let Some(code) = lr_inc_dec(operation, operand) {
            return Ok(vec![code]);
        }
        if operation == "ld" {
            return encode_lr_load(operand, labels, pc, resolve);
        }
        if operation == "ldi" {
            return encode_lr_load(&operand.replace("(hl)", "(hl+)"), labels, pc, resolve);
        }
        if operation == "ldd" {
            return encode_lr_load(&operand.replace("(hl)", "(hl-)"), labels, pc, resolve);
        }
        if operation == "ldh" {
            let Some((dst, src)) = operand.split_once(',') else {
                return Err(Diagnostic::new(format!("invalid ldh syntax `{operand}`")));
            };
            let (dst, src) = (dst.trim(), src.trim());
            if dst == "(c)" && src == "a" {
                return Ok(vec![0xE2]);
            }
            if dst == "a" && src == "(c)" {
                return Ok(vec![0xF2]);
            }
            let (opcode, value) = if dst.starts_with('(') && src == "a" {
                (0xE0, &dst[1..dst.len() - 1])
            } else if dst == "a" && src.starts_with('(') {
                (0xF0, &src[1..src.len() - 1])
            } else {
                return Err(Diagnostic::new(format!("invalid ldh syntax `{operand}`")));
            };
            let address = lr_value(value, labels, pc, resolve)?;
            let offset = if address >= 0xFF00 {
                address - 0xFF00
            } else {
                address
            };
            let offset = u8::try_from(offset).map_err(|_| {
                Diagnostic::new(format!("LDH address `{value}` is outside FF00h..FFFFh"))
            })?;
            return Ok(vec![opcode, offset]);
        }
        if let Some(bytes) = encode_lr_alu(operation, operand, labels, pc, resolve)? {
            return Ok(bytes);
        }
        if let Some(bytes) = encode_lr_control(operation, operand, labels, pc, resolve)? {
            return Ok(bytes);
        }
        if matches!(operation, "push" | "pop") {
            let register = lr_stack_register(operand).ok_or_else(|| {
                Diagnostic::new(format!("invalid LR35902 stack register `{operand}`"))
            })?;
            return Ok(vec![
                if operation == "push" { 0xC5 } else { 0xC1 } + register * 0x10,
            ]);
        }
    }
    Err(Diagnostic::new(format!(
        "assembler does not support LR35902 instruction `{text}`"
    )))
}

fn lr_r8(value: &str) -> Option<u8> {
    match value.trim() {
        "b" => Some(0),
        "c" => Some(1),
        "d" => Some(2),
        "e" => Some(3),
        "h" => Some(4),
        "l" => Some(5),
        "(hl)" => Some(6),
        "a" => Some(7),
        _ => None,
    }
}

fn lr_r16(value: &str) -> Option<u8> {
    match value.trim() {
        "bc" => Some(0),
        "de" => Some(1),
        "hl" => Some(2),
        "sp" => Some(3),
        _ => None,
    }
}

fn lr_stack_register(value: &str) -> Option<u8> {
    match value.trim() {
        "bc" => Some(0),
        "de" => Some(1),
        "hl" => Some(2),
        "af" => Some(3),
        _ => None,
    }
}

fn lr_condition(value: &str) -> Option<u8> {
    match value.trim() {
        "nz" => Some(0),
        "z" => Some(1),
        "nc" => Some(2),
        "c" => Some(3),
        _ => None,
    }
}

fn lr_cb_operation(operation: &str, operand: &str) -> Result<Option<u8>, Diagnostic> {
    if matches!(operation, "bit" | "res" | "set") {
        let Some((bit, register)) = operand.split_once(',') else {
            return Ok(None);
        };
        let bit = bit
            .trim()
            .parse::<u8>()
            .map_err(|_| Diagnostic::new(format!("invalid bit index `{bit}`")))?;
        if bit > 7 {
            return Err(Diagnostic::new(format!("bit index {bit} is outside 0..7")));
        }
        let register = lr_r8(register)
            .ok_or_else(|| Diagnostic::new(format!("invalid LR35902 register `{register}`")))?;
        let base = match operation {
            "bit" => 0x40,
            "res" => 0x80,
            _ => 0xC0,
        };
        return Ok(Some(base + bit * 8 + register));
    }
    let row = match operation {
        "rlc" => 0,
        "rrc" => 1,
        "rl" => 2,
        "rr" => 3,
        "sla" => 4,
        "sra" => 5,
        "swap" => 6,
        "srl" => 7,
        _ => return Ok(None),
    };
    let register = lr_r8(operand)
        .ok_or_else(|| Diagnostic::new(format!("invalid LR35902 register `{operand}`")))?;
    Ok(Some(row * 8 + register))
}

fn lr_inc_dec(operation: &str, operand: &str) -> Option<u8> {
    let increment = operation == "inc";
    if !increment && operation != "dec" {
        return None;
    }
    if let Some(register) = lr_r8(operand) {
        return Some((if increment { 0x04 } else { 0x05 }) + register * 8);
    }
    lr_r16(operand).map(|register| (if increment { 0x03 } else { 0x0B }) + register * 0x10)
}

fn lr_value(
    expr: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<u32, Diagnostic> {
    if resolve {
        eval_instruction_expression(expr.trim(), labels, pc)
    } else {
        Ok(0)
    }
}

fn lr_u8(
    expr: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<u8, Diagnostic> {
    let value = lr_value(expr, labels, pc, resolve)?;
    u8::try_from(value).map_err(|_| Diagnostic::new(format!("value {expr} is outside u8 range")))
}

fn lr_u16(
    expr: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<[u8; 2], Diagnostic> {
    let value = lr_value(expr, labels, pc, resolve)?;
    let value = u16::try_from(value)
        .map_err(|_| Diagnostic::new(format!("value {expr} is outside u16 range")))?;
    Ok(value.to_le_bytes())
}

fn encode_lr_load(
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let Some((dst, src)) = operand.split_once(',') else {
        return Err(Diagnostic::new(format!("invalid ld syntax `{operand}`")));
    };
    let (dst, src) = (dst.trim(), src.trim());
    if let (Some(dst), Some(src)) = (lr_r8(dst), lr_r8(src)) {
        if dst == 6 && src == 6 {
            return Err(Diagnostic::new("use `halt`, not `ld (hl), (hl)`"));
        }
        return Ok(vec![0x40 + dst * 8 + src]);
    }
    if let Some(dst) = lr_r8(dst)
        && !src.starts_with('(')
    {
        let value = lr_u8(src, labels, pc, resolve)?;
        return Ok(vec![0x06 + dst * 8, value]);
    }
    if dst == "hl" && (src.starts_with("sp+") || src.starts_with("sp-")) {
        return Ok(vec![0xF8, parse_lr_signed(&src[2..])? as u8]);
    }
    if let Some(register) = lr_r16(dst) {
        if dst == "sp" && src == "hl" {
            return Ok(vec![0xF9]);
        }
        let value = lr_u16(src, labels, pc, resolve)?;
        return Ok(vec![0x01 + register * 0x10, value[0], value[1]]);
    }
    let indirect = match (dst, src) {
        ("(bc)", "a") => Some(0x02),
        ("(de)", "a") => Some(0x12),
        ("(hl+)", "a") | ("(hli)", "a") => Some(0x22),
        ("(hl-)", "a") | ("(hld)", "a") => Some(0x32),
        ("a", "(bc)") => Some(0x0A),
        ("a", "(de)") => Some(0x1A),
        ("a", "(hl+)") | ("a", "(hli)") => Some(0x2A),
        ("a", "(hl-)") | ("a", "(hld)") => Some(0x3A),
        _ => None,
    };
    if let Some(opcode) = indirect {
        return Ok(vec![opcode]);
    }
    if dst == "(c)" && src == "a" {
        return Ok(vec![0xE2]);
    }
    if dst == "a" && src == "(c)" {
        return Ok(vec![0xF2]);
    }
    if dst.starts_with('(') && dst.ends_with(')') {
        let address = &dst[1..dst.len() - 1];
        let value = lr_u16(address, labels, pc, resolve)?;
        return match src {
            "a" => Ok(vec![0xEA, value[0], value[1]]),
            "sp" => Ok(vec![0x08, value[0], value[1]]),
            _ => Err(Diagnostic::new(format!("invalid LR35902 load `{operand}`"))),
        };
    }
    if dst == "a" && src.starts_with('(') && src.ends_with(')') {
        let value = lr_u16(&src[1..src.len() - 1], labels, pc, resolve)?;
        return Ok(vec![0xFA, value[0], value[1]]);
    }
    Err(Diagnostic::new(format!("invalid LR35902 load `{operand}`")))
}

fn encode_lr_alu(
    operation: &str,
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    if operation == "add" && operand.starts_with("hl,") {
        let register = lr_r16(operand[3..].trim())
            .ok_or_else(|| Diagnostic::new(format!("invalid add operand `{operand}`")))?;
        return Ok(Some(vec![0x09 + register * 0x10]));
    }
    if operation == "add" && operand.starts_with("sp,") {
        return Ok(Some(vec![
            0xE8,
            parse_lr_signed(operand[3..].trim())? as u8,
        ]));
    }
    let (row, source) = match operation {
        "add" => (0, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "adc" => (1, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "sub" => (2, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "sbc" => (3, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "and" => (4, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "xor" => (5, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "or" => (6, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "cp" => (7, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        _ => return Ok(None),
    };
    if let Some(register) = lr_r8(source) {
        return Ok(Some(vec![0x80 + row * 8 + register]));
    }
    Ok(Some(vec![
        0xC6 + row * 8,
        lr_u8(source, labels, pc, resolve)?,
    ]))
}

fn encode_lr_control(
    operation: &str,
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    if operation == "rst" {
        let vector = lr_u8(operand, labels, pc, resolve)?;
        if vector > 0x38 || vector % 8 != 0 {
            return Err(Diagnostic::new(
                "LR35902 restart vector must be 00h..38h in steps of 8",
            ));
        }
        return Ok(Some(vec![0xC7 + vector]));
    }
    if operation == "ret" {
        return Ok(lr_condition(operand).map(|condition| vec![0xC0 + condition * 8]));
    }
    if !matches!(operation, "jr" | "jp" | "call") {
        return Ok(None);
    }
    let (condition, target) = operand
        .split_once(',')
        .map_or((None, operand), |(condition, target)| {
            (lr_condition(condition), target.trim())
        });
    if operand.contains(',') && condition.is_none() {
        return Err(Diagnostic::new(format!(
            "invalid branch condition `{operand}`"
        )));
    }
    if operation == "jr" {
        let opcode = condition.map_or(0x18, |condition| 0x20 + condition * 8);
        let target = lr_value(target, labels, pc, resolve)?;
        let offset = if resolve {
            relative_offset(pc, target)?
        } else {
            0
        };
        return Ok(Some(vec![opcode, offset]));
    }
    let value = lr_u16(target, labels, pc, resolve)?;
    let opcode = match (operation, condition) {
        ("jp", None) => 0xC3,
        ("call", None) => 0xCD,
        ("jp", Some(c)) => 0xC2 + c * 8,
        ("call", Some(c)) => 0xC4 + c * 8,
        _ => unreachable!(),
    };
    Ok(Some(vec![opcode, value[0], value[1]]))
}

fn parse_lr_signed(text: &str) -> Result<i8, Diagnostic> {
    let text = text.trim();
    let expression = parse_assembly_expression(text)?;
    let value = eval_expression_value(&expression, &HashMap::new(), 0)?;
    i8::try_from(value)
        .map_err(|_| Diagnostic::new(format!("signed byte `{text}` is outside -128..127")))
}

fn push16(bytes: &mut Vec<u8>, value: u32) -> Result<(), Diagnostic> {
    if value > 0xFFFF {
        return Err(Diagnostic::new(format!(
            "address operand 0x{value:X} is outside the 16-bit address space"
        )));
    }
    bytes.push(value as u8);
    bytes.push((value >> 8) as u8);
    Ok(())
}

pub(crate) fn relative_offset(pc: u32, target: u32) -> Result<u8, Diagnostic> {
    let next_pc = (pc + 2) & 0xFF_FFFF;
    let offset = target as i64 - next_pc as i64;
    if !(-128..=127).contains(&offset) {
        return Err(Diagnostic::new(format!(
            "relative jump target 0x{target:06X} is out of range from 0x{pc:06X}"
        )));
    }
    Ok((offset as i8) as u8)
}

fn parse_addr(text: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u32, Diagnostic> {
    match eval_instruction_expression(text, labels, pc) {
        Ok(value) if value <= Address24::MAX => Ok(value),
        Ok(_) => Err(Diagnostic::new(format!(
            "address operand `{text}` is outside the 24-bit address space"
        ))),
        Err(_) if looks_like_label_ref(text) => {
            Err(Diagnostic::new(format!("unknown assembly label `{text}`")))
        }
        Err(error) if error.message.contains("outside the 24-bit address space") => {
            Err(Diagnostic::new(format!(
                "address operand `{text}` is outside the 24-bit address space"
            )))
        }
        Err(error) => Err(error),
    }
}

fn looks_like_label_ref(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '.' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '.' || ch.is_ascii_alphanumeric())
}

pub(crate) fn parse_number(text: &str) -> Result<u32, Diagnostic> {
    let text = text.trim().trim_end_matches(',');
    let parsed = if let Some(hex) = text.strip_prefix('>') {
        u32::from_str_radix(hex.strip_suffix('h').unwrap_or(hex), 16)
    } else if let Some(hex) = text.strip_prefix('$') {
        u32::from_str_radix(hex, 16)
    } else if let Some(binary) = text.strip_prefix('%') {
        u32::from_str_radix(binary, 2)
    } else if let Some(hex) = text.strip_suffix('h') {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else if let Some(binary) = text.strip_prefix("0b") {
        u32::from_str_radix(binary, 2)
    } else {
        text.parse()
    };
    parsed.map_err(|_| Diagnostic::new(format!("invalid numeric operand `{text}`")))
}

fn push24(bytes: &mut Vec<u8>, value: u32) {
    bytes.push(value as u8);
    bytes.push((value >> 8) as u8);
    bytes.push((value >> 16) as u8);
}

#[cfg(feature = "test-runner")]
struct TestMachine {
    memory: HashMap<u32, u8>,
    address_mask: u32,
    ports: [u8; 256],
    cycles: Cell<i64>,
    halted: bool,
    result_code: u8,
    debug_output: Vec<u8>,
    memory_test_abi: bool,
}

#[cfg(feature = "test-runner")]
impl TestMachine {
    fn new(address_limit: u32, memory_test_abi: bool) -> Self {
        Self {
            memory: HashMap::new(),
            address_mask: address_limit,
            ports: [0; 256],
            cycles: Cell::new(0),
            halted: false,
            result_code: 0,
            debug_output: Vec::new(),
            memory_test_abi,
        }
    }
}

#[cfg(feature = "test-runner")]
impl Machine for TestMachine {
    fn peek(&self, address: u32) -> u8 {
        self.memory
            .get(&(address & self.address_mask))
            .copied()
            .unwrap_or(0)
    }

    fn poke(&mut self, address: u32, value: u8) {
        let address = address & self.address_mask;
        self.memory.insert(address, value);
        if self.memory_test_abi {
            match address {
                0xFF80 => self.debug_output.push(value),
                0xFF81 => self.result_code = value,
                0xFF82 if value != 0 => self.halted = true,
                _ => {}
            }
        }
    }

    fn use_cycles(&self, cycles: i32) {
        self.cycles
            .set(self.cycles.get().wrapping_add(cycles as i64));
    }

    fn port_in(&mut self, address: u16) -> u8 {
        self.ports[address as usize & 0xFF]
    }

    fn port_out(&mut self, address: u16, value: u8) {
        let port = address as usize & 0xFF;
        self.ports[port] = value;
        if port == 0x0C {
            self.debug_output.push(value);
        }
        if port == 0x0D {
            self.result_code = value;
        }
        if port == 0x0E && value == 1 {
            self.halted = true;
        }
    }
}

#[cfg(test)]
mod architecture_operand_tests {
    use super::*;

    fn assert_spacing_assembles(cpu: AssemblerCpu, compact: &str, spaced: &str, base_addr: u32) {
        let compact_bytes = assemble_subset_with_symbols_at(cpu, compact, base_addr)
            .unwrap_or_else(|error| panic!("compact `{compact}` failed: {error}"))
            .bytes;
        let spaced_bytes = assemble_subset_with_symbols_at(cpu, spaced, base_addr)
            .unwrap_or_else(|error| panic!("spaced `{spaced}` failed: {error}"))
            .bytes;
        assert_eq!(compact_bytes, spaced_bytes, "{} emission", cpu.as_str());
        assert_eq!(
            measure_assembly(cpu, compact).unwrap(),
            measure_assembly(cpu, spaced).unwrap(),
            "{} sizing",
            cpu.as_str()
        );
    }

    #[cfg(feature = "z80")]
    #[test]
    fn z80_ez80_spacing_variants_assemble_equivalently() {
        for cpu in [AssemblerCpu::Z80, AssemblerCpu::Ez80] {
            assert_spacing_assembles(cpu, "rlc (ix+1)", "rlc ( ix    + 1 )", 0x100);
        }
    }

    #[cfg(feature = "intel")]
    #[test]
    fn intel_spacing_variants_assemble_equivalently() {
        for cpu in [AssemblerCpu::I8080, AssemblerCpu::I8085] {
            assert_spacing_assembles(cpu, "lxi h,1234h", "lxi h    ,    1234h", 0x100);
        }
    }

    #[cfg(feature = "lr35902")]
    #[test]
    fn lr35902_spacing_variants_assemble_equivalently() {
        assert_spacing_assembles(
            AssemblerCpu::Lr35902,
            "ld hl,sp+1",
            "ld hl , sp    + 1",
            0x150,
        );
    }

    #[cfg(feature = "avr")]
    #[test]
    fn avr_spacing_variants_assemble_equivalently() {
        assert_spacing_assembles(AssemblerCpu::Avr, "ldd r1,y+1", "ldd r1 , y    + 1", 0);
    }

    #[cfg(feature = "dcpu")]
    #[test]
    fn dcpu_spacing_variants_assemble_equivalently() {
        assert_spacing_assembles(
            AssemblerCpu::Dcpu,
            "set a,[sp+1]\nset b,pick 1",
            "set a , [ sp    + 1 ]\nset b , pick    1",
            0,
        );
    }

    #[cfg(feature = "m6800")]
    #[test]
    fn m6800_spacing_variants_assemble_equivalently() {
        assert_spacing_assembles(AssemblerCpu::M6800, "ldaa $+2", "ldaa $    + 2", 0x1000);
    }

    #[cfg(feature = "m68k")]
    #[test]
    fn m68k_nested_comma_spacing_variants_assemble_equivalently() {
        assert_spacing_assembles(
            AssemblerCpu::M68k,
            "move.w (4,a0,d0.w),d1",
            "move.w ( 4 , a0 , d0.w ) , d1",
            0x1000,
        );
    }

    #[cfg(feature = "mos6502")]
    #[test]
    fn mos6502_spacing_variants_assemble_equivalently() {
        assert_spacing_assembles(
            AssemblerCpu::Mos6502,
            "lda ($20),y",
            "lda ( $20 ) , y",
            0xc000,
        );
    }

    #[cfg(feature = "tms9900")]
    #[test]
    fn tms9900_spacing_variants_assemble_equivalently() {
        assert_spacing_assembles(
            AssemblerCpu::Tms9900,
            "a @>8300(r4),r5",
            "a @>8300 ( r4 ) , r5",
            0x1000,
        );
    }
}

#[cfg(all(test, feature = "test-runner"))]
mod tests;
