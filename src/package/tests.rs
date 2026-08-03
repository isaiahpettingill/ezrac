    use super::*;

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
