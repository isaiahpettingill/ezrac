use super::*;

#[test]
fn creates_m35fd_fat12_image_with_named_files() {
    let files = [
        DiskFile::new("BOOT.BIN", &[0x34, 0x12, 0x78, 0x56]),
        DiskFile::new("README.TXT", b"M35FD support file\r\n"),
    ];
    let image =
        create_disk_image(&DiskRequest::new(DiskFormat::M35Fd, "EZRA DCPU", &files)).unwrap();

    assert_eq!(image.len(), 448 * 1024);
    assert_eq!(&image[11..13], &1024u16.to_le_bytes());
    assert_eq!(&image[19..21], &448u16.to_le_bytes());
    assert_eq!(&image[24..26], &7u16.to_le_bytes());
    assert_eq!(&image[26..28], &2u16.to_le_bytes());
    assert_eq!(&image[510..512], &[0x55, 0xaa]);

    let root = 3 * 1024;
    assert_eq!(&image[root..root + 11], b"EZRA DCPU  ");
    assert_eq!(&image[root + 32..root + 43], b"BOOT    BIN");
    assert_eq!(&image[root + 64..root + 75], b"README  TXT");
    let data = 7 * 1024;
    assert_eq!(&image[data..data + 4], &[0x34, 0x12, 0x78, 0x56]);
    assert_eq!(&image[data + 1024..data + 1044], b"M35FD support file\r\n");
}

#[test]
fn creates_big_endian_m35fd_by_swapping_each_word() {
    let files = [DiskFile::new("BOOT.BIN", &[0x34, 0x12])];
    let little = create_disk_image(&DiskRequest::new(DiskFormat::M35Fd, "EZRA", &files)).unwrap();
    let big = create_disk_image(&DiskRequest::new(
        DiskFormat::M35FdBigEndian,
        "EZRA",
        &files,
    ))
    .unwrap();

    assert_eq!(little.len(), big.len());
    for (little, big) in little.chunks_exact(2).zip(big.chunks_exact(2)) {
        assert_eq!(little, [big[1], big[0]]);
    }
}

#[test]
fn creates_standard_fat12_floppy_sizes_and_geometry() {
    let file = [DiskFile::new("EZRA.COM", &[0xc3, 0x00, 0x01])];
    let image_720 =
        create_disk_image(&DiskRequest::new(DiskFormat::Fat12_720K, "EZRA CPM", &file)).unwrap();
    assert_eq!(image_720.len(), 737_280);
    assert_eq!(image_720[13], 2);
    assert_eq!(&image_720[22..24], &3u16.to_le_bytes());
    assert_eq!(&image_720[24..26], &9u16.to_le_bytes());

    let image_1440 = create_disk_image(&DiskRequest::new(
        DiskFormat::Fat12_1440K,
        "EZRA DOS",
        &file,
    ))
    .unwrap();
    assert_eq!(image_1440.len(), 1_474_560);
    assert_eq!(image_1440[13], 1);
    assert_eq!(&image_1440[22..24], &9u16.to_le_bytes());
    assert_eq!(&image_1440[24..26], &18u16.to_le_bytes());
}

#[test]
fn creates_d64_with_prg_and_support_file_chains() {
    let program = (0..300).map(|value| value as u8).collect::<Vec<_>>();
    let files = [
        DiskFile::new("HELLO.PRG", &program),
        DiskFile::new("LEVEL.DAT", b"level one"),
    ];
    let image = create_disk_image(&DiskRequest::new(
        DiskFormat::Commodore1541,
        "EZRA DISK",
        &files,
    ))
    .unwrap();

    assert_eq!(image.len(), 174_848);
    let bam = d64_offset(18, 0);
    assert_eq!(&image[bam + 0x90..bam + 0x99], b"EZRA DISK");
    let directory = d64_offset(18, 1);
    assert_eq!(image[directory + 2], 0x82);
    assert_eq!(&image[directory + 5..directory + 14], b"HELLO.PRG");
    assert_eq!(image[directory + 32 + 2], 0x81);

    let first_track = image[directory + 3];
    let first_sector = image[directory + 4];
    let first = d64_offset(first_track, first_sector);
    assert_eq!(&image[first + 2..first + 256], &program[..254]);
    let second = d64_offset(image[first], image[first + 1]);
    assert_eq!(image[second], 0);
    assert_eq!(image[second + 1], 47);
    assert_eq!(&image[second + 2..second + 48], &program[254..]);
}

#[test]
fn rejects_names_that_collide_after_disk_encoding() {
    let files = [
        DiskFile::new("readme.txt", b"one"),
        DiskFile::new("README.TXT", b"two"),
    ];
    let error =
        create_disk_image(&DiskRequest::new(DiskFormat::Fat12_720K, "EZRA", &files)).unwrap_err();
    assert!(error.message.contains("duplicate FAT"));
}
