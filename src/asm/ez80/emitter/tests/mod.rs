use std::path::Path;

use crate::{
    compile::load_program,
    parser::parse_program,
    vm::{TestRunOptions, run_assembly_test, run_assembly_test_with_options},
};

use super::*;

mod arithmetic_and_constants;
mod arrays_structs_and_pointers;
mod core_emission;
mod cpu_targets_and_mmio;
mod embeds_strings_debug_and_memory;
mod expression_validation;
mod functions_and_control_flow;
mod inline_asm;
mod inline_asm_and_abi;
mod modules_and_sdk;
mod optimization_and_constants;
mod type_validation;
