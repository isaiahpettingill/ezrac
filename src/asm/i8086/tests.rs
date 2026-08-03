    use super::*;
    use crate::target::AssemblerCpu;
    use crate::vm::assemble_subset_with_symbols_at;

    fn encode_one(source: &str) -> Vec<u8> {
        encode_instruction(source, &HashMap::new(), 0x1000).unwrap()
    }

    #[test]
    fn golden_fixed_and_implicit_instructions() {
        for (source, expected) in [
            ("aaa", vec![0x37]),
            ("aad", vec![0xD5, 0x0A]),
            ("aam", vec![0xD4, 0x0A]),
            ("aas", vec![0x3F]),
            ("cbw", vec![0x98]),
            ("cwd", vec![0x99]),
            ("daa", vec![0x27]),
            ("das", vec![0x2F]),
            ("iret", vec![0xCF]),
            ("lahf", vec![0x9F]),
            ("sahf", vec![0x9E]),
            ("pushf", vec![0x9C]),
            ("popf", vec![0x9D]),
            ("wait", vec![0x9B]),
            ("hlt", vec![0xF4]),
            ("cmc", vec![0xF5]),
            ("clc", vec![0xF8]),
            ("stc", vec![0xF9]),
            ("cli", vec![0xFA]),
            ("sti", vec![0xFB]),
            ("cld", vec![0xFC]),
            ("std", vec![0xFD]),
        ] {
            assert_eq!(encode_one(source), expected, "{source}");
        }
    }

    #[test]
    fn golden_covers_alu_data_stack_and_io_forms() {
        let cases: &[(&str, &[u8])] = &[
            ("add al,7fh", &[0x04, 0x7F]),
            ("or ax,1234h", &[0x0D, 0x34, 0x12]),
            ("adc [bx+si],cl", &[0x10, 0x08]),
            ("sbb dx,[bp+di-2]", &[0x1B, 0x53, 0xFE]),
            ("and word ptr [bx],-1", &[0x83, 0x27, 0xFF]),
            (
                "sub word ptr [1234h],128",
                &[0x81, 0x2E, 0x34, 0x12, 0x80, 0x00],
            ),
            ("xor bh,byte ptr es:[di+4]", &[0x26, 0x32, 0x7D, 0x04]),
            ("cmp word ptr [bp],1", &[0x83, 0x7E, 0x00, 0x01]),
            ("mov ax,[1234h]", &[0xA1, 0x34, 0x12]),
            ("mov [1234h],al", &[0xA2, 0x34, 0x12]),
            ("mov ds,ax", &[0x8E, 0xD8]),
            ("mov ax,cs", &[0x8C, 0xC8]),
            ("xchg ax,di", &[0x97]),
            ("test byte ptr [si],80h", &[0xF6, 0x04, 0x80]),
            ("lea bx,[bp+si+1234h]", &[0x8D, 0x9A, 0x34, 0x12]),
            ("les di,far ptr [bx]", &[0xC4, 0x3F]),
            ("push ds", &[0x1E]),
            ("pop word ptr [bx]", &[0x8F, 0x07]),
            ("in al,20h", &[0xE4, 0x20]),
            ("out dx,ax", &[0xEF]),
        ];
        for (source, expected) in cases {
            assert_eq!(encode_one(source), *expected, "{source}");
        }
    }

    #[test]
    fn golden_covers_unary_shift_control_string_and_escape_forms() {
        let cases: &[(&str, &[u8])] = &[
            ("inc ax", &[0x40]),
            ("dec byte ptr [bx]", &[0xFE, 0x0F]),
            ("not word ptr [si]", &[0xF7, 0x14]),
            ("neg bl", &[0xF6, 0xDB]),
            ("mul word ptr [di]", &[0xF7, 0x25]),
            ("idiv ch", &[0xF6, 0xFD]),
            ("rol byte ptr [bx],1", &[0xD0, 0x07]),
            ("sar ax,cl", &[0xD3, 0xF8]),
            ("call 1100h", &[0xE8, 0xFD, 0x00]),
            ("jmp short 0ff0h", &[0xEB, 0xEE]),
            ("call far 1234h:5678h", &[0x9A, 0x78, 0x56, 0x34, 0x12]),
            ("jmp far ptr [bx]", &[0xFF, 0x2F]),
            ("ret 4", &[0xC2, 0x04, 0x00]),
            ("retf", &[0xCB]),
            ("int 3", &[0xCC]),
            ("rep movsw", &[0xF3, 0xA5]),
            ("repne scasb", &[0xF2, 0xAE]),
            ("rep ds: movsb", &[0xF3, 0x3E, 0xA4]),
            ("lock add word ptr [bx],1", &[0xF0, 0x83, 0x07, 0x01]),
            ("esc 63,[bp+di]", &[0xDF, 0x3B]),
            ("esc 0,7", &[0xD8, 0xC7]),
        ];
        for (source, expected) in cases {
            assert_eq!(encode_one(source), *expected, "{source}");
        }
    }

    #[test]
    fn all_effective_address_modes_and_displacement_sizes_encode() {
        for (address, modrm) in [
            ("[bx+si]", 0x00),
            ("[bx+di]", 0x01),
            ("[bp+si]", 0x02),
            ("[bp+di]", 0x03),
            ("[si]", 0x04),
            ("[di]", 0x05),
            ("[bx]", 0x07),
        ] {
            assert_eq!(encode_one(&format!("mov ax,{address}")), [0x8B, modrm]);
        }
        assert_eq!(encode_one("mov ax,[bp]"), [0x8B, 0x46, 0x00]);
        assert_eq!(encode_one("mov ax,[bx+127]"), [0x8B, 0x47, 0x7F]);
        assert_eq!(encode_one("mov ax,[bx+128]"), [0x8B, 0x87, 0x80, 0x00]);
        assert_eq!(instruction_len("mov ax,[bx+symbol]").unwrap(), 4);
    }

    #[test]
    fn labels_and_every_short_branch_alias_resolve() {
        let source = "start:\n jz next\n loop start\nnext:\n jmp near start\n";
        let assembled =
            assemble_subset_with_symbols_at(AssemblerCpu::I8086, source, 0x1000).unwrap();
        assert_eq!(assembled.bytes, [0x74, 0x02, 0xE2, 0xFC, 0xE9, 0xF9, 0xFF]);
        for alias in [
            "jo", "jno", "jb", "jc", "jnae", "jae", "jnb", "jnc", "je", "jz", "jne", "jnz", "jbe",
            "jna", "ja", "jnbe", "js", "jns", "jp", "jpe", "jnp", "jpo", "jl", "jnge", "jge",
            "jnl", "jle", "jng", "jg", "jnle",
        ] {
            assert_eq!(
                instruction_len(&format!("{alias} 0")).unwrap(),
                2,
                "{alias}"
            );
        }
    }

    #[test]
    fn documented_mnemonic_matrix_is_complete() {
        let forms = [
            "aaa",
            "aad",
            "aam",
            "aas",
            "adc ax,bx",
            "add ax,bx",
            "and ax,bx",
            "call 1003h",
            "call far 1:2",
            "call bx",
            "call far ptr [bx]",
            "cbw",
            "clc",
            "cld",
            "cli",
            "cmc",
            "cmp ax,bx",
            "cmpsb",
            "cmpsw",
            "cwd",
            "daa",
            "das",
            "dec ax",
            "div ax",
            "hlt",
            "idiv ax",
            "imul ax",
            "in al,1",
            "in ax,dx",
            "inc ax",
            "int 4",
            "int3",
            "into",
            "iret",
            "jcxz 2",
            "jmp 1003h",
            "jmp short 2",
            "jmp far 1:2",
            "jmp bx",
            "jmp far ptr [bx]",
            "lahf",
            "lds ax,[bx]",
            "lea ax,[bx]",
            "les ax,[bx]",
            "lodsb",
            "lodsw",
            "loop 2",
            "loope 2",
            "loopne 2",
            "loopnz 2",
            "loopz 2",
            "mov ax,bx",
            "mov ax,1",
            "mov ax,[1]",
            "mov [1],ax",
            "mov ds,ax",
            "mov ax,ds",
            "movsb",
            "movsw",
            "mul ax",
            "neg ax",
            "nop",
            "not ax",
            "or ax,bx",
            "out 1,al",
            "out dx,ax",
            "pop ax",
            "pop ds",
            "pop word ptr [bx]",
            "popf",
            "push ax",
            "push cs",
            "push word ptr [bx]",
            "pushf",
            "rcl ax,1",
            "rcr ax,cl",
            "ret",
            "ret 2",
            "retn",
            "retf",
            "rol ax,1",
            "ror ax,cl",
            "sahf",
            "sal ax,1",
            "sar ax,cl",
            "sbb ax,bx",
            "scasb",
            "scasw",
            "shl ax,1",
            "shr ax,cl",
            "stc",
            "std",
            "sti",
            "stosb",
            "stosw",
            "sub ax,bx",
            "test ax,bx",
            "test ax,1",
            "wait",
            "xchg ax,bx",
            "xlat",
            "xor ax,bx",
            "esc 0,[bx]",
        ];
        for form in forms {
            instruction_len(form).unwrap_or_else(|error| panic!("{form}: {error}"));
        }

        for (mnemonic, opcode) in [
            ("jo", 0x70),
            ("jno", 0x71),
            ("jb", 0x72),
            ("jae", 0x73),
            ("je", 0x74),
            ("jne", 0x75),
            ("jbe", 0x76),
            ("ja", 0x77),
            ("js", 0x78),
            ("jns", 0x79),
            ("jp", 0x7A),
            ("jnp", 0x7B),
            ("jl", 0x7C),
            ("jge", 0x7D),
            ("jle", 0x7E),
            ("jg", 0x7F),
        ] {
            assert_eq!(encode_one(&format!("{mnemonic} 1002h")), [opcode, 0]);
        }
        for (mnemonic, opcode) in [
            ("movsb", 0xA4),
            ("movsw", 0xA5),
            ("cmpsb", 0xA6),
            ("cmpsw", 0xA7),
            ("stosb", 0xAA),
            ("stosw", 0xAB),
            ("lodsb", 0xAC),
            ("lodsw", 0xAD),
            ("scasb", 0xAE),
            ("scasw", 0xAF),
        ] {
            assert_eq!(encode_one(mnemonic), [opcode]);
        }
    }

    #[test]
    fn prefixed_segments_and_numeric_branches_work_through_both_passes() {
        let assembled = assemble_subset_with_symbols_at(
            AssemblerCpu::I8086,
            "rep ds: movsb\njmp short 1000h\n",
            0x1000,
        )
        .unwrap();
        assert_eq!(assembled.bytes, [0xF3, 0x3E, 0xA4, 0xEB, 0xFB]);

        assert_eq!(
            encode_instruction("jmp near 9000h", &HashMap::new(), 0,).unwrap(),
            [0xE9, 0xFD, 0x8F]
        );
        assert_eq!(
            encode_instruction("jmp short 0", &HashMap::new(), 0xFFFE).unwrap(),
            [0xEB, 0x00]
        );
    }

    #[test]
    fn symbolic_sizes_remain_stable_between_assembly_passes() {
        let source = "vector equ 3\nint vector\nmov ax,[bx+target]\ntarget:\nnop\n";
        let assembled =
            assemble_subset_with_symbols_at(AssemblerCpu::I8086, source, 0x1000).unwrap();
        assert_eq!(assembled.bytes, [0xCD, 0x03, 0x8B, 0x87, 0x06, 0x10, 0x90]);
    }

    #[test]
    fn rejects_post_8086_reserved_and_invalid_forms() {
        for source in [
            "pusha",
            "push 1",
            "imul ax,bx",
            "shl ax,2",
            "enter 4,0",
            "leave",
            "insb",
            "mov eax,1",
            "mov cs,ax",
            "pop cs",
            "lea ax,bx",
            "jmp far ax",
            "lock cmp word ptr [bx],1",
            "lock add ax,1",
            "lock shl word ptr [bx],1",
            "repne movsb",
            "rep nop",
            "xchg ax,al",
            "xchg al,ax",
            "mov ax,byte ptr [1234h]",
            "mov al,word ptr [1234h]",
            "mov byte ptr [1234h],ax",
            "mov word ptr [1234h],al",
            "mov short ax,bx",
            "add ax,far ptr [bx]",
            "call short bx",
            "jmp short bx",
            "jmp short 1234h:5678h",
            "call near 1234h:5678h",
            "jmp short far ptr [bx]",
            "mov ax,[sp]",
            "mov ax,[cs]",
            "mov ax,[bx*2]",
            "mov ax,[bx+bp]",
            "inc [bx]",
        ] {
            assert!(
                instruction_len(source).is_err(),
                "unexpectedly accepted `{source}`"
            );
        }
    }
