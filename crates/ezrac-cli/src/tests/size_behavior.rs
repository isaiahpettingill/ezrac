use super::*;

#[test]
fn representative_size_baselines_are_deterministic() {
    let source =
        std::fs::read_to_string(repository_root().join("tests/fixtures/size/tiny.ezra")).unwrap();
    for target in [
        "cpm-2.2-z80",
        "cpm-2.2-i8080",
        "msdos-com-i8086",
        "agonlight-mos-ez80",
        "commodore64-6502",
    ] {
        let files = [ezra::api::WorkspaceFile::text("tiny.ezra", &source)];
        let request = ezra::api::CompileRequest::new("tiny.ezra", target);
        let build =
            ezra::api::build_workspace(&ezra::api::Workspace::new(&files), "tiny.ezra", &request)
                .unwrap_or_else(|error| panic!("{target}: {error}"));
        let expected = match target {
            "cpm-2.2-z80" | "cpm-2.2-i8080" => {
                "ezrac-size-v1\ntext=13\nrodata=0\ninitialized_data=0\nassets=0\nbss=0\nruntime_helpers=unknown\nmachine_code_payload=13\naddress_span=13\naddress_gaps=0\nfinal_package=13\nsection:.rodata=0\nsection:.text=13\n"
            }
            "msdos-com-i8086" => {
                "ezrac-size-v1\ntext=33\nrodata=0\ninitialized_data=0\nassets=0\nbss=0\nruntime_helpers=unknown\nmachine_code_payload=33\naddress_span=33\naddress_gaps=0\nfinal_package=33\nsection:.rodata=0\nsection:.text=33\n"
            }
            "agonlight-mos-ez80" => {
                "ezrac-size-v1\ntext=14\nrodata=0\ninitialized_data=0\nassets=0\nbss=0\nruntime_helpers=unknown\nmachine_code_payload=14\naddress_span=14\naddress_gaps=0\nfinal_package=83\nsection:.rodata=0\nsection:.text=14\n"
            }
            "commodore64-6502" => {
                "ezrac-size-v1\ntext=11\nrodata=0\ninitialized_data=0\nassets=0\nbss=0\nruntime_helpers=unknown\nmachine_code_payload=11\naddress_span=11\naddress_gaps=0\nfinal_package=25\nsection:.rodata=0\nsection:.text=11\n"
            }
            _ => unreachable!(),
        };
        assert_eq!(build.size_report.to_stable_string(), expected);
        assert_eq!(build.size_report.final_package, build.executable.len());
        assert!(build.size_report.machine_code_payload <= build.size_report.address_span);
    }
}

#[test]
fn source_storage_sections_are_reported_separately_from_code_payload() {
    let source = r#"
        global initialized: u8 = 1
        global zeroed: u8 = 0
        embed sprite: bytes = bytes [0x11, 0x22, 0x33]

        fn main() {
            let value: u8 = initialized + zeroed
            let pixel: u8 = *sprite.ptr
            let text: ptr<u8> = "ok"
        }
    "#;
    let files = [ezra::api::WorkspaceFile::text("storage.ezra", source)];
    let request = ezra::api::CompileRequest::new("storage.ezra", "cpm-2.2-z80");
    let build =
        ezra::api::build_workspace(&ezra::api::Workspace::new(&files), "storage.ezra", &request)
            .unwrap();

    assert_eq!(build.size_report.rodata, 3);
    assert_eq!(build.size_report.initialized_data, 1);
    assert_eq!(build.size_report.assets, 3);
    assert_eq!(build.size_report.bss, 1);
    assert!(build.size_report.machine_code_payload > 0);
}

#[test]
fn runtime_helper_size_is_reported_when_helper_symbols_have_spans() {
    let source = "fn main() { test.pass() }";
    let files = [ezra::api::WorkspaceFile::text("helper.ezra", source)];
    let request = ezra::api::CompileRequest::new("helper.ezra", "cpm-2.2-z80");
    let build =
        ezra::api::build_workspace(&ezra::api::Workspace::new(&files), "helper.ezra", &request)
            .unwrap();

    assert!(build.size_report.runtime_helpers.is_some());
    assert!(
        build
            .size_report
            .to_stable_string()
            .contains("runtime_helpers=")
    );
}

#[test]
fn helper_budget_diagnostic_names_the_runtime_helper_class() {
    let files = [ezra::api::WorkspaceFile::text(
        "helper.ezra",
        "fn main() { test.pass() }",
    )];
    let request = ezra::api::CompileRequest::new("helper.ezra", "cpm-2.2-z80");
    let mut build = ezra::api::BuildRequest::for_target("cpm-2.2-z80").unwrap();
    build.size_budgets = ezra::api::SizeBudgets::default().with_runtime_helpers(0);
    let error = ezra::api::build_workspace_with_request(
        &ezra::api::Workspace::new(&files),
        "helper.ezra",
        &request,
        &build,
    )
    .unwrap_err();

    assert!(
        error
            .message
            .contains("runtime-helper size budget exceeded"),
        "{error}"
    );
    assert!(
        error.message.contains("remove helper-using calls"),
        "{error}"
    );
}
