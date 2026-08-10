#![cfg(all(feature = "std", feature = "msp430"))]

use std::path::Path;

use ezra::{
    api::{
        BuildRequest, CompileRequest, Workspace, WorkspaceFile, build_workspace,
        link_assembly_program,
    },
    asm::{AssemblyPreprocessOptions, preprocess_assembly_source},
};
use mspsim::{CpuVariant, Elf32, TestOutcome, TestRunner, TestRunnerOptions, TestRunnerResult};

const TARGET: &str = "msp430-none-elf";
const TEST_PORT: u32 = 0xff00;

fn run_msp430_elf(executable: &[u8], variant: CpuVariant) -> TestRunnerResult {
    let image = Elf32::parse(executable).expect("compiler output should be a valid ELF32 image");
    assert_eq!(image.machine, Some(mspsim::EM_MSP430));
    assert_eq!(image.inferred_variant(), Some(variant));

    TestRunner::new(TestRunnerOptions {
        cpu: Some(variant),
        max_instructions: 256,
        test_port: TEST_PORT,
        ..Default::default()
    })
    .run_image(&image)
    .expect("MSP430 ELF should execute in mspsim")
}

fn assert_test_output(result: &TestRunnerResult) {
    assert_eq!(result.outcome, TestOutcome::Passed, "{result:?}");
    assert_eq!(result.lines, ["EXIT"]);
}

#[test]
fn compiles_packages_and_executes_msp430_source_elf() {
    let source = r##"
        fn main() {
            asm volatile(clobber memory) {
                "mov.b #0x45, &0xff00"
                "mov.b #0x58, &0xff00"
                "mov.b #0x49, &0xff00"
                "mov.b #0x54, &0xff00"
                "mov.b #0x0a, &0xff00"
            }
        }
    "##;
    let files = [WorkspaceFile::text("main.ezra", source)];
    let build = build_workspace(
        &Workspace::new(&files),
        "main.ezra",
        &CompileRequest::new("main.ezra", TARGET),
    )
    .expect("MSP430 source should compile and package");

    assert_eq!(build.executable_extension, "elf");
    assert!(build.assembly.contains("section .text"));
    assert!(!build.machine_code.is_empty());

    let result = run_msp430_elf(&build.executable, CpuVariant::Msp430);
    assert_test_output(&result);
}

#[test]
fn compiles_and_executes_msp430x_20_bit_u20_and_pointer_behavior() {
    let source = r##"
        global high_pointer: ptr<u8> = cast<ptr<u8>>(0xABCDEu20)
        global high_word_pointer: ptr<u20> = cast<ptr<u20>>(0x54320u20)
        global result: u20 = 0

        fn main() {
            let pointer: ptr<u8> = high_pointer
            *pointer = 0x5A
            let round_trip: u20 = cast<u20>(pointer)
            let word_pointer: ptr<u20> = high_word_pointer
            *word_pointer = 0x54321u20
            let word_round_trip: u20 = *word_pointer
            result = round_trip + 1u20
            if round_trip != 0xABCDEu20 {
                asm volatile(clobber memory) {
                    "mov.b #0x46, &0xff00"
                    "mov.b #0x41, &0xff00"
                    "mov.b #0x49, &0xff00"
                    "mov.b #0x4C, &0xff00"
                    "mov.b #0x3A, &0xff00"
                    "mov.b #0x20, &0xff00"
                    "mov.b #0x50, &0xff00"
                    "mov.b #0x0a, &0xff00"
                }
            } else if *pointer != 0x5A {
                asm volatile(clobber memory) {
                    "mov.b #0x46, &0xff00"
                    "mov.b #0x41, &0xff00"
                    "mov.b #0x49, &0xff00"
                    "mov.b #0x4C, &0xff00"
                    "mov.b #0x3A, &0xff00"
                    "mov.b #0x20, &0xff00"
                    "mov.b #0x44, &0xff00"
                    "mov.b #0x0a, &0xff00"
                }
            } else if word_round_trip != 0x54321u20 {
                asm volatile(clobber memory) {
                    "mov.b #0x46, &0xff00"
                    "mov.b #0x41, &0xff00"
                    "mov.b #0x49, &0xff00"
                    "mov.b #0x4C, &0xff00"
                    "mov.b #0x3A, &0xff00"
                    "mov.b #0x20, &0xff00"
                    "mov.b #0x57, &0xff00"
                    "mov.b #0x0a, &0xff00"
                }
            } else if result != 0xABCDFu20 {
                asm volatile(clobber memory) {
                    "mov.b #0x46, &0xff00"
                    "mov.b #0x41, &0xff00"
                    "mov.b #0x49, &0xff00"
                    "mov.b #0x4C, &0xff00"
                    "mov.b #0x3A, &0xff00"
                    "mov.b #0x20, &0xff00"
                    "mov.b #0x55, &0xff00"
                    "mov.b #0x0a, &0xff00"
                }
            } else {
                asm volatile(clobber memory) {
                    "mov.b #0x45, &0xff00"
                    "mov.b #0x58, &0xff00"
                    "mov.b #0x49, &0xff00"
                    "mov.b #0x54, &0xff00"
                    "mov.b #0x0a, &0xff00"
                }
            }
        }
    "##;

    for (target, variant) in [
        ("msp430x-none-elf", CpuVariant::Msp430x),
        ("msp430x2-none-elf", CpuVariant::Msp430x2),
    ] {
        let files = [WorkspaceFile::text("main.ezra", source)];
        let build = build_workspace(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", target),
        )
        .unwrap_or_else(|error| panic!("{target} should compile and package: {error}"));

        let result = run_msp430_elf(&build.executable, variant);
        assert_eq!(
            result.outcome,
            TestOutcome::Passed,
            "{target}: {result:?}\n{}",
            build.assembly
        );
        assert_eq!(result.lines, ["EXIT"]);
    }
}

#[test]
fn assembles_packages_and_executes_msp430_assembly() {
    let assembly = r#"
        section .text
            mov.b #0x45, &0xff00
            mov.b #0x58, &0xff00
            mov.b #0x49, &0xff00
            mov.b #0x54, &0xff00
            mov.b #0x0a, &0xff00
        wait:
            jmp wait
    "#;
    let preprocessed = preprocess_assembly_source(
        "main.asm",
        assembly,
        AssemblyPreprocessOptions::for_compiled_features(TARGET, "msp430"),
    )
    .expect("MSP430 assembly should preprocess");
    let build = BuildRequest::for_target(TARGET).expect("MSP430 target should resolve");
    let linked = link_assembly_program(Path::new("main.asm"), &preprocessed.program, &build)
        .expect("MSP430 assembly should link and package");

    assert_eq!(linked.executable_extension, "elf");
    assert!(!linked.machine_code.is_empty());

    let result = run_msp430_elf(&linked.executable, CpuVariant::Msp430);
    assert_test_output(&result);
}
