//! Chunk decoding: decrypt, then decompress (VZip/LZMA, VZstd, or plain zip).

use crate::{Error, Result, crypto};
use sha1::{Digest, Sha1};
use std::io::Read;

const MAX_CHUNK_SIZE: usize = 32 * 1024 * 1024;
const MAX_PREALLOC: usize = 1024 * 1024;
/// Upper bound on LZMA dictionary + probability model memory (KiB).
const MAX_LZMA_MEM_KB: u32 = 64 * 1024;

pub fn decode(raw: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    let plain = crypto::symmetric_decrypt(raw, key)?;
    decompress(&plain)
}

pub fn decode_with_key(raw: &[u8], key: &crypto::DepotKey, scratch: &mut Vec<u8>) -> Result<Vec<u8>> {
    key.decrypt_into(raw, scratch)?;
    decompress(scratch)
}

pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    if data.starts_with(b"VSZa") {
        vzstd(data)
    } else if data.starts_with(b"VZa") {
        vzip(data)
    } else if data.starts_with(b"PK") {
        pkzip(data)
    } else {
        Err(Error::Format("unknown chunk compression".into()))
    }
}

/// Valve's LZMA wrapper: "VZa" + u32 + 5 bytes of LZMA properties + data,
/// footer: u32 crc32, u32 uncompressed size, "zv".
fn vzip(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 7 + 5 + 10 || !data.ends_with(b"zv") {
        return Err(Error::Format("bad VZip chunk".into()));
    }
    let footer = &data[data.len() - 10..];
    let size = u32::from_le_bytes(footer[4..8].try_into().unwrap()) as u64;
    if size > MAX_CHUNK_SIZE as u64 {
        return Err(Error::Format("vzip chunk exceeds 32 MiB limit".into()));
    }
    let props_byte = data[7];
    let dict_size = u32::from_le_bytes(data[8..12].try_into().unwrap());
    let mem_kb = lzma_rust2::lzma_get_memory_usage_by_props(dict_size, props_byte)
        .map_err(|e| Error::Format(format!("lzma props: {e}")))?;
    if mem_kb > MAX_LZMA_MEM_KB {
        return Err(Error::Format("vzip dictionary exceeds memory limit".into()));
    }
    let body = &data[12..data.len() - 10];
    let reader = lzma_rust2::LzmaReader::new_with_props(body, size, props_byte, dict_size, None)
        .map_err(|e| Error::Format(format!("lzma: {e}")))?;
    let mut out = Vec::with_capacity((size as usize).min(MAX_PREALLOC));
    reader
        .take(MAX_CHUNK_SIZE as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| Error::Format(format!("lzma: {e}")))?;
    if out.len() > MAX_CHUNK_SIZE {
        return Err(Error::Format("vzip chunk exceeds 32 MiB limit".into()));
    }
    out.truncate(size as usize);
    Ok(out)
}

/// Valve's zstd wrapper: "VSZa" + u32 crc, then a zstd frame, then a footer.
fn vzstd(data: &[u8]) -> Result<Vec<u8>> {
    let payload = data.get(8..).ok_or_else(|| Error::Format("bad VZstd chunk".into()))?;
    let dec = ruzstd::decoding::StreamingDecoder::new(payload).map_err(|e| Error::Format(format!("zstd: {e}")))?;
    let cap = match dec.decoder.content_size() {
        0 => payload.len().saturating_mul(2).min(MAX_PREALLOC),
        size => size.min(MAX_PREALLOC as u64) as usize,
    };
    let mut out = Vec::with_capacity(cap);
    dec.take(MAX_CHUNK_SIZE as u64 + 1).read_to_end(&mut out).map_err(|e| Error::Format(format!("zstd: {e}")))?;
    if out.len() > MAX_CHUNK_SIZE {
        return Err(Error::Format("vzstd chunk exceeds 32 MiB limit".into()));
    }
    Ok(out)
}

fn pkzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| Error::Format(format!("zip: {e}")))?;
    let file = archive.by_index(0).map_err(|e| Error::Format(format!("zip: {e}")))?;
    if file.size() > MAX_CHUNK_SIZE as u64 {
        return Err(Error::Format("zip chunk exceeds 32 MiB limit".into()));
    }
    let mut out = Vec::with_capacity((file.size() as usize).min(MAX_PREALLOC));
    file.take(MAX_CHUNK_SIZE as u64 + 1).read_to_end(&mut out)?;
    if out.len() > MAX_CHUNK_SIZE {
        return Err(Error::Format("zip chunk exceeds 32 MiB limit".into()));
    }
    Ok(out)
}

/// A chunk's id is the 20-byte SHA-1 of its decompressed bytes.
pub fn verify(data: &[u8], sha: &[u8]) -> bool {
    sha.len() == 20 && Sha1::digest(data).as_slice() == sha
}

pub fn hex(bytes: &[u8]) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(LUT[(b >> 4) as usize]);
        out.push(LUT[(b & 0x0f) as usize]);
    }
    // SAFETY: all bytes pushed are ASCII hex digits.
    unsafe { String::from_utf8_unchecked(out) }
}

#[cfg(test)]
mod decompression_bounds_tests {
    use super::*;

    #[test]
    fn rejects_oversized_and_truncated_chunks() {
        use std::io::Write;
        // Truncated VZstd header should return Err, never panic.
        assert!(decompress(b"VSZa12").is_err());

        // Zip bomb > 32 MiB.
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            z.start_file("z", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
            let block = vec![0u8; 1024 * 1024];
            for _ in 0..33 {
                z.write_all(&block).unwrap();
            }
            z.finish().unwrap();
        }
        assert!(decompress(buf.get_ref()).is_err(), "zip chunk > 32 MiB must fail");

        // VZip claiming > 32 MiB in footer.
        let mut vzip_bad = vec![b'V', b'Z', b'a', 0, 0, 0, 0, 0x5d, 0, 0, 0x10, 0, 0];
        vzip_bad.extend_from_slice(&0u32.to_le_bytes()); // crc
        vzip_bad.extend_from_slice(&(33u32 * 1024 * 1024).to_le_bytes());
        vzip_bad.extend_from_slice(b"zv");
        assert!(decompress(&vzip_bad).is_err(), "vzip chunk > 32 MiB must fail");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_depot_lzma_and_zstd_frames() {
        // Python lzma FORMAT_ALONE wrapped as VZip, and zstd stdin
        // output (no frame content size) wrapped as VZstd.
        let vzip = b"VZa\xb6\x06\x90\xa1\x5d\x00\x00\x80\x00\x00\x20\x9d\x4a\x47\x11\x8d\x8d\xeb\x99\xb4\x9d\x16\xc5\x83\x47\xa9\xc1\xbf\xf3\xf3\x3c\x21\xd1\xff\xff\xf1\x0c\x00\x00\xb6\x06\x90\xa1\x13\x00\x00\x00zv";
        let vzstd = b"VSZa\xb6\x06\x90\xa1\x28\xb5\x2f\xfd\x04\x58\x99\x00\x00Aurora depot chunk\n\x42\xc9\x32\xed";
        for raw in [vzip.as_slice(), vzstd.as_slice()] {
            assert_eq!(decompress(raw).unwrap(), b"Aurora depot chunk\n");
        }
    }

    #[test]
    fn decodes_a_zip_chunk() {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            z.start_file("z", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
            z.write_all(b"chunk payload").unwrap();
            z.finish().unwrap();
        }
        let key = [9u8; 32];
        let enc = crate::crypto::symmetric_encrypt_for_tests(buf.get_ref(), &key, [1u8; 16]);
        let out = decode(&enc, &key).unwrap();
        assert_eq!(out, b"chunk payload");
        assert!(verify(&out, Sha1::digest(b"chunk payload").as_slice()));
    }

    #[test]
    fn verify_rejects_empty_or_wrong_len_sha() {
        assert!(!verify(b"chunk payload", &[]));
        assert!(!verify(b"", &[]));
        let full = Sha1::digest(b"chunk payload");
        assert!(!verify(b"chunk payload", &full[..19]));
        assert!(!verify(b"chunk payload", &[0u8; 20]));
    }
}
