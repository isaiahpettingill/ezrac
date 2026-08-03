    use super::*;

    fn words(bytes: &[u8]) -> Vec<u16> {
        bytes
            .chunks_exact(2)
            .map(|word| u16::from_le_bytes([word[0], word[1]]))
            .collect()
    }

    #[test]
    fn encodes_every_basic_opcode_form() {
        let labels = HashMap::new();
        let cases = [
            ("set", 0x01),
            ("add", 0x02),
            ("sub", 0x03),
            ("mul", 0x04),
            ("mli", 0x05),
            ("div", 0x06),
            ("dvi", 0x07),
            ("mod", 0x08),
            ("mdi", 0x09),
            ("and", 0x0a),
            ("bor", 0x0b),
            ("xor", 0x0c),
            ("shr", 0x0d),
            ("asr", 0x0e),
            ("shl", 0x0f),
            ("ifb", 0x10),
            ("ifc", 0x11),
            ("ife", 0x12),
            ("ifn", 0x13),
            ("ifg", 0x14),
            ("ifa", 0x15),
            ("ifl", 0x16),
            ("ifu", 0x17),
            ("adx", 0x1a),
            ("sbx", 0x1b),
            ("sti", 0x1e),
            ("std", 0x1f),
        ];

        for (mnemonic, opcode) in cases {
            let source = format!("{mnemonic} b, a");
            assert_eq!(
                words(&encode_instruction(&source, &labels, 0).unwrap()),
                [opcode | 0x20]
            );
        }
    }

    #[test]
    fn encodes_every_special_opcode_form() {
        let labels = HashMap::new();
        for (mnemonic, opcode) in [
            ("jsr", 0x01),
            ("int", 0x08),
            ("iag", 0x09),
            ("ias", 0x0a),
            ("rfi", 0x0b),
            ("iaq", 0x0c),
            ("hwn", 0x10),
            ("hwq", 0x11),
            ("hwi", 0x12),
        ] {
            let source = format!("{mnemonic} a");
            assert_eq!(
                words(&encode_instruction(&source, &labels, 0).unwrap()),
                [opcode << 5]
            );
        }
    }

    #[test]
    fn encodes_all_a_operand_encodings() {
        let labels = HashMap::new();
        let cases = [
            ("a", 0x00, None),
            ("b", 0x01, None),
            ("c", 0x02, None),
            ("x", 0x03, None),
            ("y", 0x04, None),
            ("z", 0x05, None),
            ("i", 0x06, None),
            ("j", 0x07, None),
            ("[a]", 0x08, None),
            ("[b]", 0x09, None),
            ("[c]", 0x0a, None),
            ("[x]", 0x0b, None),
            ("[y]", 0x0c, None),
            ("[z]", 0x0d, None),
            ("[i]", 0x0e, None),
            ("[j]", 0x0f, None),
            ("[0x1111+a]", 0x10, Some(0x1111)),
            ("[b + 0x1112]", 0x11, Some(0x1112)),
            ("[0x1113 + c]", 0x12, Some(0x1113)),
            ("[x+0x1114]", 0x13, Some(0x1114)),
            ("[0x1115+y]", 0x14, Some(0x1115)),
            ("[z+0x1116]", 0x15, Some(0x1116)),
            ("[0x1117+i]", 0x16, Some(0x1117)),
            ("[j+0x1118]", 0x17, Some(0x1118)),
            ("pop", 0x18, None),
            ("peek", 0x19, None),
            ("pick 0x1119", 0x1a, Some(0x1119)),
            ("[sp + 0x111a]", 0x1a, Some(0x111a)),
            ("sp", 0x1b, None),
            ("pc", 0x1c, None),
            ("ex", 0x1d, None),
            ("[0x111e]", 0x1e, Some(0x111e)),
            ("0x111f", 0x1f, Some(0x111f)),
            ("0xffff", 0x20, None),
        ];

        for (operand, code, extra) in cases {
            let source = format!("set b, {operand}");
            let encoded = words(&encode_instruction(&source, &labels, 0).unwrap());
            assert_eq!(encoded[0], 0x01 | (0x01 << 5) | (code << 10), "{source}");
            assert_eq!(encoded.get(1).copied(), extra, "{source}");
        }
        for value in 0..=30 {
            let encoded =
                words(&encode_instruction(&format!("set b, {value}"), &labels, 0).unwrap());
            assert_eq!(encoded, [0x01 | (0x01 << 5) | ((0x21 + value) << 10)]);
        }
        assert_eq!(
            words(&encode_instruction("set b, -1", &labels, 0).unwrap()),
            [0x01 | (0x01 << 5) | (0x20 << 10)],
        );
    }

    #[test]
    fn encodes_all_b_operand_encodings() {
        let labels = HashMap::new();
        let cases = [
            ("a", 0x00),
            ("b", 0x01),
            ("c", 0x02),
            ("x", 0x03),
            ("y", 0x04),
            ("z", 0x05),
            ("i", 0x06),
            ("j", 0x07),
            ("[a]", 0x08),
            ("[b]", 0x09),
            ("[c]", 0x0a),
            ("[x]", 0x0b),
            ("[y]", 0x0c),
            ("[z]", 0x0d),
            ("[i]", 0x0e),
            ("[j]", 0x0f),
            ("[0x1000+a]", 0x10),
            ("[0x1000+b]", 0x11),
            ("[0x1000+c]", 0x12),
            ("[0x1000+x]", 0x13),
            ("[0x1000+y]", 0x14),
            ("[0x1000+z]", 0x15),
            ("[0x1000+i]", 0x16),
            ("[0x1000+j]", 0x17),
            ("push", 0x18),
            ("peek", 0x19),
            ("pick 1", 0x1a),
            ("sp", 0x1b),
            ("pc", 0x1c),
            ("ex", 0x1d),
            ("[0x1000]", 0x1e),
            ("0x1000", 0x1f),
        ];

        for (operand, code) in cases {
            let source = format!("set {operand}, a");
            let encoded = words(&encode_instruction(&source, &labels, 0).unwrap());
            assert_eq!(encoded[0], 0x01 | (code << 5), "{source}");
        }
    }

    #[test]
    fn preserves_extra_word_order_and_label_word_addresses() {
        let labels = HashMap::from([("Destination".to_owned(), 0x1234)]);
        assert_eq!(
            words(&encode_instruction("set [0x1111], 0x2222", &labels, 0).unwrap()),
            [0x7fc1, 0x1111, 0x2222],
        );
        assert_eq!(
            words(&encode_instruction("JSR destination", &labels, 0).unwrap()),
            [0x7c20, 0x091a],
        );
        assert_eq!(instruction_len("set [symbol], symbol").unwrap(), 6);
        assert_eq!(instruction_len("hwi symbol").unwrap(), 4);
    }

    #[test]
    fn evaluates_constant_and_word_address_expressions() {
        let labels = HashMap::from([("Destination".to_owned(), 0x0020)]);

        assert_eq!(
            words(&encode_instruction("set a, (2 + 3) * 4", &labels, 0).unwrap()),
            [0x01 | ((0x21 + 20) << 10)],
        );
        assert_eq!(
            words(&encode_instruction("set a, ~0", &labels, 0).unwrap()),
            [0x01 | (0x20 << 10)],
        );
        assert_eq!(
            words(&encode_instruction("set a, 1 << 4", &labels, 0).unwrap()),
            [0x01 | ((0x21 + 16) << 10)],
        );
        assert_eq!(
            words(&encode_instruction("set a, destination + 2", &labels, 0).unwrap()),
            [0x7c01, 0x0012],
        );
        assert_eq!(instruction_len("set a, 32 / destination").unwrap(), 4);
        assert_eq!(
            words(&encode_instruction("set a, 32 / destination", &labels, 0).unwrap()),
            [0x7c01, 0x0002],
        );
        assert_eq!(
            words(&encode_instruction("set a, $ + 2", &labels, 0x20).unwrap()),
            [0x7c01, 0x0012],
        );
        assert_eq!(
            words(&encode_instruction("set a, [destination + 2 + i]", &labels, 0).unwrap()),
            [0x5801, 0x0012],
        );
    }

    #[test]
    fn rejects_invalid_expressions_and_register_addresses() {
        let labels = HashMap::new();
        for source in [
            "set a, 1 / 0",
            "set a, 1 << 128",
            "set a, [a + b]",
            "set a, [a * 2]",
        ] {
            assert!(encode_instruction(source, &labels, 0).is_err(), "{source}");
        }
    }

    #[test]
    fn rejects_invalid_operand_positions_and_arity() {
        let labels = HashMap::new();
        for source in [
            "set 0, a",
            "set 0xffff, a",
            "set pop, a",
            "set a, push",
            "set a",
            "set a, b, c",
            "int",
            "int a, b",
            "rfi ,",
            "unknown a",
        ] {
            assert!(encode_instruction(source, &labels, 0).is_err(), "{source}");
            assert!(instruction_len(source).is_err(), "{source}");
        }
    }
