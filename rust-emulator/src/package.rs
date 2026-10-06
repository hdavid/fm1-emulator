// SPDX-License-Identifier: GPL-3.0-only
// Read-only FWSC/JLFS decoding. Cipher and CRC mirror tools/fm1pkg_make.py.
use crate::bus::Bus;

pub(crate) struct Package {
    pub flash: Vec<u8>,
    pub key: u16,
    pub header: Vec<u8>,
}

fn slice(data: &[u8], offset: usize, size: usize) -> Result<&[u8], String> {
    data.get(offset..offset.checked_add(size).ok_or("package range overflow")?)
        .ok_or_else(|| "truncated package range".into())
}
fn u16_at(data: &[u8], offset: usize) -> Result<u16, String> {
    Ok(u16::from_le_bytes(
        slice(data, offset, 2)?.try_into().unwrap(),
    ))
}
fn u32_at(data: &[u8], offset: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        slice(data, offset, 4)?.try_into().unwrap(),
    ))
}
pub(crate) fn crc(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |mut value, &byte| {
        value ^= (byte as u16) << 8;
        for _ in 0..8 {
            value = (value << 1) ^ if value & 0x8000 != 0 { 0x1021 } else { 0 };
        }
        value
    })
}
fn check(data: &[u8], expected: u16, name: &str) -> Result<(), String> {
    if crc(data) != expected {
        return Err(format!("{name} checksum mismatch"));
    }
    Ok(())
}
pub(crate) fn enc(data: &mut [u8], mut key: u16) {
    for byte in data {
        *byte ^= key as u8;
        key = (key << 1) ^ if key & 0x8000 != 0 { 0x1021 } else { 0 };
    }
}
pub(crate) fn sfc(data: &mut [u8], key: u16) {
    for (index, block) in data.chunks_mut(32).enumerate() {
        enc(block, key ^ (index * 8) as u16);
    }
}

fn resource(raw: &[u8], key: u16, offset: usize, expected: u16) -> Result<Vec<u8>, String> {
    if !offset.is_multiple_of(32) {
        return Err("unsupported resource cipher alignment".into());
    }
    let mut data = raw.to_vec();
    // UFW auxiliary files use the same 32-byte SFC cipher, keyed by their
    // absolute position in the logical package, not their target flash address.
    // FM-1_015 USR at 0x93400 verifies CRC e3bc and contains factory voices.
    for (index, block) in data.chunks_mut(32).enumerate() {
        enc(block, key ^ ((offset / 4 + index * 8) as u16));
    }
    check(&data, expected, "USR")?;
    Ok(data)
}

/// Whether a decrypted voice bank looks like DX7 packed voices: the first
/// 128-byte voice's name (bytes 118-127) is printable ASCII, as FM-1_015's
/// "PIANO 1   " is. Guards the relocated search below against a chance
/// 16-bit CRC match.
fn plausible_voices(data: &[u8]) -> bool {
    data.get(118..128)
        .is_none_or(|name| name.iter().all(|byte| (0x20..0x7f).contains(byte)))
}

/// A USR payload whose ciphertext was made for another package position:
/// Baud Girl FM-1_093 stores FM-1_015's USR bytes unchanged at 0xae400 (CRC
/// e3bc verifies only as if at 0x93400). Both packages place resources on
/// 1 KB boundaries, and the cipher position repeats every 256 KB (it enters
/// as position / 4, 16 bits), so 256 positions cover every candidate. Only a
/// payload whose plaintext CRC verifies and whose first voice is plausible
/// is returned, with the position it verified at.
fn relocated_resource(raw: &[u8], key: u16, expected: u16) -> Option<(usize, Vec<u8>)> {
    (0..0x40000).step_by(0x400).find_map(|offset| {
        resource(raw, key, offset, expected)
            .ok()
            .filter(|data| plausible_voices(data))
            .map(|data| (offset, data))
    })
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    // Synthetic data encoded independently with tools/fm1pkg_make.py;
    // spans two complete cipher blocks and a one-byte final block.
    const RAW: [u8; 65] = [
        0x49, 0x72, 0x6e, 0x9e, 0x2e, 0xe2, 0x58, 0x18, 0xb0, 0xf4, 0x56, 0x2d, 0xca, 0x53, 0xb7,
        0xfc, 0x58, 0x09, 0x91, 0x9c, 0xd1, 0xa1, 0xff, 0x59, 0x3f, 0xe6, 0x11, 0x74, 0xe7, 0x7d,
        0x71, 0x0b, 0x63, 0x4a, 0x09, 0x97, 0xbe, 0xfe, 0x16, 0x0a, 0xb1, 0xe3, 0x4f, 0x33, 0xc8,
        0x20, 0xe0, 0x24, 0x35, 0x4b, 0x96, 0x2c, 0x79, 0xf2, 0xe4, 0xe9, 0xf3, 0xe6, 0xcc, 0x98,
        0x11, 0x22, 0x65, 0xeb, 0x3e,
    ];

    #[test]
    fn auxiliary_cipher_uses_package_position_and_checks_plaintext_crc() {
        let data = resource(&RAW, 0x980f, 0x93400, 0x05e6).unwrap();
        assert_eq!(
            data,
            b"FM1 synthetic preset bytes; not device firmware.\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0!"
        );
        assert_eq!(RAW[0], 0x49);
        // Flash target and a relocated container offset give different keys.
        for offset in [0xea000, 0xae400] {
            assert!(resource(&RAW, 0x980f, offset, 0x05e6).is_err());
        }
        assert!(resource(&RAW, 0x980f, 0x93401, 0x05e6).is_err());
        let mut corrupt = RAW;
        corrupt[64] ^= 1;
        assert!(resource(&corrupt, 0x980f, 0x93400, 0x05e6).is_err());
        assert!(resource(&RAW[..64], 0x980f, 0x93400, 0x05e6).is_err());
    }

    /// A package with a USR resource. `copied`: the payload keeps RAW's
    /// ciphertext for package position 0x93400 wherever it is stored, as
    /// Baud Girl FM-1_093 keeps FM-1_015's.
    fn with_usr(target: u32, capacity: u32, corrupt: bool, copied: bool) -> Vec<u8> {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../build/display/firmware.fwsc");
        let raw = std::fs::read(path).unwrap();
        let mut logical = Vec::new();
        for block in raw[..960].chunks_exact(48) {
            logical.extend_from_slice(&block[..47]);
        }
        logical.extend_from_slice(&raw[960..]);
        let mut header = logical[..64].to_vec();
        enc(&mut header, 0xffff);
        // Make a full-size flash payload with a sentinel in key_mac. The
        // fixture only needs flash.bin and USR; omit the unused OTA entry.
        let mut flash_entry = logical[64..144].to_vec();
        enc(&mut flash_entry, 0xffff);
        assert_eq!(name(&flash_entry, 64), b"flash.bin");
        let start = u32_at(&flash_entry, 8).unwrap() as usize;
        let size = u32_at(&flash_entry, 12).unwrap() as usize;
        assert_eq!(start, 0x400);
        let mut flash = logical[start..start + size].to_vec();
        flash.resize(1024 * 1024, 255);
        flash[0xff000..0xff004].copy_from_slice(b"KEEP");
        flash_entry[4..6].copy_from_slice(&crc(&flash).to_le_bytes());
        for at in [12, 16] {
            flash_entry[at..at + 4].copy_from_slice(&(flash.len() as u32).to_le_bytes());
        }
        enc(&mut flash_entry, 0xffff);
        logical.truncate(start);
        logical[64..144].copy_from_slice(&flash_entry);
        logical.extend_from_slice(&flash);
        let count = 1;
        assert!(64 + (count + 1) * 80 <= 0x400);
        let offset = logical.len().next_multiple_of(32);
        let mut payload = resource(&RAW, 0x980f, 0x93400, 0x05e6).unwrap();
        for (i, block) in payload.chunks_mut(32).enumerate() {
            enc(block, 0x980f ^ (offset / 4 + i * 8) as u16);
        }
        if copied {
            payload = RAW.to_vec();
        }
        if corrupt {
            payload[0] ^= 1;
        }
        logical.resize(offset, 255);
        logical.extend_from_slice(&payload);
        let mut entry = vec![0; 80];
        for (at, value) in [
            (8, offset as u32),
            (12, RAW.len() as u32),
            (16, 96),
            (28, target),
            (32, capacity),
        ] {
            entry[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        entry[..2].copy_from_slice(&0x32u16.to_le_bytes());
        entry[4..6].copy_from_slice(&0x05e6u16.to_le_bytes());
        entry[64..67].copy_from_slice(b"USR");
        enc(&mut entry, 0xffff);
        logical[64 + count * 80..64 + (count + 1) * 80].copy_from_slice(&entry);
        header[4..8].copy_from_slice(&(logical.len() as u32).to_le_bytes());
        header[8..10].copy_from_slice(&((count + 1) as u16).to_le_bytes());
        let checksum = crc(&logical[64..64 + (count + 1) * 80]);
        header[2..4].copy_from_slice(&checksum.to_le_bytes());
        let checksum = crc(&header[2..]);
        header[..2].copy_from_slice(&checksum.to_le_bytes());
        enc(&mut header, 0xffff);
        logical[..64].copy_from_slice(&header);
        let mut packed = Vec::new();
        for (i, block) in logical[..940].chunks_exact(47).enumerate() {
            packed.extend_from_slice(block);
            packed.push(raw[i * 48 + 47]);
        }
        packed.extend_from_slice(&logical[940..]);
        packed
    }

    #[test]
    fn preset_loading_preserves_the_application_and_flash_key_region() {
        let raw = with_usr(0xea000, 0x12000, false, false);
        let (package, image) = Package::decode(&raw).unwrap();
        let bad = with_usr(0xea000, 0x12000, true, false);
        let (without_usr, unchanged) = Package::decode(&bad).unwrap();
        assert_eq!(image, unchanged);
        assert_eq!(
            &package.flash[0xea000..0xea041],
            resource(&RAW, 0x980f, 0x93400, 0x05e6).unwrap()
        );
        assert_eq!(&without_usr.flash[0xea000..0xea041], &[255; 65]);
        assert_eq!(package.flash.len(), without_usr.flash.len());
        assert_eq!(&package.flash[0xff000..], &without_usr.flash[0xff000..]);
        assert_eq!(&package.flash[..0xea000], &without_usr.flash[..0xea000]);
    }

    #[test]
    fn a_usr_copied_from_another_package_position_loads_where_it_verifies() {
        // Baud Girl FM-1_093 stores FM-1_015's USR ciphertext unchanged at
        // 0xae400; it verifies only as if at 0x93400.
        let (package, _) = Package::decode(&with_usr(0xea000, 0x12000, false, true)).unwrap();
        assert_eq!(
            &package.flash[0xea000..0xea041],
            resource(&RAW, 0x980f, 0x93400, 0x05e6).unwrap()
        );
        // Nothing verifies a corrupted copy.
        let mut raw = with_usr(0xea000, 0x12000, false, true);
        let at = raw.len() - 10;
        raw[at] ^= 1;
        let (package, _) = Package::decode(&raw).unwrap();
        assert_eq!(&package.flash[0xea000..0xea041], &[255; 65]);
    }

    #[test]
    fn a_relocated_voice_bank_needs_a_printable_first_voice_name() {
        let mut bank = vec![0x63u8; 128];
        bank[118..128].copy_from_slice(b"PIANO 1   ");
        assert!(plausible_voices(&bank));
        bank[120] = 0xd7;
        assert!(!plausible_voices(&bank));
        // Shorter than one voice: nothing to check.
        assert!(plausible_voices(&[0xd7; 65]));
    }

    #[test]
    fn auxiliary_file_cannot_overwrite_code_or_exceed_its_reservation() {
        for (target, capacity) in [(0x4000, 0x12000), (0xea000, 64), (0xea000, u32::MAX)] {
            assert!(Package::decode(&with_usr(target, capacity, false, false)).is_err());
        }
    }
}
fn name(data: &[u8], start: usize) -> &[u8] {
    let bytes = &data[start..];
    &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())]
}
fn entry(data: &[u8]) -> Result<(usize, usize), String> {
    check(&data[2..], u16_at(data, 0)?, "flash directory entry")?;
    Ok((u32_at(data, 4)? as usize, u32_at(data, 8)? as usize))
}

impl Package {
    pub fn decode(raw: &[u8]) -> Result<(Self, Vec<u8>), String> {
        if raw.len() < 960 {
            return Err("FWSC package is too short".into());
        }
        let product: Vec<u8> = (0..20)
            .filter_map(|i| {
                let marker = raw[i * 48 + 47];
                (marker != 0x7d).then(|| marker.wrapping_sub(i as u8 + 1))
            })
            .collect();
        if !product.starts_with(b"FM-1_")
            || product.len() <= 5
            || !product[5..].iter().all(u8::is_ascii_digit)
        {
            return Err("package identity is not FM-1".into());
        }
        let mut logical = Vec::with_capacity(raw.len() - 20);
        for block in raw[..960].chunks_exact(48) {
            logical.extend_from_slice(&block[..47]);
        }
        logical.extend_from_slice(&raw[960..]);
        let mut head = slice(&logical, 0, 64)?.to_vec();
        enc(&mut head, 0xffff);
        check(&head[2..], u16_at(&head, 0)?, "package header")?;
        if name(&head, 16) != b"AC791N" {
            return Err("package is not for AC791N".into());
        }
        let count = u16_at(&head, 8)? as usize;
        let table = slice(&logical, 64, count * 80)?;
        check(table, u16_at(&head, 2)?, "package file table")?;
        let mut flash = None;
        let mut usr = None;
        for stored in table.chunks_exact(80) {
            let mut e = stored.to_vec();
            enc(&mut e, 0xffff);
            if name(&e, 64) == b"flash.bin" {
                if flash.is_some() {
                    return Err("duplicate flash.bin".into());
                }
                let payload = slice(&logical, u32_at(&e, 8)? as usize, u32_at(&e, 12)? as usize)?;
                check(payload, u16_at(&e, 4)?, "flash.bin")?;
                flash = Some(payload.to_vec());
            } else if name(&e, 64) == b"USR" {
                if usr.is_some() || u16_at(&e, 0)? != 0x32 {
                    return Err("unsupported or duplicate USR resource".into());
                }
                let offset = u32_at(&e, 8)? as usize;
                let size = u32_at(&e, 12)? as usize;
                let target = u32_at(&e, 28)? as usize;
                let capacity = u32_at(&e, 32)? as usize;
                if size > capacity || u32_at(&e, 16)? as usize > capacity {
                    return Err("USR payload exceeds its reserved area".into());
                }
                let payload = slice(&logical, offset, size)?;
                usr = Some((offset, u16_at(&e, 4)?, target, capacity, payload));
            }
        }
        let mut flash = flash.ok_or("package has no flash.bin")?;
        if flash.len() > 1024 * 1024 {
            return Err("flash.bin exceeds the FM-1 flash".into());
        }
        let mut header = slice(&flash, 0, 32)?.to_vec();
        enc(&mut header, 0xffff);
        check(&header[2..], u16_at(&header, 0)?, "flash header")?;
        let mut key = None;
        let mut base = None;
        for stored in slice(&flash, 32, 128)?.chunks_exact(32) {
            let mut e = stored.to_vec();
            enc(&mut e, 0xffff);
            let (offset, size) = entry(&e)?;
            match name(&e, 16) {
                b"isd_config.ini" => {
                    let config = slice(&flash, offset, size)?;
                    check(config, u16_at(&e, 2)?, "flash isd_config.ini")?;
                    let blob = slice(config, 0, 32)?;
                    check(blob, u16_at(config, 32)?, "chip key")?;
                    let sum = blob[..16].iter().fold(0u8, |a, b| a.wrapping_add(*b));
                    let threshold = if sum >= 0xe0 {
                        0xaa
                    } else if sum <= 0x10 {
                        0x55
                    } else {
                        sum
                    };
                    key = Some((0..16).fold(0, |value, i| {
                        value
                            | if blob[16 + i] ^ blob[15 - i] < threshold {
                                1 << i
                            } else {
                                0
                            }
                    }));
                }
                b"app_dir_head" => base = Some(offset),
                _ => {}
            }
        }
        let key = key.ok_or("missing chip key")?;
        if base != Some(0x4000) {
            return Err("unsupported SFC application base".into());
        }
        let mut area = slice(
            &flash,
            0x4000,
            flash
                .len()
                .checked_sub(0x4000)
                .ok_or("flash.bin is too short")?,
        )?
        .to_vec();
        sfc(&mut area, key);
        let area_entry = slice(&area, 0, 32)?;
        let (address, size) = entry(area_entry)?;
        if address != crate::XIP as usize || name(area_entry, 16) != b"app_area_head" {
            return Err("unsupported application execution address".into());
        }
        check(
            slice(
                &area,
                32,
                size.checked_sub(32).ok_or("invalid app area size")?,
            )?,
            u16_at(area_entry, 2)?,
            "application area",
        )?;
        let app_entry = slice(&area, 32, 32)?;
        let (offset, size) = entry(app_entry)?;
        if offset != 0x120 || name(app_entry, 16) != b"app.bin" {
            return Err("unsupported app.bin directory layout".into());
        }
        let image = slice(&area, offset, size)?.to_vec();
        check(&image, u16_at(app_entry, 2)?, "app.bin")?;
        if let Some((offset, expected, target, capacity, payload)) = usr {
            // Match the guest's reserved-region descriptor before installing
            // bytes. An auxiliary payload must never overwrite code or keys.
            let reserved = area[32..0x120].chunks_exact(32).any(|entry| {
                name(entry, 16) == b"USR"
                    && entry[12] == 0x92
                    && u32_at(entry, 4).ok() == Some(target as u32)
                    && u32_at(entry, 8).ok() == Some(capacity as u32)
            });
            let end = target
                .checked_add(capacity)
                .ok_or("USR flash range overflow")?;
            if !reserved || target < 0x4000 + u32_at(area_entry, 8)? as usize || end > 0xff000 {
                return Err("USR does not match a separate reserved flash area".into());
            }
            let data = match resource(payload, key, offset, expected) {
                Ok(data) => Some(data),
                Err(error) => match relocated_resource(payload, key, expected) {
                    // Modified update packages can retain an auxiliary file
                    // encrypted at its old package offset. A device updated
                    // from the official firmware holds that factory data
                    // (unverified on hardware): install it once its plaintext
                    // CRC verifies; never install unverified bytes.
                    Some((original, data)) => {
                        eprintln!(
                            "FWSC: {error} at package offset {offset:#x}; it verifies as if at {original:#x} (modulo 256 KB; copied from another package): installing that factory USR data"
                        );
                        Some(data)
                    }
                    None => {
                        eprintln!("FWSC: {error}; USR preset data was not loaded");
                        None
                    }
                },
            };
            if let Some(data) = data {
                flash.resize(flash.len().max(end), 255);
                flash[target..target + data.len()].copy_from_slice(&data);
            }
        }
        Ok((Self { flash, key, header }, image))
    }

    pub fn initialize(&self, bus: &mut Bus) -> Result<(), String> {
        bus.load_flash(&self.flash, self.key);
        // SPL BOOT_DEVICE_INFO (SDK include_lib/system/boot.h). Its fs_info
        // points to a decoded flash_head; the guest copies it into boot_info.
        const PARAM: u32 = 0x01c7fe08;
        const HEAD: u32 = 0x01c7fe40;
        for (i, &byte) in self.header.iter().enumerate() {
            bus.write(HEAD + i as u32, byte as u32, 1)
                .map_err(|e| e.to_string())?;
        }
        for (offset, value) in [
            (0, HEAD),
            (4, 0x4000),
            (8, 0x02000000),
            (12, self.key as u32),
        ] {
            bus.write(PARAM + offset, value, 4)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Package;
    #[test]
    fn malformed_packages_fail_without_panics() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../build/display/firmware.fwsc");
        let good = std::fs::read(path).unwrap();
        for length in [0, 47, 959, 960, 1200, good.len() / 2] {
            assert!(Package::decode(&good[..length]).is_err());
        }
        let mut bad = good.clone();
        bad[1100] ^= 1; // Stored flash payload; its outer CRC must reject it.
        assert!(Package::decode(&bad)
            .err()
            .unwrap()
            .contains("flash.bin checksum"));
        let mut bad = good;
        bad[0] ^= 1;
        assert!(Package::decode(&bad)
            .err()
            .unwrap()
            .contains("package header checksum"));
    }
}
