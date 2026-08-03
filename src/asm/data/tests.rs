    use super::*;

    #[test]
    fn formats_byte_records_and_escaped_text() {
        assert_eq!(
            byte_data_lines("db", &[0x12, 0xAB], ByteLiteralStyle::HexSuffix, 16),
            ["    db 12h, ABh"]
        );
        assert_eq!(
            terminated_text_data_line(".dm", "A\n\\\"", "00h"),
            "    .dm \"A\\n\\\\\\\"\", 00h"
        );
    }
