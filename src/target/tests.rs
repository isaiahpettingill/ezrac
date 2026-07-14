use super::*;

#[test]
#[cfg(feature = "m68k")]
fn parses_target_triples_with_optional_versions() {
    assert_eq!(
        parse_target_triple("agonlight-console8-ez80-1.0")
            .unwrap()
            .cpu,
        CpuFamily::Ez80
    );
    assert_eq!(
        parse_target_triple("cpm-2.2-z80").unwrap().cpu,
        CpuFamily::Z80
    );
    assert_eq!(
        parse_target_triple("bare-z80n").unwrap().cpu,
        CpuFamily::Z80N
    );
    assert_eq!(
        parse_target_triple("bare-z180").unwrap().cpu,
        CpuFamily::Z180
    );
    assert_eq!(
        parse_target_triple("bare-i8085").unwrap().cpu,
        CpuFamily::I8085
    );
    assert_eq!(
        parse_target_triple("sega-genesis-m68k").unwrap().cpu,
        CpuFamily::M68k
    );
}

#[test]
fn rejects_targets_without_known_cpu_family() {
    let error = parse_target_triple("agonlight-console8").unwrap_err();
    assert!(error.contains("missing a supported CPU family"), "{error}");
}

#[test]
fn resolves_z80_and_ez80_target_profiles() {
    assert!(resolve_target_profile(Some("ti84plusce-ez80")).is_ok());
    let z80 = resolve_target_profile(Some("zxspectrum-z80")).unwrap();

    assert_eq!(z80.triple.cpu, CpuFamily::Z80);
    assert_eq!(z80.memory.pointer_width_bits, 16);
    assert_eq!(z80.memory.address_width_bits, 16);
    assert_eq!(z80.output_format, OutputFormat::ZxSpectrumTap);
}

#[test]
fn cpm_z80_targets_default_to_com_output() {
    let cpm = resolve_target_profile(Some("cpm-2.2-z80")).unwrap();

    assert_eq!(cpm.output_format, OutputFormat::CpmCom);
    assert_eq!(cpm.output_format.extension(), "com");
}

#[test]
fn cpm_8080_targets_default_to_com_output() {
    let cpm = resolve_target_profile(Some("cpm-2.2-i8080")).unwrap();

    assert_eq!(cpm.triple.cpu, CpuFamily::I8080);
    assert_eq!(cpm.output_format, OutputFormat::CpmCom);
    assert_eq!(cpm.output_format.extension(), "com");
    assert_eq!(cpm.memory.address_width_bits, 16);
}

#[test]
fn cpm_8085_targets_default_to_com_output() {
    let cpm = resolve_target_profile(Some("cpm-2.2-i8085")).unwrap();

    assert_eq!(cpm.triple.cpu, CpuFamily::I8085);
    assert_eq!(cpm.output_format, OutputFormat::CpmCom);
    assert_eq!(cpm.output_format.extension(), "com");
    assert_eq!(cpm.memory.address_width_bits, 16);
}

#[test]
fn resolves_bare_targets_without_default_sdk_symbols() {
    let target = resolve_target_profile(Some("bare-z180")).unwrap();

    assert_eq!(target.triple.cpu, CpuFamily::Z180);
    assert_eq!(target.output_format, OutputFormat::RawBin);
    assert_eq!(target.memory.address_width_bits, 16);
    assert!(!target.default_sdk_symbols);
}

#[cfg(feature = "mos6502")]
#[test]
fn resolves_generic_bare_6502_target() {
    let profile = resolve_target_profile(Some("generic-6502-bare")).unwrap();

    assert_eq!(profile.triple.cpu, CpuFamily::Mos6502);
    assert_eq!(profile.memory.pointer_width_bits, 16);
    assert_eq!(profile.output_format, OutputFormat::RawBin);
    assert!(!profile.default_sdk_symbols);
    assert!(!profile.supports_port_io());
}

#[cfg(feature = "tms9900")]
#[test]
fn resolves_bare_tms9900_target() {
    let profile = resolve_target_profile(Some("bare-tms9900")).unwrap();

    assert_eq!(profile.triple.cpu, CpuFamily::Tms9900);
    assert_eq!(profile.memory.pointer_width_bits, 16);
    assert_eq!(profile.memory.address_width_bits, 16);
    assert_eq!(profile.output_format, OutputFormat::RawBin);
    assert!(!profile.default_sdk_symbols);
    assert!(!profile.supports_port_io());
}

#[cfg(feature = "mos6502")]
#[test]
fn commodore64_target_defaults_to_prg_output() {
    let profile = resolve_target_profile(Some("commodore64-6502")).unwrap();

    assert_eq!(profile.triple.cpu, CpuFamily::Mos6502);
    assert_eq!(profile.output_format, OutputFormat::Commodore64Prg);
    assert_eq!(profile.output_format.extension(), "prg");
    assert_eq!(profile.memory.address_width_bits, 16);
}

#[test]
fn ti_calculator_targets_default_to_8xp_output() {
    for target in [
        "ti83-z80",
        "ti84plus-z80",
        "ti84plusce-ez80",
        "ti83premiumce-ez80",
    ] {
        let target = resolve_target_profile(Some(target)).unwrap();
        assert_eq!(target.output_format, OutputFormat::Ti8xp);
        assert_eq!(target.output_format.extension(), "8xp");
    }
}

#[test]
fn ez180n_targets_default_to_gaem_output() {
    let target = resolve_target_profile(Some("ez180n-ez80")).unwrap();

    assert_eq!(target.output_format, OutputFormat::Ez180nGaem);
    assert_eq!(target.output_format.extension(), "gaem");
}

#[cfg(feature = "m68k")]
#[test]
fn resolves_generic_bare_m68k_target() {
    let profile = resolve_target_profile(Some("generic-m68k-bare")).unwrap();

    assert_eq!(profile.triple.cpu, CpuFamily::M68k);
    assert_eq!(profile.memory.pointer_width_bits, 24);
    assert_eq!(profile.memory.address_width_bits, 24);
    assert_eq!(profile.output_format, OutputFormat::RawBin);
    assert!(!profile.default_sdk_symbols);
    assert!(!profile.supports_port_io());
}

#[test]
fn parses_output_formats() {
    assert_eq!(parse_output_format("bin"), Ok(OutputFormat::RawBin));
    assert_eq!(parse_output_format("com"), Ok(OutputFormat::CpmCom));
    assert_eq!(parse_output_format("gaem"), Ok(OutputFormat::Ez180nGaem));
    assert_eq!(parse_output_format("hex"), Ok(OutputFormat::IntelHex));
    assert_eq!(parse_output_format("8xp"), Ok(OutputFormat::Ti8xp));
    assert_eq!(parse_output_format("8ek"), Ok(OutputFormat::Ti8ek));
    assert_eq!(parse_output_format("8xk"), Ok(OutputFormat::Ti8xk));
    assert_eq!(parse_output_format("tap"), Ok(OutputFormat::ZxSpectrumTap));
    assert_eq!(parse_output_format("gb"), Ok(OutputFormat::GameBoyGb));
    assert_eq!(parse_output_format("prg"), Ok(OutputFormat::Commodore64Prg));
    assert_eq!(parse_output_format("crt"), Ok(OutputFormat::Commodore64Crt));
    let error = parse_output_format("bad").unwrap_err();
    assert!(
        error.contains(
            "expected `bin`, `com`, `gaem`, `hex`, `tap`, `gb`, `prg`, `crt`, `8xp`, `8ek`, or `8xk`"
        ),
        "{error}"
    );
}

#[test]
fn resolves_game_boy_assembly_targets() {
    for target in ["gameboy-dmg-lr35902", "gameboy-color-lr35902"] {
        let profile = resolve_target_profile(Some(target)).unwrap();
        assert_eq!(profile.triple.cpu, CpuFamily::Lr35902);
        assert_eq!(profile.memory.address_width_bits, 16);
        assert_eq!(profile.output_format, OutputFormat::GameBoyGb);
        assert_eq!(
            AssemblerCpu::from(profile.triple.cpu),
            AssemblerCpu::Lr35902
        );
    }
}

#[test]
#[cfg(feature = "avr")]
fn resolves_arduboy_avr_target_profile() {
    let profile = super::resolve_target_profile(Some("arduboy-avr")).unwrap();
    assert_eq!(profile.triple.cpu, super::CpuFamily::Avr);
    assert_eq!(profile.output_format, super::OutputFormat::ArduinoHex);
    assert_eq!(profile.memory.pointer_width_bits, 16);
}
