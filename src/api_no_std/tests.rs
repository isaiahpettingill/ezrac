    use super::*;

    #[test]
    fn arbitrary_i8086_target_uses_a_16_bit_layout() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let request = CompileRequest::new("main.ezra", "custom-board-i8086");
        let build = build_workspace(&Workspace::new(&files), "main.ezra", &request).unwrap();
        let options = assembly_options_for_target(&request.target, CpuFamily::I8086, false, true);

        assert_eq!(options.entry_addr.get(), 0);
        assert_eq!(options.stack_top.get(), 0xFFFF);
        assert!(!build.machine_code.is_empty());
    }

    #[test]
    fn alloc_only_api_accepts_versioned_msdos_i8086_targets() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let request = CompileRequest::new("main.ezra", "msdos-com-i8086-6.22");
        let build = build_workspace(&Workspace::new(&files), "main.ezra", &request).unwrap();
        let options = assembly_options_for_target(&request.target, CpuFamily::I8086, false, true);

        assert_eq!(build.output_format, OutputFormat::CpmCom);
        assert_eq!(build.executable_extension, "com");
        assert!(options.dos_executable);
        assert!(build.assembly.contains("    int 0x21\n"));
    }

    #[test]
    fn compilation_strictly_validates_i8086_inline_assembly() {
        let files = [WorkspaceFile::text(
            "main.ezra",
            "fn main() { asm volatile { \"pusha\" } }",
        )];
        let error = compile_workspace_to_assembly(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", "bare-i8086"),
        )
        .unwrap_err();

        assert!(
            error
                .message
                .contains("assembler does not support 8086 instruction `pusha`"),
            "{error}"
        );
    }

    #[test]
    fn build_layout_validation_rejects_text_that_exceeds_its_region() {
        let layout = Layout::bare_16("i8086");
        let error = validate_text_section_fit(&layout, 0x8001).unwrap_err();

        assert_eq!(
            error.message,
            "section `.text` does not fit in region `code`"
        );
    }

    #[test]
    fn alloc_only_api_builds_raw_msdos_com_images() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let request = CompileRequest::new("main.ezra", "msdos-com-i8086");
        let build = build_workspace(&Workspace::new(&files), "main.ezra", &request).unwrap();
        let start = build
            .symbols
            .iter()
            .find(|symbol| symbol.name == "__ezra_start")
            .unwrap();

        assert_eq!(start.addr, 0x0100);
        assert_eq!(build.output_format, OutputFormat::CpmCom);
        assert_eq!(build.executable_extension, "com");
        assert_eq!(build.executable, build.machine_code);
        assert!(build.map.contains(".text"));
        assert!(build.assembly.contains("    int 0x21\n"));
    }

    #[cfg(feature = "mos6502")]
    #[test]
    fn alloc_only_api_rejects_nes_source_before_parsing() {
        let error = compile_source_to_assembly(
            "this is not valid EZRA source",
            &CompileRequest::new("main.ezra", "nes-2a03"),
        )
        .unwrap_err();

        assert!(error.message.contains("assembly-only"), "{error}");
    }

    #[test]
    fn alloc_only_api_links_preprocessed_assembly_with_a_map() {
        let build = BuildRequest::for_target("bare-i8086").unwrap();
        let preprocessed = crate::asm::preprocess_assembly_source(
            "main.asm",
            "start:\n    nop\n",
            crate::asm::AssemblyPreprocessOptions::for_compiled_features("bare-i8086", "i8086"),
        )
        .unwrap();
        let linked = link_assembly_program("main.asm", &preprocessed.program, &build).unwrap();

        assert_eq!(&linked.machine_code[..3], &[0x90]);
        assert_eq!(linked.executable, linked.machine_code);
        assert!(
            linked.map.contains("start        0x000000"),
            "{}",
            linked.map
        );
    }
