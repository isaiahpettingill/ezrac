
use super::*;

fn enc(text: &str, variant: Mos6502Variant) -> Vec<u8> {
    encode(text, &HashMap::new(), 0, false, variant).unwrap()
}

fn enc_at(text: &str, pc: u32, variant: Mos6502Variant) -> Vec<u8> {
    encode(text, &HashMap::new(), pc, false, variant).unwrap()
}

fn err(text: &str, variant: Mos6502Variant) -> String {
    encode(text, &HashMap::new(), 0, false, variant)
        .unwrap_err()
        .to_string()
}

// ── NMOS 6502 (baseline) ──────────────────────────────────────────────

#[test]
fn nmos_standard_opcodes() {
    assert_eq!(enc("lda #$10", Mos6502Variant::Nmos6502), vec![0xA9, 0x10]);
    assert_eq!(enc("lda $20", Mos6502Variant::Nmos6502), vec![0xA5, 0x20]);
    assert_eq!(
        enc("lda $2000", Mos6502Variant::Nmos6502),
        vec![0xAD, 0x00, 0x20]
    );
    assert_eq!(enc("lda $20,x", Mos6502Variant::Nmos6502), vec![0xB5, 0x20]);
    assert_eq!(
        enc("lda $2000,x", Mos6502Variant::Nmos6502),
        vec![0xBD, 0x00, 0x20]
    );
    assert_eq!(
        enc("lda $2000,y", Mos6502Variant::Nmos6502),
        vec![0xB9, 0x00, 0x20]
    );
    assert_eq!(
        enc("lda ($20,x)", Mos6502Variant::Nmos6502),
        vec![0xA1, 0x20]
    );
    assert_eq!(
        enc("lda ($20),y", Mos6502Variant::Nmos6502),
        vec![0xB1, 0x20]
    );
    assert_eq!(
        enc("sta $1234", Mos6502Variant::Nmos6502),
        vec![0x8D, 0x34, 0x12]
    );
    assert_eq!(
        enc("jmp $3456", Mos6502Variant::Nmos6502),
        vec![0x4C, 0x56, 0x34]
    );
    assert_eq!(
        enc("jsr $5678", Mos6502Variant::Nmos6502),
        vec![0x20, 0x78, 0x56]
    );
    assert_eq!(enc("beq $10", Mos6502Variant::Nmos6502), vec![0xF0, 0x0E]);
    assert_eq!(enc("nop", Mos6502Variant::Nmos6502), vec![0xEA]);
    assert_eq!(enc("sbc #$05", Mos6502Variant::Nmos6502), vec![0xE9, 0x05]);
}

#[test]
fn nmos_accumulator_and_implied() {
    assert_eq!(enc("asl a", Mos6502Variant::Nmos6502), vec![0x0A]);
    assert_eq!(enc("rol a", Mos6502Variant::Nmos6502), vec![0x2A]);
    assert_eq!(enc("lsr a", Mos6502Variant::Nmos6502), vec![0x4A]);
    assert_eq!(enc("ror a", Mos6502Variant::Nmos6502), vec![0x6A]);
    assert_eq!(enc("tax", Mos6502Variant::Nmos6502), vec![0xAA]);
    assert_eq!(enc("txa", Mos6502Variant::Nmos6502), vec![0x8A]);
    assert_eq!(enc("clc", Mos6502Variant::Nmos6502), vec![0x18]);
    assert_eq!(enc("sec", Mos6502Variant::Nmos6502), vec![0x38]);
}

#[test]
fn nmos_relative_branch() {
    // bcs $10 at PC=0 → offset = $10 - 2 = $0E
    assert_eq!(
        enc_at("bcs $10", 0, Mos6502Variant::Nmos6502),
        vec![0xB0, 0x0E]
    );
    // bne label at PC=0x100, target = 0x0F8 → offset = 0x0F8 - (0x100 + 2) = -10 = 0xF6
    let mut labels = HashMap::new();
    labels.insert("label".to_string(), 0x0F8);
    assert_eq!(
        encode("bne label", &labels, 0x100, true, Mos6502Variant::Nmos6502).unwrap(),
        vec![0xD0, 0xF6]
    );
}

// ── 65C02 opcodes ─────────────────────────────────────────────────────

#[test]
fn cmos_bra() {
    assert_eq!(
        enc_at("bra $10", 0, Mos6502Variant::Cmos65C02),
        vec![0x80, 0x0E]
    );
    assert_eq!(
        enc_at("bra $05", 2, Mos6502Variant::Cmos65C02),
        vec![0x80, 0x01]
    );
    assert_eq!(
        enc_at("bra $02", 0, Mos6502Variant::Cmos65C02),
        vec![0x80, 0x00]
    );
}

#[test]
fn cmos_stack_ops() {
    assert_eq!(enc("phx", Mos6502Variant::Cmos65C02), vec![0xDA]);
    assert_eq!(enc("phy", Mos6502Variant::Cmos65C02), vec![0x5A]);
    assert_eq!(enc("plx", Mos6502Variant::Cmos65C02), vec![0xFA]);
    assert_eq!(enc("ply", Mos6502Variant::Cmos65C02), vec![0x7A]);
}

#[test]
fn cmos_stp_wai() {
    assert_eq!(enc("stp", Mos6502Variant::Cmos65C02), vec![0xDB]);
    assert_eq!(enc("wai", Mos6502Variant::Cmos65C02), vec![0xCB]);
}

#[test]
fn cmos_inc_dec_accumulator() {
    assert_eq!(enc("inc a", Mos6502Variant::Cmos65C02), vec![0x1A]);
    assert_eq!(enc("dec a", Mos6502Variant::Cmos65C02), vec![0x3A]);
}

#[test]
fn cmos_bit_immediate() {
    assert_eq!(enc("bit #$80", Mos6502Variant::Cmos65C02), vec![0x89, 0x80]);
    assert_eq!(enc("bit #$00", Mos6502Variant::Cmos65C02), vec![0x89, 0x00]);
}

#[test]
fn cmos_stz() {
    assert_eq!(enc("stz $10", Mos6502Variant::Cmos65C02), vec![0x64, 0x10]);
    assert_eq!(
        enc("stz $1234", Mos6502Variant::Cmos65C02),
        vec![0x9C, 0x34, 0x12]
    );
    assert_eq!(
        enc("stz $10,x", Mos6502Variant::Cmos65C02),
        vec![0x74, 0x10]
    );
    assert_eq!(
        enc("stz $1234,x", Mos6502Variant::Cmos65C02),
        vec![0x9E, 0x34, 0x12]
    );
}

#[test]
fn cmos_variant_rejects_on_nmos() {
    assert!(err("bra $10", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("phx", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("phy", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("plx", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("ply", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("stp", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("wai", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("inc a", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("dec a", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("bit #$80", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("stz $10", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("trb $20", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("tsb $30", Mos6502Variant::Nmos6502).contains("assembler does not support"));
}

// ── 65C02 JMP (abs,X) ────────────────────────────────────────────────

#[test]
fn cmos_jmp_indexed_indirect_x() {
    assert_eq!(
        enc("jmp ($1234,x)", Mos6502Variant::Cmos65C02),
        vec![0x7C, 0x34, 0x12]
    );
}

#[test]
fn cmos_jmp_indexed_indirect_x_rejected_on_nmos() {
    assert!(err("jmp ($1234,x)", Mos6502Variant::Nmos6502).contains("assembler does not support"));
}

// ── 65C02 (zp) indirect ───────────────────────────────────────────────

#[test]
fn cmos_zp_indirect_adc() {
    assert_eq!(
        enc("adc ($12)", Mos6502Variant::Cmos65C02),
        vec![0x72, 0x12]
    );
}

#[test]
fn cmos_zp_indirect_and_cmp_ora() {
    assert_eq!(
        enc("and ($34)", Mos6502Variant::Cmos65C02),
        vec![0x32, 0x34]
    );
    assert_eq!(
        enc("cmp ($56)", Mos6502Variant::Cmos65C02),
        vec![0xD2, 0x56]
    );
    assert_eq!(
        enc("ora ($78)", Mos6502Variant::Cmos65C02),
        vec![0x12, 0x78]
    );
}

#[test]
fn cmos_zp_indirect_eor_lda_sta_sbc() {
    assert_eq!(
        enc("eor ($10)", Mos6502Variant::Cmos65C02),
        vec![0x52, 0x10]
    );
    assert_eq!(
        enc("lda ($20)", Mos6502Variant::Cmos65C02),
        vec![0xB2, 0x20]
    );
    assert_eq!(
        enc("sta ($30)", Mos6502Variant::Cmos65C02),
        vec![0x92, 0x30]
    );
    assert_eq!(
        enc("sbc ($40)", Mos6502Variant::Cmos65C02),
        vec![0xF2, 0x40]
    );
}

#[test]
fn cmos_zp_indirect_works_on_65c816_too() {
    assert_eq!(
        enc("adc ($12)", Mos6502Variant::Wdc65C816),
        vec![0x72, 0x12]
    );
    assert_eq!(
        enc("lda ($20)", Mos6502Variant::Wdc65C816),
        vec![0xB2, 0x20]
    );
}

#[test]
fn cmos_zp_indirect_rejected_on_nmos() {
    assert!(err("adc ($12)", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("sta ($30)", Mos6502Variant::Nmos6502).contains("assembler does not support"));
}

// ── 65C02 RMB / SMB ─────────────────────────────────────────────────

#[test]
fn cmos_rmb_smb() {
    assert_eq!(enc("rmb0 $12", Mos6502Variant::Cmos65C02), vec![0x07, 0x12]);
    assert_eq!(enc("rmb3 $34", Mos6502Variant::Cmos65C02), vec![0x37, 0x34]);
    assert_eq!(enc("rmb7 $56", Mos6502Variant::Cmos65C02), vec![0x77, 0x56]);
    assert_eq!(enc("smb0 $78", Mos6502Variant::Cmos65C02), vec![0x87, 0x78]);
    assert_eq!(enc("smb4 $9A", Mos6502Variant::Cmos65C02), vec![0xC7, 0x9A]);
    assert_eq!(enc("smb7 $BC", Mos6502Variant::Cmos65C02), vec![0xF7, 0xBC]);
}

#[test]
fn cmos_rmb_smb_rejected_on_nmos() {
    assert!(err("rmb0 $12", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("smb0 $12", Mos6502Variant::Nmos6502).contains("assembler does not support"));
}

// ── 65C02 BBR / BBS ─────────────────────────────────────────────────

#[test]
fn cmos_bbr_bbs() {
    // relative offset = target - (pc + 3) for 3-byte instruction
    assert_eq!(
        enc_at("bbr0 $12,$20", 0, Mos6502Variant::Cmos65C02),
        vec![0x0F, 0x12, 0x1D]
    );
    assert_eq!(
        enc_at("bbr4 $34,$50", 0, Mos6502Variant::Cmos65C02),
        vec![0x4F, 0x34, 0x4D]
    );
    assert_eq!(
        enc_at("bbs0 $56,$80", 0, Mos6502Variant::Cmos65C02),
        vec![0x8F, 0x56, 0x7D]
    );
    assert_eq!(
        enc_at("bbs7 $78,$60", 0, Mos6502Variant::Cmos65C02),
        vec![0xFF, 0x78, 0x5D]
    );
}

#[test]
fn cmos_bbr_bbs_rejected_on_nmos() {
    assert!(err("bbr0 $12", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("bbs0 $12", Mos6502Variant::Nmos6502).contains("assembler does not support"));
}

// ── 65C816 opcodes ────────────────────────────────────────────────────

#[test]
fn wdc816_exchange() {
    assert_eq!(enc("xba", Mos6502Variant::Wdc65C816), vec![0xEB]);
    assert_eq!(enc("xce", Mos6502Variant::Wdc65C816), vec![0xFB]);
}

#[test]
fn wdc816_rep_sep() {
    assert_eq!(enc("rep #$10", Mos6502Variant::Wdc65C816), vec![0xC2, 0x10]);
    assert_eq!(enc("sep #$80", Mos6502Variant::Wdc65C816), vec![0xE2, 0x80]);
}

#[test]
fn wdc816_block_move() {
    // MVP/MVN take dst_bank,src_bank as two-byte operand (LE)
    // mvp $00,$01 → src=$00, dst=$01 → packed 0x0001 → LE [0x01,0x00]
    assert_eq!(
        enc("mvp $00,$01", Mos6502Variant::Wdc65C816),
        vec![0x44, 0x01, 0x00]
    );
    assert_eq!(
        enc("mvn $FF,$80", Mos6502Variant::Wdc65C816),
        vec![0x54, 0x80, 0xFF]
    );
}

#[test]
fn wdc816_push_effective() {
    assert_eq!(
        enc("pea $1234", Mos6502Variant::Wdc65C816),
        vec![0xF4, 0x34, 0x12]
    );
    assert_eq!(
        enc("pei ($10)", Mos6502Variant::Wdc65C816),
        vec![0xD4, 0x10]
    );
}

#[test]
fn wdc816_per() {
    // per $10 at PC=0 → offset = $10 - (0 + 3) = $0D (16-bit)
    assert_eq!(
        enc_at("per $10", 0, Mos6502Variant::Wdc65C816),
        vec![0x62, 0x0D, 0x00]
    );
    assert_eq!(
        enc_at("per $05", 0, Mos6502Variant::Wdc65C816),
        vec![0x62, 0x02, 0x00]
    );
}

#[test]
fn wdc816_indexed_indirect_x() {
    assert_eq!(
        enc("jmp ($1234,x)", Mos6502Variant::Wdc65C816),
        vec![0x7C, 0x34, 0x12]
    );
    assert_eq!(
        enc("jsr ($5678,x)", Mos6502Variant::Wdc65C816),
        vec![0xFC, 0x78, 0x56]
    );
}

#[test]
fn wdc816_absolute_long_jump() {
    assert_eq!(
        enc("jmp !$123456", Mos6502Variant::Wdc65C816),
        vec![0x5C, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        enc("jsr !$654321", Mos6502Variant::Wdc65C816),
        vec![0x22, 0x21, 0x43, 0x65]
    );
}

#[test]
fn wdc816_indirect_long_jmp() {
    assert_eq!(
        enc("jmp [$1234]", Mos6502Variant::Wdc65C816),
        vec![0xDC, 0x34, 0x12]
    );
}

#[test]
fn wdc816_absolute_long_alu() {
    assert_eq!(
        enc("lda !$123456", Mos6502Variant::Wdc65C816),
        vec![0xAF, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        enc("sta !$123456", Mos6502Variant::Wdc65C816),
        vec![0x8F, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        enc("adc !$123456", Mos6502Variant::Wdc65C816),
        vec![0x6F, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        enc("sbc !$123456", Mos6502Variant::Wdc65C816),
        vec![0xEF, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        enc("and !$123456", Mos6502Variant::Wdc65C816),
        vec![0x2F, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        enc("ora !$123456", Mos6502Variant::Wdc65C816),
        vec![0x0F, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        enc("eor !$123456", Mos6502Variant::Wdc65C816),
        vec![0x4F, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        enc("cmp !$123456", Mos6502Variant::Wdc65C816),
        vec![0xCF, 0x56, 0x34, 0x12]
    );
}

#[test]
fn wdc816_auto_long_for_sufficiently_large_address() {
    // Without `!`, values > 0xFFFF should use AbsoluteLong
    assert_eq!(
        enc("lda $123456", Mos6502Variant::Wdc65C816),
        vec![0xAF, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        enc("sta $123456", Mos6502Variant::Wdc65C816),
        vec![0x8F, 0x56, 0x34, 0x12]
    );
    // Values ≤ 0xFFFF still use normal absolute
    assert_eq!(
        enc("lda $1234", Mos6502Variant::Wdc65C816),
        vec![0xAD, 0x34, 0x12]
    );
}

#[test]
fn wdc816_wdm() {
    assert_eq!(enc("wdm #$42", Mos6502Variant::Wdc65C816), vec![0x42, 0x42]);
    assert_eq!(enc("wdm #$00", Mos6502Variant::Wdc65C816), vec![0x42, 0x00]);
}

#[test]
fn wdc816_cmos_opcodes_also_work() {
    // 65C816 is a superset of 65C02
    assert_eq!(enc("bra $10", Mos6502Variant::Wdc65C816), vec![0x80, 0x0E]);
    assert_eq!(enc("phx", Mos6502Variant::Wdc65C816), vec![0xDA]);
    assert_eq!(enc("phy", Mos6502Variant::Wdc65C816), vec![0x5A]);
    assert_eq!(enc("plx", Mos6502Variant::Wdc65C816), vec![0xFA]);
    assert_eq!(enc("ply", Mos6502Variant::Wdc65C816), vec![0x7A]);
    assert_eq!(enc("stp", Mos6502Variant::Wdc65C816), vec![0xDB]);
    assert_eq!(enc("wai", Mos6502Variant::Wdc65C816), vec![0xCB]);
    assert_eq!(enc("inc a", Mos6502Variant::Wdc65C816), vec![0x1A]);
    assert_eq!(enc("dec a", Mos6502Variant::Wdc65C816), vec![0x3A]);
    assert_eq!(enc("bit #$80", Mos6502Variant::Wdc65C816), vec![0x89, 0x80]);
    assert_eq!(enc("stz $10", Mos6502Variant::Wdc65C816), vec![0x64, 0x10]);
    assert_eq!(enc("trb $20", Mos6502Variant::Wdc65C816), vec![0x14, 0x20]);
    assert_eq!(enc("tsb $30", Mos6502Variant::Wdc65C816), vec![0x04, 0x30]);
}

#[test]
fn wdc816_variant_rejects_on_nmos() {
    assert!(err("xba", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("xce", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("rep #$01", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("sep #$01", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("pei ($10)", Mos6502Variant::Nmos6502).contains("assembler does not support"));
}

// ── 65C816 COP, BRL, RTL ──────────────────────────────────────────────

#[test]
fn wdc816_cop() {
    assert_eq!(enc("cop #$42", Mos6502Variant::Wdc65C816), vec![0x02, 0x42]);
    assert_eq!(enc("cop #$00", Mos6502Variant::Wdc65C816), vec![0x02, 0x00]);
}

#[test]
fn wdc816_brl() {
    // brl $10 at PC=0 → offset = $10 - (0 + 3) = $0D (16-bit)
    assert_eq!(
        enc_at("brl $10", 0, Mos6502Variant::Wdc65C816),
        vec![0x82, 0x0D, 0x00]
    );
    assert_eq!(
        enc_at("brl $0120", 0, Mos6502Variant::Wdc65C816),
        vec![0x82, 0x1D, 0x01]
    );
}

#[test]
fn wdc816_rtl() {
    assert_eq!(enc("rtl", Mos6502Variant::Wdc65C816), vec![0x6B]);
}

// ── 65C816 bank/DP register push/pull ─────────────────────────────────

#[test]
fn wdc816_phb_phd_phk() {
    assert_eq!(enc("phb", Mos6502Variant::Wdc65C816), vec![0x8B]);
    assert_eq!(enc("phd", Mos6502Variant::Wdc65C816), vec![0x0B]);
    assert_eq!(enc("phk", Mos6502Variant::Wdc65C816), vec![0x4B]);
}

#[test]
fn wdc816_plb_pld() {
    assert_eq!(enc("plb", Mos6502Variant::Wdc65C816), vec![0xAB]);
    assert_eq!(enc("pld", Mos6502Variant::Wdc65C816), vec![0x2B]);
}

// ── 65C816 transfer instructions ──────────────────────────────────────

#[test]
fn wdc816_transfer_tcs_tsc_tcd_tdc() {
    assert_eq!(enc("tcs", Mos6502Variant::Wdc65C816), vec![0x1B]);
    assert_eq!(enc("tsc", Mos6502Variant::Wdc65C816), vec![0x3B]);
    assert_eq!(enc("tcd", Mos6502Variant::Wdc65C816), vec![0x5B]);
    assert_eq!(enc("tdc", Mos6502Variant::Wdc65C816), vec![0x7B]);
}

#[test]
fn wdc816_transfer_txy_tyx() {
    assert_eq!(enc("txy", Mos6502Variant::Wdc65C816), vec![0x9B]);
    assert_eq!(enc("tyx", Mos6502Variant::Wdc65C816), vec![0xBB]);
}

#[test]
fn wdc816_new_implied_rejected_on_nmos() {
    assert!(err("rtl", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("phb", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("phd", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("phk", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("plb", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("pld", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("tcs", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("tsc", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("tcd", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("tdc", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("txy", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("tyx", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("cop #$01", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    assert!(err("brl $10", Mos6502Variant::Nmos6502).contains("assembler does not support"));
}

// ── Ricoh 2A03 ────────────────────────────────────────────────────────

#[test]
fn ricoh_sbc_immediate_uses_eb() {
    // NMOS uses $E9, Ricoh 2A03 uses $EB
    assert_eq!(enc("sbc #$05", Mos6502Variant::Ricoh2A03), vec![0xEB, 0x05]);
    assert_eq!(enc("sbc #$FF", Mos6502Variant::Ricoh2A03), vec![0xEB, 0xFF]);
}

#[test]
fn ricoh_standard_nmos_opcodes_still_work() {
    assert_eq!(enc("lda #$10", Mos6502Variant::Ricoh2A03), vec![0xA9, 0x10]);
    assert_eq!(
        enc("sta $2000", Mos6502Variant::Ricoh2A03),
        vec![0x8D, 0x00, 0x20]
    );
    assert_eq!(
        enc("jsr $2000", Mos6502Variant::Ricoh2A03),
        vec![0x20, 0x00, 0x20]
    );
    assert_eq!(enc("nop", Mos6502Variant::Ricoh2A03), vec![0xEA]);
    assert_eq!(enc("tax", Mos6502Variant::Ricoh2A03), vec![0xAA]);
}

#[test]
fn ricoh_rejects_65c02_opcodes() {
    assert!(err("bra $10", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
    assert!(err("phx", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
    assert!(err("stz $10", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
    assert!(err("inc a", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
}

#[test]
fn ricoh_rejects_65c816_opcodes() {
    assert!(err("xba", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
    assert!(err("xce", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
    assert!(err("rep #$01", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
    assert!(err("sep #$01", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
    assert!(err("pei ($10)", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
}

// ── instruction_len_for_variant ───────────────────────────────────────

#[test]
fn test_instruction_len_for_variant() {
    assert_eq!(
        instruction_len_for_variant("nop", Mos6502Variant::Nmos6502).unwrap(),
        1
    );
    assert_eq!(
        instruction_len_for_variant("nop", Mos6502Variant::Cmos65C02).unwrap(),
        1
    );
    assert_eq!(
        instruction_len_for_variant("nop", Mos6502Variant::Wdc65C816).unwrap(),
        1
    );
    assert_eq!(
        instruction_len_for_variant("nop", Mos6502Variant::Ricoh2A03).unwrap(),
        1
    );

    assert_eq!(
        instruction_len_for_variant("bra $10", Mos6502Variant::Cmos65C02).unwrap(),
        2
    );
    assert_eq!(
        instruction_len_for_variant("bra $10", Mos6502Variant::Wdc65C816).unwrap(),
        2
    );

    assert_eq!(
        instruction_len_for_variant("phx", Mos6502Variant::Cmos65C02).unwrap(),
        1
    );
    assert_eq!(
        instruction_len_for_variant("phx", Mos6502Variant::Wdc65C816).unwrap(),
        1
    );

    assert_eq!(
        instruction_len_for_variant("lda !$123456", Mos6502Variant::Wdc65C816).unwrap(),
        4
    );
    assert_eq!(
        instruction_len_for_variant("jmp !$123456", Mos6502Variant::Wdc65C816).unwrap(),
        4
    );
    assert_eq!(
        instruction_len_for_variant("per $10", Mos6502Variant::Wdc65C816).unwrap(),
        3
    );
    assert_eq!(
        instruction_len_for_variant("pea $1234", Mos6502Variant::Wdc65C816).unwrap(),
        3
    );

    assert_eq!(
        instruction_len_for_variant("sbc #$05", Mos6502Variant::Ricoh2A03).unwrap(),
        2
    );
}

// ── encode_instruction_for_variant ────────────────────────────────────

#[test]
fn test_encode_instruction_for_variant_with_labels() {
    let mut labels = HashMap::new();
    labels.insert("target".to_string(), 0x2000);
    let bytes = encode_instruction_for_variant(
        "jmp target",
        &labels,
        0x1000,
        true,
        Mos6502Variant::Nmos6502,
    )
    .unwrap();
    assert_eq!(bytes, vec![0x4C, 0x00, 0x20]);

    let bytes = encode_instruction_for_variant(
        "beq target",
        &labels,
        0x1FF0,
        true,
        Mos6502Variant::Nmos6502,
    )
    .unwrap();
    // offset = 0x2000 - (0x1FF0 + 2) = 0x2000 - 0x1FF2 = 0x0E
    assert_eq!(bytes, vec![0xF0, 0x0E]);
}

#[test]
fn test_encode_instruction_for_variant_defaults_to_nmos() {
    let bytes = encode_instruction("lda #$10", &HashMap::new(), 0, false).unwrap();
    assert_eq!(bytes, vec![0xA9, 0x10]);
}

// ── Variant-as-str ────────────────────────────────────────────────────

#[test]
fn variant_as_str() {
    assert_eq!(Mos6502Variant::Nmos6502.as_str(), "6502");
    assert_eq!(Mos6502Variant::Cmos65C02.as_str(), "65C02");
    assert_eq!(Mos6502Variant::Wdc65C816.as_str(), "65C816");
    assert_eq!(Mos6502Variant::Ricoh2A03.as_str(), "2A03");
}
