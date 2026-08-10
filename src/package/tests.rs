use super::*;

#[test]
fn packages_snes_5a22_entry_code_as_32k_lorom() {
    let mut request = PackageRequest::new(
        crate::target::SNES_5A22_TARGET,
        OutputFormat::SnesRom,
        0x008000,
        0x008000,
    );
    request.executable_name = Some("EZRA SNES".to_owned());
    let rom = package_executable(&request, &[0x18, 0xFB, 0x80, 0xFE]).unwrap();
    assert_eq!(rom.len(), 0x8000);
    assert_eq!(&rom[..4], &[0x18, 0xFB, 0x80, 0xFE]);
    assert_eq!(&rom[0x7FC0..0x7FC9], b"EZRA SNES");
    assert_eq!(rom[0x7FD5], 0x20);
    assert_eq!(&rom[0x7FFC..0x7FFE], &[0x00, 0x80]);
    let checksum = u16::from_le_bytes([rom[0x7FDE], rom[0x7FDF]]);
    let complement = u16::from_le_bytes([rom[0x7FDC], rom[0x7FDD]]);
    assert_eq!(checksum ^ complement, 0xFFFF);
}

#[test]
fn rejects_snes_code_that_overwrites_the_internal_header() {
    let error = package_executable(
        &PackageRequest::new(
            crate::target::SNES_5A22_TARGET,
            OutputFormat::SnesRom,
            0x008000,
            0x008000,
        ),
        &vec![0; 0x7FC1],
    )
    .unwrap_err();
    assert!(error.message.contains("internal header"));
}

#[test]
fn packages_nes_nrom_image_without_adding_a_second_header() {
    let mut image = vec![0; 0x6010];
    image[..8].copy_from_slice(b"NES\x1A\x01\x01\0\0");
    for (offset, vector) in [(0x400A, 0xC000u16), (0x400C, 0xC010), (0x400E, 0xC020)] {
        image[offset..offset + 2].copy_from_slice(&vector.to_le_bytes());
    }
    let packaged = package_executable(
        &PackageRequest::new("nes-2a03", OutputFormat::NesRom, 0xBFF0, 0xC000),
        &image,
    )
    .unwrap();
    assert_eq!(packaged, image);
}

#[test]
fn packages_nes_entry_code_as_nrom_with_vectors_and_a_solid_tile() {
    let code = [0x78, 0xD8, 0x4C, 0x00, 0xC0];
    let packaged = package_executable(
        &PackageRequest::new("nes-2a03", OutputFormat::NesRom, 0xBFF0, 0xC000),
        &code,
    )
    .unwrap();

    assert_eq!(packaged.len(), 0x6010);
    assert_eq!(&packaged[..8], b"NES\x1A\x01\x01\0\0");
    assert_eq!(&packaged[0x10..0x10 + code.len()], &code);
    assert!(packaged[0x4010..0x4018].iter().all(|byte| *byte == 0xFF));
    assert!(packaged[0x4018..].iter().all(|byte| *byte == 0));
    for offset in [0x400A, 0x400C, 0x400E] {
        assert_eq!(&packaged[offset..offset + 2], &0xC000u16.to_le_bytes());
    }
}

#[test]
fn packages_source_nes_chr_tiles_after_reserved_tile_zero() {
    let code = [0x78, 0xD8, 0x4C, 0x00, 0xC0];
    let chr_payload = (0..32).collect::<Vec<_>>();
    let mut context = PackageContext::new();
    context.nes = Some(NesPackageOptions {
        chr_payload: chr_payload.clone(),
    });
    let packaged = package_executable_with_context(
        &PackageRequest::new("nes-2a03", OutputFormat::NesRom, 0xBFF0, 0xC000),
        &context,
        &code,
    )
    .unwrap();

    assert!(packaged[0x4010..0x4018].iter().all(|byte| *byte == 0xFF));
    assert!(packaged[0x4018..0x4020].iter().all(|byte| *byte == 0));
    assert_eq!(&packaged[0x4020..0x4040], chr_payload.as_slice());
    assert!(packaged[0x4040..].iter().all(|byte| *byte == 0));
}

#[test]
fn rejects_invalid_source_nes_chr_payloads() {
    let request = PackageRequest::new("nes-2a03", OutputFormat::NesRom, 0xBFF0, 0xC000);
    for (payload, expected) in [
        (vec![0; 15], "whole 16-byte tiles"),
        (vec![0; 0x2000], "exceeds 8176 bytes"),
    ] {
        let mut context = PackageContext::new();
        context.nes = Some(NesPackageOptions {
            chr_payload: payload,
        });
        let error = package_executable_with_context(&request, &context, &[0x78]).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
    }
}

#[test]
fn rejects_nes_images_with_the_wrong_mapper_or_size() {
    let mut image = vec![0; 0x6010];
    image[..8].copy_from_slice(b"NES\x1A\x01\x01\0\0");
    for offset in [0x400A, 0x400C, 0x400E] {
        image[offset..offset + 2].copy_from_slice(&0xC000u16.to_le_bytes());
    }
    image[6] = 0x10;
    let error = package_executable(
        &PackageRequest::new("nes-2a03", OutputFormat::NesRom, 0xBFF0, 0xC000),
        &image,
    )
    .unwrap_err();
    assert!(error.message.contains("mapper 0"), "{}", error.message);

    image[6] = 0;
    let error = package_executable(
        &PackageRequest::new("nes-2a03", OutputFormat::NesRom, 0xBFF0, 0xC000),
        &image[..0x6010 - 1],
    )
    .unwrap_err();
    assert!(
        error.message.contains("exactly 24592 bytes"),
        "{}",
        error.message
    );
}

#[test]
fn packages_sms_code_as_a_padded_32k_rom_with_standard_header() {
    let code = [0xF3, 0x31, 0xF0, 0xDF];
    let image = package_executable(
        &PackageRequest::new("sega-master-system-z80", OutputFormat::SmsRom, 0, 0x0069),
        &code,
    )
    .unwrap();

    assert_eq!(image.len(), 0x8000);
    assert_eq!(&image[..3], &[0xC3, 0x69, 0x00]);
    assert_eq!(&image[0x0038..0x003A], &[0xED, 0x4D]);
    assert_eq!(&image[0x0066..0x0068], &[0xED, 0x45]);
    assert_eq!(&image[0x0069..0x0069 + code.len()], &code);
    assert!(
        image[0x0069 + code.len()..0x7FF0]
            .iter()
            .all(|byte| *byte == 0xFF)
    );
    assert_eq!(&image[0x7FF0..0x7FF8], b"TMR SEGA");
    assert_eq!(&image[0x7FF8..0x7FFA], &[0xFF, 0xFF]);
    assert_eq!(&image[0x7FFC..0x7FFF], &[0, 0, 0]);
    assert_eq!(image[0x7FFF], 0x4C);
    let checksum = image[..0x7FF0]
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
    assert_eq!(u16::from_le_bytes([image[0x7FFA], image[0x7FFB]]), checksum);
}

#[test]
fn packages_game_gear_code_with_game_gear_header() {
    let code = [0xF3, 0x31, 0xF0, 0xDF];
    let image = package_executable(
        &PackageRequest::new("sega-game-gear-z80", OutputFormat::GameGearRom, 0, 0x0069),
        &code,
    )
    .unwrap();

    assert_eq!(image.len(), 0x8000);
    assert_eq!(&image[0x7FF0..0x7FF8], b"TMR SEGA");
    assert_eq!(image[0x7FFF], 0x7C);
    let checksum = image[..0x7FF0]
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
    assert_eq!(u16::from_le_bytes([image[0x7FFA], image[0x7FFB]]), checksum);
}

#[test]
fn packages_banked_sms_and_game_gear_roms() {
    for (target, format, system_nibble) in [
        ("sega-master-system-z80", OutputFormat::SmsRom, 0x40),
        ("sega-game-gear-z80", OutputFormat::GameGearRom, 0x70),
    ] {
        let request = PackageRequest::new(target, format, 0, 0x0069);
        let mut context = PackageContext::new();
        context.sega = Some(SegaPackageOptions {
            rom_size_kib: 64,
            bank_payloads: vec![vec![0x22, 0x23], vec![0x33]],
        });
        let image = package_executable_with_context(&request, &context, &[0x00]).unwrap();

        assert_eq!(image.len(), 0x10000);
        assert_eq!(&image[0x8000..0x8002], &[0x22, 0x23]);
        assert_eq!(image[0xC000], 0x33);
        assert_eq!(image[0x7FFF], system_nibble | 0x0E);
        let checksum = image[..0x7FF0]
            .iter()
            .chain(image[0x8000..].iter())
            .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
        assert_eq!(u16::from_le_bytes([image[0x7FFA], image[0x7FFB]]), checksum);
    }
}

#[test]
fn rejects_sega_bank_payload_larger_than_16k() {
    let request = PackageRequest::new("sega-master-system-z80", OutputFormat::SmsRom, 0, 0x0069);
    let mut context = PackageContext::new();
    context.sega = Some(SegaPackageOptions {
        rom_size_kib: 48,
        bank_payloads: vec![vec![0; 0x4001]],
    });

    let error = package_executable_with_context(&request, &context, &[0]).unwrap_err();
    assert!(
        error.message.contains("must fit in 16384 bytes"),
        "{}",
        error.message
    );
}

#[test]
fn rejects_sms_code_that_overlaps_the_rom_header() {
    let error = package_executable(
        &PackageRequest::new("sega-master-system-z80", OutputFormat::SmsRom, 0, 0x0069),
        &vec![0; 0x7FF0 - 0x0069 + 1],
    )
    .unwrap_err();

    assert!(
        error.message.contains("before the ROM header"),
        "{}",
        error.message
    );
}

#[test]
fn packages_agon_mos_in_memory() {
    let image = package_executable(
        &PackageRequest::new("agonlight-mos-ez80", OutputFormat::RawBin, 0x40000, 0x40045),
        &[0],
    )
    .unwrap();
    assert_eq!(&image[64..69], b"MOS\0\x01");
}
#[test]
fn packages_c64_prg_in_memory() {
    let image = package_executable(
        &PackageRequest::new(
            "commodore64-6502",
            OutputFormat::Commodore64Prg,
            0x80d,
            0x80d,
        ),
        &[0xea],
    )
    .unwrap();
    assert_eq!(&image[..2], &[1, 8]);
}

#[test]
fn packages_arduboy_with_resolved_metadata_and_name() {
    let context = PackageContext {
        executable_name: Some("demo".to_owned()),
        arduboy: Some(ArduboyPackageOptions {
            title: "Demo".to_owned(),
            author: "Ezra".to_owned(),
            version: "1.0".to_owned(),
            description: Some("A test".to_owned()),
            date: None,
            genre: None,
            source_url: None,
        }),
        ..PackageContext::new()
    };
    let image = package_executable_with_context(
        &PackageRequest::new("arduboy-avr", OutputFormat::Arduboy, 0, 0),
        &context,
        &[0],
    )
    .unwrap();
    assert!(
        image
            .windows(b"info.json".len())
            .any(|bytes| bytes == b"info.json")
    );
    assert!(
        image
            .windows(b"demo.hex".len())
            .any(|bytes| bytes == b"demo.hex")
    );
    assert!(
        image
            .windows(b"\"title\":\"Demo\"".len())
            .any(|bytes| bytes == b"\"title\":\"Demo\"")
    );
}

#[test]
fn packages_ti8xp_with_context_variable_name() {
    let context = PackageContext {
        ti8xp: Some(Ti8xpPackageOptions {
            variable_name: Some("demo-1".to_owned()),
        }),
        ..PackageContext::new()
    };
    let image = package_executable_with_context(
        &PackageRequest::new("ti84-z80", OutputFormat::Ti8xp, 0, 0),
        &context,
        &[0xc9],
    )
    .unwrap();
    assert_eq!(&image[..11], b"**TI83F*\x1A\x0A\x00");
    assert!(image.windows(8).any(|bytes| bytes == b"DEMO1\0\0\0"));
    assert!(image.windows(3).any(|bytes| bytes == [0xBB, 0x6D, 0xC9]));
}

#[test]
fn packages_zx_tap_with_resolved_executable_name() {
    let context = PackageContext {
        executable_name: Some("demo-game".to_owned()),
        ..PackageContext::new()
    };
    let image = package_executable_with_context(
        &PackageRequest::new(
            "zxspectrum-z80-48k",
            OutputFormat::ZxSpectrumTap,
            0x8000,
            0x8000,
        ),
        &context,
        &[0xc9],
    )
    .unwrap();
    assert_eq!(&image[4..14], b"DEMO_GAME ");
    assert!(image.windows(1).any(|bytes| bytes == [0xc9]));
}

#[test]
fn packages_game_boy_with_resolved_bank_payloads() {
    let context = PackageContext {
        executable_name: Some("Demo Game".to_owned()),
        game_boy: Some(GameBoyPackageOptions {
            mapper: GameBoyMapper::Mbc1,
            rom_banks: Some(4),
            ram_banks: 0,
            battery: false,
            rumble: false,
            bank_payloads: vec![vec![0x42]],
            generated_bank_payloads: vec![GameBoyBankPayload {
                bank: 3,
                bytes: vec![0x24],
            }],
            explicit_banking: true,
        }),
        ..PackageContext::new()
    };
    let image = package_executable_with_context(
        &PackageRequest::new("gameboy-lr35902", OutputFormat::GameBoyGb, 0x0150, 0x0150),
        &context,
        &[0xc9],
    )
    .unwrap();
    assert_eq!(image.len(), 0x10000);
    assert_eq!(&image[0x0134..0x013D], b"Demo Game");
    assert_eq!(image[0x8000], 0x42);
    assert_eq!(image[0xC000], 0x24);
}

#[test]
fn packages_msp430_as_little_endian_elf32() {
    let request = PackageRequest::new("msp430-none-elf", OutputFormat::Elf32, 0xC000, 0xC004);
    let context = PackageContext {
        image_kind: PackageImageKind::LoadImage,
        ..PackageContext::new()
    };
    let image =
        package_executable_with_context(&request, &context, &[0x30, 0x40, 0x02, 0x43]).unwrap();

    assert_eq!(&image[..4], b"\x7FELF");
    assert_eq!(image[4], 1); // ELFCLASS32
    assert_eq!(image[5], 1); // ELFDATA2LSB
    assert_eq!(u16::from_le_bytes([image[16], image[17]]), 2); // ET_EXEC
    assert_eq!(u16::from_le_bytes([image[18], image[19]]), 105); // EM_MSP430
    assert_eq!(
        u32::from_le_bytes(image[24..28].try_into().unwrap()),
        0xC004
    );
    assert_eq!(u16::from_le_bytes([image[44], image[45]]), 1);
    let code_offset = u32::from_le_bytes(image[52 + 4..52 + 8].try_into().unwrap()) as usize;
    let code_size = u32::from_le_bytes(image[52 + 16..52 + 20].try_into().unwrap()) as usize;
    assert_eq!(code_offset, 0x1000);
    assert_eq!(code_size, 4);
    assert_eq!(
        &image[code_offset..code_offset + code_size],
        &[0x30, 0x40, 0x02, 0x43]
    );
    assert!(
        image
            .windows(b".text".len())
            .any(|window| window == b".text")
    );
}

#[test]
fn packages_msp430x_elf32_with_machine_flags() {
    let request = PackageRequest::new("msp430x2-none-elf", OutputFormat::Elf32, 0xC000, 0xC000);
    let image = package_executable(&request, &[0x30, 0x40]).unwrap();
    assert_eq!(u32::from_le_bytes(image[36..40].try_into().unwrap()), 0x20);
}

#[test]
fn rejects_elf32_output_for_non_msp430_targets() {
    let error = package_executable(
        &PackageRequest::new("bare-z80", OutputFormat::Elf32, 0, 0),
        &[0],
    )
    .unwrap_err();
    assert!(
        error
            .message
            .contains("does not support MSP430 ELF32 output")
    );
}

#[test]
fn rejects_hex_output_for_agon_mos() {
    let error = package_executable(
        &PackageRequest::new(
            "agonlight-mos-ez80",
            OutputFormat::IntelHex,
            0x40000,
            0x40045,
        ),
        &[0],
    )
    .unwrap_err();
    assert!(
        error.message.contains("do not support Intel HEX"),
        "{error}"
    );
}

#[test]
fn intel_hex_uses_entry_or_load_address_for_the_image_kind() {
    let request = PackageRequest::new("bare-z80", OutputFormat::IntelHex, 0x1000, 0x2000);
    let entry = package_executable_with_context(&request, &PackageContext::new(), &[0xAA]).unwrap();
    assert!(
        entry
            .windows(b":01200000AA".len())
            .any(|bytes| bytes == b":01200000AA"),
        "{}",
        String::from_utf8_lossy(&entry)
    );

    let context = PackageContext {
        image_kind: PackageImageKind::LoadImage,
        ..PackageContext::new()
    };
    let load = package_executable_with_context(&request, &context, &[0xAA]).unwrap();
    assert!(
        load.windows(b":01100000AA".len())
            .any(|bytes| bytes == b":01100000AA"),
        "{}",
        String::from_utf8_lossy(&load)
    );
}

#[test]
fn rejects_intel_hex_payloads_that_overflow_the_address_space() {
    let request = PackageRequest::new(
        "bare-z80",
        OutputFormat::IntelHex,
        Address24::MAX,
        Address24::MAX,
    );
    let error = package_executable(&request, &[0xAA, 0xBB]).unwrap_err();
    assert!(
        error.message.contains("exceeds the 24-bit address space"),
        "{}",
        error.message
    );
}

#[test]
fn splits_intel_hex_records_at_64k_boundaries() {
    let request = PackageRequest::new("bare-z80", OutputFormat::IntelHex, 0xFFF8, 0xFFF8);
    let image = package_executable(&request, &[0; 16]).unwrap();
    let text = String::from_utf8(image).unwrap();
    assert!(text.contains(":08FFF800"), "{text}");
    assert!(text.contains(":080000000000000000000000F8"), "{text}");
}

#[test]
fn packages_all_ez180n_cpu_ids_before_the_payload() {
    for (target, cpu_id) in [
        ("ez180n-i8080", 0),
        ("ez180n-i8085", 1),
        ("ez180n-z80", 2),
        ("ez180n-z80n", 3),
        ("ez180n-z180", 4),
        ("ez180n-ez80", 5),
    ] {
        let payload = [0xC9, 0x42];
        let packaged = package_executable(
            &PackageRequest::new(target, OutputFormat::Ez180nGaem, 0, 0),
            &payload,
        )
        .unwrap();

        assert_eq!(&packaged[..4], b"EZRA");
        assert_eq!(packaged[4], cpu_id);
        assert_eq!(&packaged[5..], payload);
    }
}

#[test]
fn rejects_gaem_output_for_non_ez180n_targets() {
    let error = package_executable(
        &PackageRequest::new("bare-z80", OutputFormat::Ez180nGaem, 0, 0),
        &[0xC9],
    )
    .unwrap_err();
    assert!(error.message.contains("not a supported ez180N CPU target"));
}
