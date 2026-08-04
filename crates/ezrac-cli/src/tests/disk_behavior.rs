use super::*;

#[test]
fn disk_command_reads_host_files_and_writes_fat_image() {
    let root = temp_root("disk_command");
    std::fs::create_dir_all(&root).unwrap();
    let program = root.join("program.com");
    let readme = root.join("notes.txt");
    std::fs::write(&program, [0xc3, 0x00, 0x01]).unwrap();
    std::fs::write(&readme, b"support file\r\n").unwrap();
    let output = root.join("cpm.dsk");

    create_disk(&DiskCommandOptions {
        output: output.clone(),
        format: DiskFormat::Fat12_720K,
        label: "EZRA CPM".to_owned(),
        files: vec![
            DiskInput {
                name: "EZRA.COM".to_owned(),
                path: program,
            },
            DiskInput {
                name: "README.TXT".to_owned(),
                path: readme,
            },
        ],
    })
    .unwrap();

    let image = std::fs::read(&output).unwrap();
    assert_eq!(image.len(), 737_280);
    let root_directory = 7 * 512;
    assert_eq!(&image[root_directory..root_directory + 11], b"EZRA CPM   ");
    assert_eq!(
        &image[root_directory + 32..root_directory + 43],
        b"EZRA    COM"
    );
    assert_eq!(
        &image[root_directory + 64..root_directory + 75],
        b"README  TXT"
    );

    let _ = std::fs::remove_dir_all(root);
}
