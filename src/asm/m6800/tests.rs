    use super::*;
    use crate::target::AssemblerCpu;
    use crate::vm::assemble_subset_with_symbols_at;

    fn labels() -> HashMap<String, u32> {
        HashMap::new()
    }

    #[test]
    fn golden_encodes_every_official_mnemonic_and_addressing_form() {
        // Opcode bases are transcribed from the Motorola M6800 programming manual.
        let inherent = [
            ("nop", 0x01),
            ("tap", 0x06),
            ("tpa", 0x07),
            ("inx", 0x08),
            ("dex", 0x09),
            ("clv", 0x0A),
            ("sev", 0x0B),
            ("clc", 0x0C),
            ("sec", 0x0D),
            ("cli", 0x0E),
            ("sei", 0x0F),
            ("sba", 0x10),
            ("cba", 0x11),
            ("tab", 0x16),
            ("tba", 0x17),
            ("daa", 0x19),
            ("aba", 0x1B),
            ("tsx", 0x30),
            ("ins", 0x31),
            ("pula", 0x32),
            ("pulb", 0x33),
            ("des", 0x34),
            ("txs", 0x35),
            ("psha", 0x36),
            ("pshb", 0x37),
            ("rts", 0x39),
            ("rti", 0x3B),
            ("wai", 0x3E),
            ("swi", 0x3F),
            ("nega", 0x40),
            ("coma", 0x43),
            ("lsra", 0x44),
            ("rora", 0x46),
            ("asra", 0x47),
            ("asla", 0x48),
            ("rola", 0x49),
            ("deca", 0x4A),
            ("inca", 0x4C),
            ("tsta", 0x4D),
            ("clra", 0x4F),
            ("negb", 0x50),
            ("comb", 0x53),
            ("lsrb", 0x54),
            ("rorb", 0x56),
            ("asrb", 0x57),
            ("aslb", 0x58),
            ("rolb", 0x59),
            ("decb", 0x5A),
            ("incb", 0x5C),
            ("tstb", 0x5D),
            ("clrb", 0x5F),
        ];
        for (mnemonic, opcode) in inherent {
            assert_eq!(
                emit_instruction(mnemonic, &labels(), 0x1000).unwrap(),
                Some(vec![opcode]),
                "{mnemonic}"
            );
            assert_eq!(instruction_len(mnemonic).unwrap(), Some(1), "{mnemonic}");
        }

        let branches = [
            ("bra", 0x20),
            ("brn", 0x21),
            ("bhi", 0x22),
            ("bls", 0x23),
            ("bcc", 0x24),
            ("bhs", 0x24),
            ("bcs", 0x25),
            ("blo", 0x25),
            ("bne", 0x26),
            ("beq", 0x27),
            ("bvc", 0x28),
            ("bvs", 0x29),
            ("bpl", 0x2A),
            ("bmi", 0x2B),
            ("bge", 0x2C),
            ("blt", 0x2D),
            ("bgt", 0x2E),
            ("ble", 0x2F),
            ("bsr", 0x8D),
        ];
        for (mnemonic, opcode) in branches {
            let source = format!("{mnemonic} 1080h");
            assert_eq!(
                emit_instruction(&source, &labels(), 0x1000).unwrap(),
                Some(vec![opcode, 0x7E]),
                "{source}"
            );
            assert_eq!(instruction_len(&source).unwrap(), Some(2), "{source}");
        }

        for (source, expected) in [
            ("jsr <12h", vec![0x9D, 0x12]),
            ("jsr 12h, x", vec![0xAD, 0x12]),
            ("jsr >1234h", vec![0xBD, 0x12, 0x34]),
        ] {
            assert_eq!(
                emit_instruction(source, &labels(), 0x1000).unwrap(),
                Some(expected),
                "{source}"
            );
        }

        let memory = [
            ("neg", 0x00),
            ("com", 0x03),
            ("lsr", 0x04),
            ("ror", 0x06),
            ("asr", 0x07),
            ("asl", 0x08),
            ("rol", 0x09),
            ("dec", 0x0A),
            ("inc", 0x0C),
            ("tst", 0x0D),
            ("jmp", 0x0E),
            ("clr", 0x0F),
        ];
        for (mnemonic, offset) in memory {
            for (operand, opcode, bytes) in [
                ("12h, x", 0x60 + offset, vec![0x60 + offset, 0x12]),
                (">1234h", 0x70 + offset, vec![0x70 + offset, 0x12, 0x34]),
            ] {
                let source = format!("{mnemonic} {operand}");
                assert_eq!(
                    emit_instruction(&source, &labels(), 0x1000).unwrap(),
                    Some(bytes),
                    "{source}"
                );
                assert_eq!(
                    instruction_len(&source).unwrap(),
                    Some(if opcode < 0x70 { 2 } else { 3 }),
                    "{source}"
                );
            }
        }

        let accumulator = [
            ("suba", 0x80, false, false),
            ("cmpa", 0x81, false, false),
            ("sbca", 0x82, false, false),
            ("anda", 0x84, false, false),
            ("bita", 0x85, false, false),
            ("ldaa", 0x86, false, false),
            ("staa", 0x87, false, true),
            ("eora", 0x88, false, false),
            ("adca", 0x89, false, false),
            ("oraa", 0x8A, false, false),
            ("adda", 0x8B, false, false),
            ("cpx", 0x8C, true, false),
            ("lds", 0x8E, true, false),
            ("sts", 0x8F, true, true),
            ("subb", 0xC0, false, false),
            ("cmpb", 0xC1, false, false),
            ("sbcb", 0xC2, false, false),
            ("andb", 0xC4, false, false),
            ("bitb", 0xC5, false, false),
            ("ldab", 0xC6, false, false),
            ("stab", 0xC7, false, true),
            ("eorb", 0xC8, false, false),
            ("adcb", 0xC9, false, false),
            ("orab", 0xCA, false, false),
            ("addb", 0xCB, false, false),
            ("ldx", 0xCE, true, false),
            ("stx", 0xCF, true, true),
        ];
        for (mnemonic, base, word, store) in accumulator {
            let immediate = if word { "#1234h" } else { "#12h" };
            let forms = if store {
                vec![("<12h", 0x10), ("12h, x", 0x20), (">1234h", 0x30)]
            } else {
                vec![
                    (immediate, 0x00),
                    ("<12h", 0x10),
                    ("12h, x", 0x20),
                    (">1234h", 0x30),
                ]
            };
            for (operand, add) in forms {
                let source = format!("{mnemonic} {operand}");
                let mut expected = vec![base + add];
                if add == 0 && word {
                    expected.extend([0x12, 0x34]);
                } else if add == 0 || add == 0x10 || add == 0x20 {
                    expected.push(0x12);
                } else {
                    expected.extend([0x12, 0x34]);
                }
                assert_eq!(
                    emit_instruction(&source, &labels(), 0x1000).unwrap(),
                    Some(expected.clone()),
                    "{source}"
                );
                assert_eq!(
                    instruction_len(&source).unwrap(),
                    Some(expected.len()),
                    "{source}"
                );
            }
        }

        for (source, expected) in [
            ("lsla", vec![0x48]),
            ("lslb", vec![0x58]),
            ("lsl 12h, x", vec![0x68, 0x12]),
            ("lsl >1234h", vec![0x78, 0x12, 0x34]),
        ] {
            assert_eq!(
                emit_instruction(source, &labels(), 0x1000).unwrap(),
                Some(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn validates_operand_and_branch_boundaries() {
        let labels = labels();
        for (source, expected) in [
            ("ldaa #ffh", vec![0x86, 0xFF]),
            ("cpx #ffffh", vec![0x8C, 0xFF, 0xFF]),
            ("staa <ffh", vec![0x97, 0xFF]),
            ("ldaa ffh, x", vec![0xA6, 0xFF]),
            ("jmp >ffffh", vec![0x7E, 0xFF, 0xFF]),
            ("bra f82h", vec![0x20, 0x80]),
            ("bra 1081h", vec![0x20, 0x7F]),
        ] {
            assert_eq!(
                emit_instruction(source, &labels, 0x1000).unwrap(),
                Some(expected),
                "{source}"
            );
        }
        for source in [
            "ldaa #100h",
            "cpx #10000h",
            "staa <100h",
            "ldaa 100h, x",
            "jmp >10000h",
            "bra f81h",
            "bra 1082h",
            "bra 10000h",
            "ldaa ,x",
        ] {
            assert!(
                assemble_subset_with_symbols_at(AssemblerCpu::M6800, source, 0x1000).is_err(),
                "{source}"
            );
        }
    }

    #[test]
    #[cfg(feature = "m6800")]
    fn resolves_case_insensitive_labels_equates_and_expressions() {
        let program = assemble_subset_with_symbols_at(
            AssemblerCpu::M6800,
            "origin equ 20h\nnext = origin + 2\nStart:\n ldaa #ORIGIN + 1\n staa <NEXT\n bra start\n",
            0x1000,
        )
        .unwrap();
        assert_eq!(program.bytes, [0x86, 0x21, 0x97, 0x22, 0x20, 0xFA]);

        let labels = HashMap::from([("destination".to_owned(), 0x1234)]);
        assert_eq!(
            emit_instruction("ldx destination", &labels, 0x1000).unwrap(),
            Some(vec![0xFE, 0x12, 0x34])
        );
        assert_eq!(
            emit_instruction("ldaa $ + 2", &labels, 0x1000).unwrap(),
            Some(vec![0xB6, 0x10, 0x02])
        );
        assert_eq!(
            emit_instruction("ldaa $+2", &labels, 0x1000).unwrap(),
            Some(vec![0xB6, 0x10, 0x02])
        );
    }

    #[test]
    fn rejects_invalid_mnemonics_and_addressing_forms() {
        for source in [
            "ldaa",
            "staa #1",
            "jmp <10h",
            "neg <10h",
            "bra #1000h",
            "inx 1",
            "ldaa 1,y",
            "asld",
            "ld a, 1",
        ] {
            assert!(
                assemble_subset_with_symbols_at(AssemblerCpu::M6800, source, 0x1000).is_err(),
                "{source}"
            );
        }
    }
