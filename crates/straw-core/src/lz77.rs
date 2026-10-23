use anyhow::{bail, Result};

const MAGIC: &[u8; 4] = b"LZ77";
const WINDOW_SIZE: usize = 4096;
const WINDOW_START: usize = 0xFEE;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 18;

pub fn decompress_lz77(data: &[u8], expected_size: Option<usize>, strict: bool) -> Result<Vec<u8>> {
    let (stream, expected_size) = if data.starts_with(MAGIC) {
        if data.len() < 12 {
            bail!("truncated LZ77 header");
        }
        let expected = read_u32_le(data, 4) as usize;
        let comp_size = read_u32_le(data, 8) as usize;
        let stream_end = 12 + comp_size;
        if data.len() < stream_end {
            if strict {
                bail!(
                    "truncated LZ77 stream: have {} bytes, header comp_size={}",
                    data.len().saturating_sub(12),
                    comp_size
                );
            }
            (&data[12..], expected)
        } else {
            (&data[12..stream_end], expected)
        }
    } else {
        let expected = expected_size
            .ok_or_else(|| anyhow::anyhow!("expected_size or LZ77 header required"))?;
        (data, expected)
    };

    let mut out = Vec::with_capacity(expected_size);
    let mut window = [0_u8; WINDOW_SIZE];
    let mut window_pos = WINDOW_START;
    let mut pos = 0;

    while pos < stream.len() && out.len() < expected_size {
        let flags = stream[pos];
        pos += 1;

        for bit in 0..8 {
            if out.len() >= expected_size {
                break;
            }

            let is_literal = (flags & (1 << bit)) != 0;
            if is_literal {
                if pos >= stream.len() {
                    break;
                }
                let value = stream[pos];
                pos += 1;
                out.push(value);
                window[window_pos] = value;
                window_pos = (window_pos + 1) & 0xFFF;
            } else {
                if pos + 1 >= stream.len() {
                    break;
                }
                let b1 = stream[pos];
                let b2 = stream[pos + 1];
                pos += 2;

                let mut offset = b1 as usize | (((b2 & 0xF0) as usize) << 4);
                let length = ((b2 & 0x0F) as usize) + MIN_MATCH;

                for _ in 0..length {
                    if out.len() >= expected_size {
                        break;
                    }
                    let value = window[offset];
                    out.push(value);
                    window[window_pos] = value;
                    window_pos = (window_pos + 1) & 0xFFF;
                    offset = (offset + 1) & 0xFFF;
                }
            }
        }
    }

    if strict && out.len() != expected_size {
        bail!(
            "incomplete LZ77 decompression: output={} expected={}",
            out.len(),
            expected_size
        );
    }

    Ok(out)
}

pub fn compress_lz77(uncompressed: &[u8], all_literal: bool) -> Vec<u8> {
    let mut compressed = Vec::new();
    let mut window = [0_u8; WINDOW_SIZE];
    let mut window_pos = WINDOW_START;
    let mut src_pos = 0;
    let mut block_flags = 0_u8;
    let mut bit_count = 0;
    let mut block_tokens = Vec::new();

    while src_pos < uncompressed.len() {
        let max_len = MAX_MATCH.min(uncompressed.len() - src_pos);
        let (match_offset, mut match_len) = if !all_literal && max_len >= MIN_MATCH {
            find_best_match(uncompressed, src_pos, max_len, &window, window_pos)
        } else {
            (0, 0)
        };

        if match_len < MIN_MATCH {
            match_len = 1;
            block_flags |= 1 << bit_count;
            block_tokens.push(uncompressed[src_pos]);
        } else {
            let b1 = (match_offset & 0xFF) as u8;
            let b2 =
                (((match_offset >> 4) & 0xF0) as u8) | (((match_len - MIN_MATCH) & 0x0F) as u8);
            block_tokens.extend([b1, b2]);
        }

        for k in 0..match_len {
            window[window_pos] = uncompressed[src_pos + k];
            window_pos = (window_pos + 1) & 0xFFF;
        }

        src_pos += match_len;
        bit_count += 1;
        if bit_count == 8 {
            flush_block(
                &mut compressed,
                &mut block_flags,
                &mut bit_count,
                &mut block_tokens,
            );
        }
    }

    flush_block(
        &mut compressed,
        &mut block_flags,
        &mut bit_count,
        &mut block_tokens,
    );

    let mut out = Vec::with_capacity(12 + compressed.len());
    out.extend(MAGIC);
    out.extend((uncompressed.len() as u32).to_le_bytes());
    out.extend((compressed.len() as u32).to_le_bytes());
    out.extend(compressed);
    out
}

fn find_best_match(
    uncompressed: &[u8],
    src_pos: usize,
    max_len: usize,
    window: &[u8; WINDOW_SIZE],
    window_pos: usize,
) -> (usize, usize) {
    let mut best_len = 0;
    let mut best_offset = 0;

    for dist in 1..=WINDOW_SIZE {
        let w_idx = window_pos.wrapping_sub(dist) & 0xFFF;
        let mut curr_len = 0;
        while curr_len < max_len {
            let expected_byte = if curr_len < dist {
                window[(w_idx + curr_len) & 0xFFF]
            } else {
                uncompressed[src_pos + curr_len - dist]
            };
            if expected_byte != uncompressed[src_pos + curr_len] {
                break;
            }
            curr_len += 1;
        }
        if curr_len > best_len {
            best_len = curr_len;
            best_offset = w_idx;
            if best_len == max_len {
                break;
            }
        }
    }

    (best_offset, best_len)
}

fn flush_block(
    compressed: &mut Vec<u8>,
    block_flags: &mut u8,
    bit_count: &mut usize,
    block_tokens: &mut Vec<u8>,
) {
    if *bit_count > 0 {
        compressed.push(*block_flags);
        compressed.append(block_tokens);
        *block_flags = 0;
        *bit_count = 0;
    }
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        data[offset..offset + 4]
            .try_into()
            .expect("slice length checked"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datafat::{find_row, read_entries};

    #[test]
    fn literal_roundtrip() {
        let input = b"hello strawberry panic";
        let compressed = compress_lz77(input, true);
        let decompressed = decompress_lz77(&compressed, None, true).unwrap();
        assert_eq!(decompressed, input);
    }

    #[test]
    fn compressed_roundtrip() {
        let input = b"aaaaabbbbbccccccccccccccccccccccdddddaaaaabbbbb";
        let compressed = compress_lz77(input, false);
        let decompressed = decompress_lz77(&compressed, None, true).unwrap();
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rejects_truncated_header() {
        assert!(decompress_lz77(b"LZ77", None, true).is_err());
    }

    #[test]
    fn decompresses_real_lz77_entry_when_available() {
        let path = std::path::Path::new("originales/Data.bin");
        if !path.exists() {
            return;
        }

        let rows = read_entries(path).unwrap();
        let data = std::fs::read(path).unwrap();
        let entry = rows
            .iter()
            .find(|entry| {
                if entry.offset == 0 || entry.size < 12 {
                    return false;
                }
                let start = entry.offset as usize;
                data.get(start..start + 4) == Some(&b"LZ77"[..])
            })
            .or_else(|| find_row(&rows, 7461));

        let Some(entry) = entry else { return };
        let start = entry.offset as usize;
        let end = start + entry.size as usize;
        if end > data.len() || data.get(start..start + 4) != Some(&b"LZ77"[..]) {
            return;
        }

        let decompressed = decompress_lz77(&data[start..end], None, true).unwrap();
        assert!(!decompressed.is_empty());
    }
}
