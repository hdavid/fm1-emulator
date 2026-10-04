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
            }
        }
        let flash = flash.ok_or("package has no flash.bin")?;
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
