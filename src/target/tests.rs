use super::*;
use crate::layout::default_layout_for_target;

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
        parse_target_triple("bare-r800").unwrap().cpu,
        CpuFamily::R800
    );
    assert_eq!(AssemblerCpu::parse("r800").unwrap(), AssemblerCpu::R800);
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

#[cfg(not(feature = "i8086"))]
#[test]
fn i8086_aliases_report_the_required_feature() {
    for alias in ["i8086", "8086"] {
        let error = AssemblerCpu::parse(alias).unwrap_err();
        assert!(
            error.contains("requires the `i8086` Cargo feature"),
            "{error}"
        );
    }
}

#[cfg(feature = "i8086")]
#[test]
fn resolves_i8086_aliases_and_bare_target_capabilities() {
    assert_eq!(AssemblerCpu::parse("i8086").unwrap(), AssemblerCpu::I8086);
    assert_eq!(AssemblerCpu::parse("8086").unwrap(), AssemblerCpu::I8086);

    for target in ["bare-i8086", "bare-8086"] {
        let profile = resolve_target_profile(Some(target)).unwrap();
        assert_eq!(profile.triple.cpu, CpuFamily::I8086);
        assert_eq!(AssemblerCpu::from(profile.triple.cpu), AssemblerCpu::I8086);
        assert_eq!(profile.memory.pointer_width_bits, 16);
        assert_eq!(profile.memory.address_width_bits, 16);
        assert_eq!(profile.output_format, OutputFormat::RawBin);
        assert!(!profile.default_sdk_symbols);
        assert!(profile.supports_port_io());
    }
}

#[test]
fn classifies_canonical_and_versioned_msdos_i8086_targets() {
    assert!(is_msdos_i8086_target("msdos-com-i8086"));
    assert!(is_msdos_i8086_target("msdos-com-i8086-6.22"));
    assert!(!is_msdos_i8086_target("msdos-com-i8086-"));
    assert!(!is_msdos_i8086_target("msdos-com-z80-6.22"));
}

#[cfg(feature = "i8086")]
#[test]
fn versioned_msdos_i8086_targets_use_the_canonical_profile() {
    let profile = resolve_target_profile(Some("msdos-com-i8086-6.22")).unwrap();

    assert_eq!(profile.triple.cpu, CpuFamily::I8086);
    assert_eq!(profile.output_format, OutputFormat::CpmCom);
    assert_eq!(profile.memory.address_width_bits, 16);
    assert!(profile.default_sdk_symbols);
}

#[test]
fn rejects_targets_without_known_cpu_family() {
    let error = parse_target_triple("agonlight-console8").unwrap_err();
    assert!(error.contains("missing a supported CPU family"), "{error}");
}

#[test]
fn resolves_z80_and_ez80_target_profiles() {
    assert!(resolve_target_profile(Some("ti84plusce-ez80")).is_ok());
    for target in ["zxspectrum-z80", "zxspectrum-z80-128k"] {
        let z80 = resolve_target_profile(Some(target)).unwrap();

        assert_eq!(z80.triple.cpu, CpuFamily::Z80);
        assert_eq!(z80.memory.pointer_width_bits, 16);
        assert_eq!(z80.memory.address_width_bits, 16);
        assert_eq!(z80.output_format, OutputFormat::ZxSpectrumTap);
    }
}

#[test]
fn rejects_platform_cpu_combinations_that_would_mix_16_and_24_bit_assumptions() {
    for (target, expected) in [
        ("cpm-2.2-ez80", "requires CPU `z80 or i8080 or i8085`"),
        ("zxspectrum-ez80", "requires CPU `z80`"),
        ("ti84plusce-z80", "requires CPU `ez80`"),
    ] {
        let error = resolve_target_profile(Some(target)).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn cpu_capabilities_are_canonical_for_z80_family_targets() {
    for (cpu, name, width) in [
        (CpuFamily::Ez80, "ez80-adl", 24),
        (CpuFamily::Z80, "z80", 16),
        (CpuFamily::R800, "r800", 16),
        (CpuFamily::Z80N, "z80n", 16),
        (CpuFamily::Z180, "z180", 16),
        (CpuFamily::I8080, "i8080", 16),
        (CpuFamily::I8085, "i8085", 16),
    ] {
        let capabilities = cpu.capabilities();
        assert_eq!(capabilities.name, name);
        assert_eq!(capabilities.memory.pointer_width_bits, width);
        assert_eq!(capabilities.memory.address_width_bits, width);
        assert!(capabilities.supports_port_io);
    }
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
fn resolves_bare_r800_as_a_raw_16_bit_target() {
    let target = resolve_target_profile(Some("bare-r800")).unwrap();

    assert_eq!(target.triple.cpu, CpuFamily::R800);
    assert_eq!(AssemblerCpu::from(target.triple.cpu), AssemblerCpu::R800);
    assert_eq!(target.memory.pointer_width_bits, 16);
    assert_eq!(target.memory.address_width_bits, 16);
    assert_eq!(target.output_format, OutputFormat::RawBin);
    assert!(!target.default_sdk_symbols);
    assert!(target.supports_port_io());
}

#[test]
fn resolves_bare_targets_without_default_sdk_symbols() {
    let target = resolve_target_profile(Some("bare-z180")).unwrap();

    assert_eq!(target.triple.cpu, CpuFamily::Z180);
    assert_eq!(target.output_format, OutputFormat::RawBin);
    assert_eq!(target.memory.address_width_bits, 16);
    assert!(!target.default_sdk_symbols);
}

#[test]
fn resolves_sega_master_system_z80_target_profile() {
    let profile = resolve_target_profile(Some("sega-master-system-z80")).unwrap();

    assert_eq!(profile.triple.cpu, CpuFamily::Z80);
    assert_eq!(profile.output_format, OutputFormat::SmsRom);
    assert_eq!(profile.output_format.extension(), "sms");
    assert_eq!(profile.memory.pointer_width_bits, 16);
    assert_eq!(profile.memory.address_width_bits, 16);
    assert!(profile.default_sdk_symbols);
    assert_eq!(parse_output_format("sms"), Ok(OutputFormat::SmsRom));
}

#[test]
fn resolves_sega_game_gear_z80_target_profile() {
    let profile = resolve_target_profile(Some("sega-game-gear-z80")).unwrap();

    assert_eq!(profile.triple.cpu, CpuFamily::Z80);
    assert_eq!(profile.output_format, OutputFormat::GameGearRom);
    assert_eq!(profile.output_format.extension(), "gg");
    assert_eq!(profile.memory.pointer_width_bits, 16);
    assert_eq!(profile.memory.address_width_bits, 16);
    assert!(profile.default_sdk_symbols);
    assert_eq!(parse_output_format("gg"), Ok(OutputFormat::GameGearRom));
}

#[cfg(feature = "mos6502")]
#[test]
fn resolves_nes_2a03_target_profile() {
    let profile = resolve_target_profile(Some("nes-2a03")).unwrap();

    assert_eq!(profile.triple.cpu, CpuFamily::Ricoh2A03);
    assert_eq!(profile.output_format, OutputFormat::NesRom);
    assert_eq!(profile.output_format.extension(), "nes");
    assert_eq!(profile.memory.address_width_bits, 16);
    assert!(!profile.default_sdk_symbols);
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

#[cfg(feature = "mos6502")]
#[test]
fn resolves_bare_65c816_aliases_as_24_bit_raw_targets() {
    for target in ["generic-65c816-bare", "generic-65816-bare"] {
        let profile = resolve_target_profile(Some(target)).unwrap();
        assert_eq!(profile.triple.cpu, CpuFamily::Wdc65C816);
        assert_eq!(
            AssemblerCpu::from(profile.triple.cpu),
            AssemblerCpu::Wdc65C816
        );
        assert_eq!(profile.memory.pointer_width_bits, 24);
        assert_eq!(profile.memory.address_width_bits, 24);
        assert_eq!(profile.output_format, OutputFormat::RawBin);
        assert!(!profile.default_sdk_symbols);
    }
    let layout = default_layout_for_target("generic-65c816-bare");
    assert_eq!(layout.name, "bare_65c816");
    assert_eq!(layout.entry.get(), 0x008000);
}

#[cfg(feature = "tms9900")]
#[test]
fn resolves_ti99_4a_tms9900_target() {
    let profile = resolve_target_profile(Some("ti99-4a-tms9900")).unwrap();

    assert_eq!(profile.triple.cpu, CpuFamily::Tms9900);
    assert_eq!(profile.memory.pointer_width_bits, 16);
    assert_eq!(profile.output_format, OutputFormat::RawBin);
    assert!(profile.default_sdk_symbols);
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

#[cfg(feature = "dcpu")]
#[test]
fn resolves_generic_bare_dcpu_target() {
    let profile = resolve_target_profile(Some("generic-dcpu-bare")).unwrap();

    assert_eq!(profile.triple.cpu, CpuFamily::Dcpu);
    assert_eq!(profile.memory.pointer_width_bits, 16);
    assert_eq!(profile.memory.address_width_bits, 16);
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
    assert_eq!(parse_output_format("arduboy"), Ok(OutputFormat::Arduboy));
    assert_eq!(parse_output_format("8xp"), Ok(OutputFormat::Ti8xp));
    assert_eq!(parse_output_format("8ek"), Ok(OutputFormat::Ti8ek));
    assert_eq!(parse_output_format("8xk"), Ok(OutputFormat::Ti8xk));
    assert_eq!(parse_output_format("tap"), Ok(OutputFormat::ZxSpectrumTap));
    assert_eq!(parse_output_format("gb"), Ok(OutputFormat::GameBoyGb));
    assert_eq!(parse_output_format("prg"), Ok(OutputFormat::Commodore64Prg));
    assert_eq!(parse_output_format("crt"), Ok(OutputFormat::Commodore64Crt));
    assert_eq!(parse_output_format("nes"), Ok(OutputFormat::NesRom));
    assert_eq!(parse_output_format("sms"), Ok(OutputFormat::SmsRom));
    assert_eq!(parse_output_format("gg"), Ok(OutputFormat::GameGearRom));
    let error = parse_output_format("bad").unwrap_err();
    assert!(
        error.contains(
            "expected `bin`, `com`, `gaem`, `hex`, `arduboy`, `tap`, `gb`, `prg`, `crt`, `nes`, `sms`, `gg`, `8xp`, `8ek`, or `8xk`"
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
fn resolves_arduboy_and_bare_avr_target_profiles() {
    let arduboy = super::resolve_target_profile(Some("arduboy-avr")).unwrap();
    assert_eq!(arduboy.triple.cpu, super::CpuFamily::Avr);
    assert_eq!(arduboy.output_format, super::OutputFormat::ArduinoHex);
    assert_eq!(arduboy.memory.pointer_width_bits, 16);

    let bare = super::resolve_target_profile(Some("bare-avr")).unwrap();
    assert_eq!(bare.triple.cpu, super::CpuFamily::Avr);
    assert_eq!(bare.output_format, super::OutputFormat::RawBin);
    assert!(!bare.default_sdk_symbols);

    let arduboy_layout = crate::layout::default_layout_for_target("arduboy-avr");
    assert_eq!(arduboy_layout.name, "arduboy_atmega32u4");
    assert_eq!(arduboy_layout.stack.get(), 0x0AFF);
    assert_eq!(arduboy_layout.regions[0].end.get(), 0x6FFF);
}

#[test]
fn resolves_all_ez180n_cpu_target_profiles() {
    for (target, cpu, cpu_id, width) in [
        ("ez180n-i8080", CpuFamily::I8080, 0, 16),
        ("ez180n-i8085", CpuFamily::I8085, 1, 16),
        ("ez180n-z80", CpuFamily::Z80, 2, 16),
        ("ez180n-z80n", CpuFamily::Z80N, 3, 16),
        ("ez180n-z180", CpuFamily::Z180, 4, 16),
        ("ez180n-ez80", CpuFamily::Ez80, 5, 24),
    ] {
        let profile = resolve_target_profile(Some(target)).unwrap();
        assert_eq!(profile.triple.cpu, cpu);
        assert_eq!(profile.output_format, OutputFormat::Ez180nGaem);
        assert_eq!(profile.memory.pointer_width_bits, width);
        assert_eq!(profile.memory.address_width_bits, width);
        assert_eq!(ez180n_cpu_id(target), Some(cpu_id));
    }
}

#[test]
fn ez180n_layouts_match_cpu_address_widths() {
    let adl = default_layout_for_target("ez180n-ez80");
    assert_eq!(adl.validate(), Ok(()));
    assert_eq!(adl.load.get(), 0x010000);
    assert_eq!(adl.entry.get(), 0x010000);
    assert!(
        adl.regions
            .iter()
            .all(|region| { !matches!(region.name.as_str(), "low" | "header") })
    );
    assert!(
        adl.sections
            .iter()
            .any(|section| { section.name == ".header" && section.region == "code" })
    );
    assert!(
        adl.regions
            .iter()
            .any(|region| { region.name == "vram" && region.start.get() == 0x080000 })
    );

    for target in [
        "ez180n-i8080",
        "ez180n-i8085",
        "ez180n-z80",
        "ez180n-z80n",
        "ez180n-z180",
    ] {
        let layout = default_layout_for_target(target);
        assert_eq!(layout.validate(), Ok(()));
        assert_eq!(layout.name, "ez180n_16");
        assert_eq!(layout.load.get(), 0);
        assert_eq!(layout.entry.get(), 0);
        assert!(layout.stack.get() < 0xE000);
        assert!(
            layout
                .regions
                .iter()
                .all(|region| region.end.get() <= 0xFFFF)
        );
        assert!(
            layout
                .symbols
                .iter()
                .all(|symbol| symbol.value.get() <= 0xFFFF)
        );
        assert!(layout.regions.iter().any(|region| {
            region.name == "vram" && region.start.get() == 0xE000 && region.end.get() == 0xF1FF
        }));
    }
}
