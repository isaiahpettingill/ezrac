    use super::*;

    fn labels() -> HashMap<String, u32> {
        HashMap::from([
            ("target".to_owned(), 0x1010),
            ("long".to_owned(), 0x1234_5678),
        ])
    }
    #[test]
    fn golden_instruction_and_effective_address_encodings() {
        let cases: &[(&str, &[u8])] = &[
            ("move.b d0,(a1)+", &[0x12, 0xc0]),
            (
                "move.l #$12345678,d0",
                &[0x20, 0x3c, 0x12, 0x34, 0x56, 0x78],
            ),
            ("move.w (4,a0,d1.w),d2", &[0x34, 0x30, 0x10, 0x04]),
            ("move.w (4,pc,d1.l),d2", &[0x34, 0x3b, 0x18, 0x04]),
            ("move.w $1234.w,d0", &[0x30, 0x38, 0x12, 0x34]),
            (
                "move.l $12345678.l,d0",
                &[0x20, 0x39, 0x12, 0x34, 0x56, 0x78],
            ),
            ("addx.w d1,d2", &[0xd5, 0x41]),
            ("subx.b -(a1),-(a2)", &[0x95, 0x09]),
            ("abcd d1,d2", &[0xc5, 0x01]),
            ("movep.l d1,4(a2)", &[0x03, 0xca, 0x00, 0x04]),
            ("movem.l d0-d2/a1,-(sp)", &[0x48, 0xe7, 0x02, 0x07]),
            ("exg d1,a2", &[0xc3, 0x8a]),
            ("rol.w #8,d0", &[0xe1, 0x58]),
            ("move usp,a3", &[0x4e, 0x6b]),
            ("move ccr,(a0)", &[0x42, 0xd0]),
            ("move sr,(a0)", &[0x40, 0xd0]),
            ("move (a0),sr", &[0x46, 0xd0]),
        ];
        for (source, expected) in cases {
            assert_eq!(
                encode(source, &labels(), 0x1000, true).unwrap(),
                *expected,
                "{source}"
            );
            assert_eq!(instruction_len(source).unwrap(), expected.len(), "{source}");
        }
    }
    #[test]
    fn resolves_case_insensitive_labels_after_normalizing_instructions() {
        let labels = HashMap::from([("Target".to_owned(), 0x1010)]);
        assert!(encode("BNE target", &labels, 0x1000, true).is_ok());
        assert!(encode("JMP TARGET", &labels, 0x1000, true).is_ok());
    }

    #[test]
    fn every_official_family_has_a_table_driven_smoke_case() {
        let cases = [
            "abcd d0,d1",
            "add.b d0,d1",
            "adda.w d0,a1",
            "addi.b #1,d0",
            "addq.w #8,d0",
            "addx.l d0,d1",
            "and.w d0,d1",
            "andi.w #1,d0",
            "asl.w #1,d0",
            "asl (a0)",
            "bra target",
            "bsr target",
            "bne target",
            "bchg #1,d0",
            "bclr d0,(a0)",
            "bset #1,(a0)",
            "btst d0,(a0)",
            "chk (a0),d0",
            "clr.w d0",
            "cmp.w d0,d1",
            "cmpa.w d0,a1",
            "cmpi.w #1,d0",
            "cmpm.w (a0)+,(a1)+",
            "dbra d0,target",
            "divs (a0),d0",
            "divu (a0),d0",
            "eor.w d0,d1",
            "eori.w #1,d0",
            "exg d0,d1",
            "ext.w d0",
            "illegal",
            "jmp (a0)",
            "jsr (a0)",
            "lea (a0),a1",
            "link a0,#-4",
            "lsl.w #1,d0",
            "lsr.l #8,d0",
            "lsr (a0)",
            "move.w d0,d1",
            "movea.w d0,a1",
            "move ccr,(a0)",
            "move (a0),ccr",
            "move sr,(a0)",
            "move (a0),sr",
            "moveusp a0,usp",
            "movem.w d0-d1,(a0)",
            "movep.w d0,4(a0)",
            "moveq #1,d0",
            "muls (a0),d0",
            "mulu (a0),d0",
            "nbcd d0",
            "neg.w d0",
            "negx.w d0",
            "nop",
            "not.w d0",
            "or.w d0,d1",
            "ori.w #1,d0",
            "pea (a0)",
            "reset",
            "rol.w #1,d0",
            "ror (a0)",
            "roxl.w #1,d0",
            "roxr (a0)",
            "rte",
            "rtr",
            "rts",
            "sbcd d0,d1",
            "seq d0",
            "stop #$2700",
            "sub.w d0,d1",
            "suba.w d0,a1",
            "subi.w #1,d0",
            "subq.w #1,d0",
            "subx.w d0,d1",
            "swap d0",
            "tas d0",
            "trap #15",
            "trapv",
            "tst.w d0",
            "unlk a0",
        ];
        for source in cases {
            assert!(encode(source, &labels(), 0x1000, true).is_ok(), "{source}");
        }
    }
    #[test]
    fn rejects_invalid_forms_and_boundaries() {
        for source in [
            "move.x d0,d1",
            "move.b a0,d0",
            "movea.b d0,a0",
            "addq.w #0,d0",
            "addq.w #9,d0",
            "trap #16",
            "asl.w #0,d0",
            "asl.w #9,d0",
            "movem.w d2-d0,(a0)",
            "movep.b d0,0(a0)",
            "move.w (128,a0,d0.w),d1",
            "move.w (0,a0,d0),d1",
            "exg d0,(a0)",
        ] {
            assert!(encode(source, &labels(), 0x1000, true).is_err(), "{source}");
        }
    }
