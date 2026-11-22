//! Steam's "symmetric" encryption for chunks and manifest filenames:
//! the first 16 bytes are the IV, encrypted with AES-256-ECB; the rest is
//! AES-256-CBC with PKCS#7 padding.

use crate::{Error, Result};
use aes::Aes256;
use aes::cipher::{BlockDecryptMut, KeyInit, KeyIvInit, block_padding::Pkcs7, generic_array::GenericArray};
use base64::Engine;

type CbcDec = cbc::Decryptor<Aes256>;
type EcbDec = ecb::Decryptor<Aes256>;

/// Pre-expanded AES-256 key schedule for decrypting many filenames or chunks
/// under the same depot key without re-running key expansion every time.
pub struct DepotKey {
    ecb: EcbDec,
    key: [u8; 32],
}

impl DepotKey {
    pub fn new(key: &[u8]) -> Result<Self> {
        let key: [u8; 32] = key
            .try_into()
            .map_err(|_| Error::Crypto("depot key must be 32 bytes".into()))?;
        let ecb = EcbDec::new_from_slice(&key).map_err(|e| Error::Crypto(e.to_string()))?;
        Ok(Self { ecb, key })
    }

    /// Decrypt `data` into `out` (reusing `out`'s allocation).
    pub fn decrypt_into(&self, data: &[u8], out: &mut Vec<u8>) -> Result<()> {
        if data.len() < 32 || data.len() % 16 != 0 {
            return Err(Error::Crypto("ciphertext has a bad length".into()));
        }
        let mut iv = GenericArray::clone_from_slice(&data[..16]);
        self.ecb.clone().decrypt_block_mut(&mut iv);
        out.clear();
        out.extend_from_slice(&data[16..]);
        let plain_len = CbcDec::new_from_slices(&self.key, &iv)
            .map_err(|e| Error::Crypto(e.to_string()))?
            .decrypt_padded_mut::<Pkcs7>(out)
            .map_err(|e| Error::Crypto(e.to_string()))?
            .len();
        out.truncate(plain_len);
        Ok(())
    }

    pub fn decrypt_filename_with(&self, b64: &str, raw_buf: &mut Vec<u8>, plain_buf: &mut Vec<u8>) -> Result<String> {
        let trimmed = b64.trim();
        raw_buf.clear();
        if trimmed.as_bytes().contains(&b'\n') || trimmed.as_bytes().contains(&b'\r') {
            let cleaned: Vec<u8> = trimmed.bytes().filter(|&b| b != b'\n' && b != b'\r').collect();
            base64::engine::general_purpose::STANDARD
                .decode_vec(&cleaned, raw_buf)
                .map_err(|e| Error::Crypto(e.to_string()))?;
        } else {
            base64::engine::general_purpose::STANDARD
                .decode_vec(trimmed.as_bytes(), raw_buf)
                .map_err(|e| Error::Crypto(e.to_string()))?;
        }
        self.decrypt_into(raw_buf, plain_buf)?;
        let end = plain_buf.iter().position(|b| *b == 0).unwrap_or(plain_buf.len());
        Ok(String::from_utf8_lossy(&plain_buf[..end]).into_owned())
    }
}

pub fn symmetric_decrypt(data: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    if key.len() != 32 {
        return Err(Error::Crypto("depot key must be 32 bytes".into()));
    }
    if data.len() < 32 || data.len() % 16 != 0 {
        return Err(Error::Crypto("ciphertext has a bad length".into()));
    }
    let mut iv = GenericArray::clone_from_slice(&data[..16]);
    EcbDec::new_from_slice(key)
        .map_err(|e| Error::Crypto(e.to_string()))?
        .decrypt_block_mut(&mut iv);
    let mut body = data[16..].to_vec();
    let plain_len = CbcDec::new_from_slices(key, &iv)
        .map_err(|e| Error::Crypto(e.to_string()))?
        .decrypt_padded_mut::<Pkcs7>(&mut body)
        .map_err(|e| Error::Crypto(e.to_string()))?
        .len();
    body.truncate(plain_len);
    Ok(body)
}

/// Filenames in CDN manifests are base64 of encrypted, NUL-terminated text.
pub fn decrypt_filename(b64: &str, key: &[u8]) -> Result<String> {
    let dk = DepotKey::new(key)?;
    let (mut raw, mut plain) = (Vec::new(), Vec::new());
    dk.decrypt_filename_with(b64, &mut raw, &mut plain)
}

#[cfg(test)]
pub(crate) fn symmetric_encrypt_for_tests(plain: &[u8], key: &[u8], iv: [u8; 16]) -> Vec<u8> {
    use aes::cipher::{BlockEncryptMut, block_padding::Pkcs7};
    type CbcEnc = cbc::Encryptor<Aes256>;
    type EcbEnc = ecb::Encryptor<Aes256>;
    let mut enc_iv = GenericArray::clone_from_slice(&iv);
    EcbEnc::new_from_slice(key).unwrap().encrypt_block_mut(&mut enc_iv);
    let mut buf = vec![0u8; plain.len() + 16];
    buf[..plain.len()].copy_from_slice(plain);
    let body = CbcEnc::new_from_slices(key, &iv).unwrap().encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len()).unwrap().to_vec();
    let mut out = enc_iv.to_vec();
    out.extend(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = [7u8; 32];
        let data = symmetric_encrypt_for_tests(b"hello depot", &key, [3u8; 16]);
        assert_eq!(symmetric_decrypt(&data, &key).unwrap(), b"hello depot");
    }
}
