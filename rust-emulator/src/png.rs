// SPDX-License-Identifier: GPL-3.0-only
// Minimal dependency-free PNG encoder (RGB8, zlib stored blocks) for saving
// the emulated LCD framebuffer from headless runs.

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode `0x00RRGGBB` pixels, row-major, as a PNG file.
pub fn encode_rgb(width: usize, height: usize, pixels: &[u32]) -> Result<Vec<u8>, String> {
    if pixels.len() != width * height || width == 0 || height == 0 {
        return Err("pixel buffer does not match the image size".into());
    }
    let mut raw = Vec::with_capacity(height * (width * 3 + 1));
    for row in pixels.chunks(width) {
        raw.push(0); // filter: none
        for &rgb in row {
            raw.extend_from_slice(&[(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == blocks.len()));
        let length = block.len() as u16;
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc_and_adler_match_reference_values() {
        assert_eq!(super::crc32(b"IEND"), 0xae42_6082);
        assert_eq!(super::adler32(b"Wikipedia"), 0x11e6_0398);
    }

    #[test]
    fn encodes_a_valid_signature_and_size() {
        let png = super::encode_rgb(2, 1, &[0xff0000, 0x00ff00]).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(&png[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
        assert!(super::encode_rgb(2, 2, &[0]).is_err());
    }
}
