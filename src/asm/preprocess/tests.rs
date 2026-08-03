    use super::*;
    use crate::asm::frontend::AssemblyItem;
    use crate::workspace::WorkspaceFile;

    #[derive(Default)]
    struct MemoryResolver {
        files: BTreeMap<String, String>,
    }

    impl MemoryResolver {
        fn with(mut self, path: &str, source: &str) -> Self {
            self.files.insert(path.to_owned(), source.to_owned());
            self
        }
    }

    impl AssemblyIncludeResolver for MemoryResolver {
        fn resolve_include(
            &self,
            including_source_name: &str,
            include_path: &str,
        ) -> Result<ResolvedAssemblyInclude, Diagnostic> {
            let path = resolve_virtual_include_path(including_source_name, include_path);
            let source = self.files.get(&path).ok_or_else(|| {
                Diagnostic::new(format!("missing in-memory assembly include `{path}`"))
            })?;
            Ok(ResolvedAssemblyInclude::new(path, source.clone()))
        }
    }

    fn options() -> AssemblyPreprocessOptions {
        let mut options = AssemblyPreprocessOptions::new("agonlight-mos-ez80", "ez80");
        options.enabled_features.push("z80".to_owned());
        options
    }

    fn instruction_texts(preprocessed: &PreprocessedAssembly) -> Vec<String> {
        preprocessed
            .program
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                AssemblyItem::Instruction(instruction) => Some(instruction.to_text()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn expands_nested_macros_and_delimiter_aware_arguments() {
        let source = r#"%macro inner(value)
    ld hl, $value
%endmacro
%macro outer(value)
    %inner ($value + 1)
%endmacro
%outer (2 + (3 * 4))
"#;
        let result = preprocess_assembly("main.asm", source, options()).unwrap();
        assert_eq!(instruction_texts(&result), ["ld hl, 2 + (3 * 4) + 1"]);
    }

    #[test]
    fn assigns_unique_hygienic_labels_across_nested_expansions() {
        let source = r#"%macro leaf()
%%loop:
    jp %%loop
%endmacro
%macro pair()
    %leaf
    %leaf
%endmacro
%pair
%leaf
"#;
        let result = preprocess_assembly("main.asm", source, options()).unwrap();
        let labels = result
            .program
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                AssemblyItem::Label(label) => Some(label.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(labels.len(), 3);
        assert!(
            labels
                .iter()
                .all(|label| label.starts_with("__ezra_macro_"))
        );
        assert_ne!(labels[0], labels[1]);
        assert_ne!(labels[1], labels[2]);
    }

    #[test]
    fn evaluates_nested_conditions_without_mutating_inactive_state() {
        let source = r#"%if cpu("ez80")
    %define ACTIVE 7
    %if target("agonlight-mos-ez80")
        db ${ACTIVE}
    %else
        %define LEAK 1
    %endif
%else
    %define LEAK 2
%endif
%if feature("z80")
    db 8
%endif
%if defined(LEAK)
    db 9
%else
    db 10
%endif
"#;
        let result = preprocess_assembly("main.asm", source, options()).unwrap();
        let values = result
            .syntax
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ParsedAssemblyItem::Data { values, .. } => match &values[0] {
                    ParsedAssemblyDataValue::Expression(value) => Some(value.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(values, ["7", "8", "10"]);
    }

    #[test]
    fn includes_retain_provenance_and_cycles_report_the_chain() {
        let resolver = MemoryResolver::default()
            .with("lib/outer.inc", "include \"inner.inc\"\nnop\n")
            .with("lib/inner.inc", "halt\n");
        let result = preprocess_assembly_with_resolver(
            "main.asm",
            "include \"lib/outer.inc\"\n",
            &resolver,
            options(),
        )
        .unwrap();
        assert_eq!(
            source_location_name(&result.syntax.items[0].location),
            "lib/inner.inc"
        );
        assert_eq!(
            source_location_name(&result.syntax.items[1].location),
            "lib/outer.inc"
        );

        let cyclic = MemoryResolver::default()
            .with("a.inc", "include \"b.inc\"\n")
            .with("b.inc", "include \"a.inc\"\n");
        let error = preprocess_assembly_with_resolver(
            "main.asm",
            "include \"a.inc\"\n",
            &cyclic,
            options(),
        )
        .unwrap_err();
        assert!(error.message.contains("a.inc -> b.inc -> a.inc"));
        assert_eq!(source_location_name(&error.location().unwrap()), "b.inc");
    }

    #[test]
    fn rejects_wrong_arity_and_unterminated_blocks_at_the_source_location() {
        let arity = preprocess_assembly(
            "arity.asm",
            "%macro one(value)\nnop\n%endmacro\n%one\n",
            options(),
        )
        .unwrap_err();
        assert!(arity.message.contains("expects 1 arguments, got 0"));
        assert_eq!(arity.location().unwrap().line, 4);

        let unterminated =
            preprocess_assembly("unterminated.asm", "%if cpu(\"ez80\")\nnop\n", options())
                .unwrap_err();
        assert!(unterminated.span.is_some());
    }

    #[test]
    fn macro_expansion_reports_the_invocation_origin() {
        let source = "%macro broken(value)\n    org $value +\n%endmacro\n\n%broken 1\n";
        let error = preprocess_assembly("origin.asm", source, options()).unwrap_err();
        let location = error.location().unwrap();
        assert_eq!(location.line, 5);
        assert_eq!(source_location_name(&location), "origin.asm");
    }

    #[test]
    fn workspace_resolver_handles_relative_includes() {
        let files = [
            WorkspaceFile::text("src/main.asm", "include \"../lib/code.inc\"\n"),
            WorkspaceFile::text("lib/code.inc", "nop\n"),
        ];
        let workspace = Workspace::new(&files);
        let result =
            preprocess_assembly_workspace(&workspace, r"src\.\main.asm", options()).unwrap();
        assert_eq!(instruction_texts(&result), ["nop"]);
        assert_eq!(
            source_location_name(&result.syntax.items[0].location),
            "lib/code.inc"
        );
    }

    #[test]
    fn normalizes_compatibility_directives() {
        let result = preprocess_assembly(
            "compat.asm",
            ".global entry\n.assume adl = 1\nentry:\n    nop\n",
            options(),
        )
        .unwrap();
        assert_eq!(result.program.items.len(), 2);

        let error = preprocess_assembly("compat.asm", ".assume adl=0\n", options()).unwrap_err();
        assert!(error.message.contains("adl=0"));
        assert_eq!(error.location().unwrap().line, 1);
    }
