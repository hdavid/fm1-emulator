// SPDX-License-Identifier: GPL-3.0-only
// elf_symbols: names, addresses, sizes and kinds from an ELF32 symbol table
// (the profile of play_check attributes PCs to STT_FUNC symbols by size).
use fm1_emu::firmware::elf_symbols;

/// A minimal ELF32: header, then .strtab, .symtab, and three section headers
/// (null, symtab linked to strtab, strtab).
fn elf(symbols: &[(&str, u32, u32, u8, u16)]) -> Vec<u8> {
    let mut strtab = vec![0u8];
    let mut symtab = vec![0u8; 16];
    for &(name, value, size, info, shndx) in symbols {
        let offset = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        symtab.extend_from_slice(&offset.to_le_bytes());
        symtab.extend_from_slice(&value.to_le_bytes());
        symtab.extend_from_slice(&size.to_le_bytes());
        symtab.extend_from_slice(&[info, 0]);
        symtab.extend_from_slice(&shndx.to_le_bytes());
    }
    let str_off = 52u32;
    let sym_off = str_off + strtab.len() as u32;
    let sh_off = sym_off + symtab.len() as u32;
    let mut out = vec![0u8; 52];
    out[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
    out[32..36].copy_from_slice(&sh_off.to_le_bytes());
    out[46..48].copy_from_slice(&40u16.to_le_bytes());
    out[48..50].copy_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&strtab);
    out.extend_from_slice(&symtab);
    let section = |kind: u32, offset: u32, size: u32, link: u32, entsize: u32| {
        let mut sh = vec![0u8; 40];
        for (at, v) in [
            (4, kind),
            (16, offset),
            (20, size),
            (24, link),
            (36, entsize),
        ] {
            sh[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        sh
    };
    out.extend(vec![0u8; 40]);
    out.extend(section(2, sym_off, symtab.len() as u32, 2, 16));
    out.extend(section(3, str_off, strtab.len() as u32, 0, 0));
    out
}

#[test]
fn reads_functions_with_sizes_and_skips_undefined_symbols() {
    let data = elf(&[
        ("dx7_op", 0x0202_06ba, 0xb0, 0x02, 1),
        ("dx7_sintab", 0x01c0_e7d0, 4100, 0x01, 3),
        ("undefined", 0, 0, 0x12, 0),
    ]);
    let symbols = elf_symbols(&data).unwrap();
    assert_eq!(symbols.len(), 2);
    assert_eq!(
        (
            symbols[0].name.as_str(),
            symbols[0].address,
            symbols[0].size,
            symbols[0].is_function()
        ),
        ("dx7_op", 0x0202_06ba, 0xb0, true)
    );
    assert!(!symbols[1].is_function());
    assert!(elf_symbols(b"not an elf at all, but long enough to hold a header......").is_err());
}
