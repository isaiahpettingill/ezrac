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

fn run_msp430_elf(executable: &[u8]) -> TestRunnerResult {
    let image = Elf32::parse(executable).expect("compiler output should be a valid ELF32 image");
    assert_eq!(image.machine, Some(mspsim::EM_MSP430));
    assert_eq!(image.inferred_variant(), Some(CpuVariant::Msp430));

    TestRunner::new(TestRunnerOptions {
        cpu: Some(CpuVariant::Msp430),
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

    let result = run_msp430_elf(&build.executable);
    assert_test_output(&result);
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

    let result = run_msp430_elf(&linked.executable);
    assert_test_output(&result);
}
