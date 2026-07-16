use super::*;

pub(super) static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) struct CurrentDirGuard {
    previous: PathBuf,
}

impl CurrentDirGuard {
    pub(super) fn switch_to(path: &Path) -> Self {
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(path).unwrap();
        Self { previous }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.previous).unwrap();
    }
}

pub(super) fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "ezra_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

pub(super) fn assert_ti8xp(bytes: &[u8], name: &[u8; 8], program_prefix: &[u8]) {
    assert!(bytes.starts_with(b"**TI83F*\x1A\x0A\x00"), "{bytes:02X?}");
    assert_eq!(u16::from_le_bytes([bytes[55], bytes[56]]), 13);
    let payload_len = u16::from_le_bytes([bytes[57], bytes[58]]) as usize;
    assert_eq!(bytes[59], 0x06);
    assert_eq!(&bytes[60..68], name);
    assert_eq!(
        u16::from_le_bytes([bytes[70], bytes[71]]) as usize,
        payload_len
    );
    let payload_start = 72;
    let program_len = u16::from_le_bytes([bytes[72], bytes[73]]) as usize;
    assert_eq!(program_len + 2, payload_len);
    assert!(
        bytes[74..74 + program_len].starts_with(program_prefix),
        "{bytes:02X?}"
    );
    let checksum_offset = payload_start + payload_len;
    let expected = bytes[55..checksum_offset]
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
    let actual = u16::from_le_bytes([bytes[checksum_offset], bytes[checksum_offset + 1]]);
    assert_eq!(actual, expected);
}

pub(super) fn assert_ti_app(
    bytes: &[u8],
    kind: u8,
    name: &[u8; 8],
    entry: u32,
    payload_prefix: &[u8],
) {
    assert!(bytes.starts_with(b"**TIFL**\x1A\x0A\x00"), "{bytes:02X?}");
    assert_eq!(bytes[11], kind);
    assert_eq!(&bytes[12..20], name);
    assert_eq!(
        u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        entry
    );
    assert!(bytes[64..].starts_with(payload_prefix), "{bytes:02X?}");
}

pub(super) fn copy_fixture(root: &Path, name: &str) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("harness")
        .join(name);
    let destination = root.join(name);
    std::fs::copy(&source, &destination).unwrap_or_else(|error| {
        panic!(
            "failed to copy fixture {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
    destination
}
