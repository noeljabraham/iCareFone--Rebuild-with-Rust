use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use aes_gcm::aead::{AeadInPlace, KeyInit, consts::U16};
use aes_gcm::{AesGcm, Nonce, Tag, aes::Aes256};
use flate2::read::ZlibDecoder;
use hmac::{Hmac, Mac};
use md5::{Digest as Md5Digest, Md5};
use sha2::Sha256;
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CryptVersion {
    Crypt12,
    Crypt14,
    Crypt15,
}

struct ParsedHeader {
    version: CryptVersion,
    header_len: usize,
    iv: [u8; 16],
}

enum KeyMaterial {
    Crypt14 { raw: Vec<u8>, aes: [u8; 32] },
    Crypt15([u8; 32]),
}

impl Drop for KeyMaterial {
    fn drop(&mut self) {
        match self {
            Self::Crypt14 { raw, aes } => {
                raw.zeroize();
                aes.zeroize();
            }
            Self::Crypt15(key) => key.zeroize(),
        }
    }
}

pub fn decrypt_file(
    encrypted_path: &Path,
    key_input: &str,
    output_path: &Path,
) -> Result<(), String> {
    let encrypted =
        fs::read(encrypted_path).map_err(|error| format!("Could not read backup: {error}"))?;

    let trimmed = key_input.trim();
    let key_bytes = if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(trimmed).map_err(|e| format!("Invalid hex key: {e}"))?
    } else {
        let path = Path::new(trimmed);
        fs::read(path).map_err(|error| format!("Could not read key from '{trimmed}': {error}"))?
    };

    let mut key = parse_key(&key_bytes)?;
    let plaintext = decrypt_bytes(&encrypted, &mut key)?;
    if !plaintext.starts_with(b"SQLite format 3\0") {
        return Err(
            "Decryption succeeded but the payload is not a WhatsApp SQLite database".to_owned(),
        );
    }
    if output_path.exists() {
        let _ = fs::remove_file(output_path);
    }
    let temporary = output_path.with_extension("db.partial");
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(&plaintext)
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(&temporary, output_path).map_err(|error| error.to_string())?;
    Ok(())
}

fn parse_key(bytes: &[u8]) -> Result<KeyMaterial, String> {
    let mut payload = if bytes.starts_with(&[0xac, 0xed, 0x00, 0x05]) {
        parse_java_byte_array(bytes)?
    } else if bytes.len() == 64 && bytes.iter().all(u8::is_ascii_hexdigit) {
        hex::decode(bytes).map_err(|error| format!("Invalid hexadecimal key: {error}"))?
    } else {
        bytes.to_vec()
    };

    let result = match payload.len() {
        32 => {
            let mut root = [0_u8; 32];
            root.copy_from_slice(&payload);
            Ok(KeyMaterial::Crypt15(root))
        }
        131 => {
            if payload[0..2] != [0, 1] {
                return Err("Unsupported Crypt12/14 cipher version".to_owned());
            }
            if !matches!(payload[2], 1..=3) {
                return Err("Unsupported Crypt12/14 key version".to_owned());
            }
            let expected = sha2::Sha256::digest(&payload[35..51]);
            if expected.as_slice() != &payload[51..83] {
                return Err("Crypt12/14 key integrity check failed".to_owned());
            }
            if payload[83..99].iter().any(|value| *value != 0) {
                return Err("Crypt12/14 key padding is invalid".to_owned());
            }
            let mut aes = [0_u8; 32];
            aes.copy_from_slice(&payload[99..131]);
            Ok(KeyMaterial::Crypt14 {
                raw: payload.clone(),
                aes,
            })
        }
        length => Err(format!(
            "Unsupported key payload length: {length}; expected 32 or 131 bytes"
        )),
    };
    payload.zeroize();
    result
}

fn parse_java_byte_array(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let marker = bytes
        .windows(2)
        .position(|window| window == [0x78, 0x70])
        .ok_or_else(|| "Invalid Java byte-array key header".to_owned())?;
    let length_offset = marker + 2;
    if bytes.len() < length_offset + 4 {
        return Err("Truncated Java key length".to_owned());
    }
    let length =
        u32::from_be_bytes(bytes[length_offset..length_offset + 4].try_into().unwrap()) as usize;
    let payload_offset = length_offset + 4;
    if bytes.len() != payload_offset + length {
        return Err("Java key payload length does not match its header".to_owned());
    }
    Ok(bytes[payload_offset..].to_vec())
}

fn decrypt_bytes(encrypted: &[u8], key: &mut KeyMaterial) -> Result<Vec<u8>, String> {
    let header = parse_header(encrypted)?;
    let key_bytes = match (&header.version, key) {
        (CryptVersion::Crypt12 | CryptVersion::Crypt14, KeyMaterial::Crypt14 { raw, aes }) => {
            if header.version == CryptVersion::Crypt12 {
                if encrypted[0..2] != raw[0..2]
                    || encrypted[2] != raw[2]
                    || encrypted[3..35] != raw[3..35]
                    || encrypted[35..51] != raw[35..51]
                {
                    return Err(
                        "The Crypt12 backup does not match the supplied key file".to_owned()
                    );
                }
            }
            *aes
        }
        (CryptVersion::Crypt15, KeyMaterial::Crypt15(root)) => derive_crypt15_key(root),
        (CryptVersion::Crypt15, _) => {
            return Err("A Crypt15 root key is required for this backup".to_owned());
        }
        (_, _) => return Err("A Crypt12/14 key file is required for this backup".to_owned()),
    };

    let footer_len = if header.version == CryptVersion::Crypt12 {
        36
    } else {
        32
    };
    if encrypted.len() <= header.header_len + footer_len {
        return Err("Encrypted backup is truncated".to_owned());
    }
    let ciphertext_end = encrypted.len() - footer_len;
    let tag_start = ciphertext_end;
    let checksum_start = tag_start + 16;
    let ciphertext = &encrypted[header.header_len..ciphertext_end];
    let tag = &encrypted[tag_start..tag_start + 16];
    let checksum = &encrypted[checksum_start..checksum_start + 16];

    let mut hasher = Md5::new();
    hasher.update(&encrypted[..header.header_len]);
    hasher.update(ciphertext);
    hasher.update(tag);
    if hasher.finalize().as_slice() != checksum {
        return Err("Backup checksum mismatch; the file is corrupt or unsupported".to_owned());
    }

    type WhatsAppAes256Gcm = AesGcm<Aes256, U16>;
    let cipher =
        WhatsAppAes256Gcm::new_from_slice(&key_bytes).map_err(|_| "Invalid AES key".to_owned())?;
    let mut compressed = ciphertext.to_vec();
    cipher
        .decrypt_in_place_detached(
            Nonce::<U16>::from_slice(&header.iv),
            b"",
            &mut compressed,
            Tag::from_slice(tag),
        )
        .map_err(|_| {
            "Authentication failed; the key is wrong or the backup is corrupt".to_owned()
        })?;

    if compressed.starts_with(b"SQLite format 3\0") {
        return Ok(compressed);
    }
    let mut decoder = ZlibDecoder::new(compressed.as_slice());
    let mut plaintext = Vec::new();
    decoder
        .read_to_end(&mut plaintext)
        .map_err(|error| format!("Could not decompress decrypted backup: {error}"))?;
    Ok(plaintext)
}

fn parse_header(data: &[u8]) -> Result<ParsedHeader, String> {
    if data.len() >= 67 && data[0..2] == [0, 1] && matches!(data[2], 1..=3) {
        let mut iv = [0_u8; 16];
        iv.copy_from_slice(&data[51..67]);
        return Ok(ParsedHeader {
            version: CryptVersion::Crypt12,
            header_len: 67,
            iv,
        });
    }

    let mut offset = 0;
    let length = read_varint(data, &mut offset)? as usize;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| "Header length overflow".to_owned())?;
    if end > data.len() {
        return Err("Truncated protobuf backup header".to_owned());
    }
    let prefix = &data[offset..end];
    if let Some(message) = protobuf_bytes_field(prefix, 3)? {
        if let Some(value) = protobuf_bytes_field(message, 1)? {
            let iv: [u8; 16] = value
                .try_into()
                .map_err(|_| "Crypt15 IV is not 16 bytes".to_owned())?;
            return Ok(ParsedHeader {
                version: CryptVersion::Crypt15,
                header_len: end,
                iv,
            });
        }
    }
    if let Some(message) = protobuf_bytes_field(prefix, 2)? {
        if let Some(value) = protobuf_bytes_field(message, 5)? {
            let iv: [u8; 16] = value
                .try_into()
                .map_err(|_| "Crypt14 IV is not 16 bytes".to_owned())?;
            return Ok(ParsedHeader {
                version: CryptVersion::Crypt14,
                header_len: end,
                iv,
            });
        }
    }
    Err("Unrecognized WhatsApp encrypted-backup header".to_owned())
}

fn protobuf_bytes_field(data: &[u8], wanted: u64) -> Result<Option<&[u8]>, String> {
    let mut offset = 0;
    while offset < data.len() {
        let key = read_varint(data, &mut offset)?;
        let field = key >> 3;
        match key & 7 {
            0 => {
                let _ = read_varint(data, &mut offset)?;
            }
            1 => {
                offset = offset
                    .checked_add(8)
                    .ok_or_else(|| "Protobuf overflow".to_owned())?
            }
            2 => {
                let length = read_varint(data, &mut offset)? as usize;
                let end = offset
                    .checked_add(length)
                    .ok_or_else(|| "Protobuf overflow".to_owned())?;
                if end > data.len() {
                    return Err("Truncated protobuf field".to_owned());
                }
                if field == wanted {
                    return Ok(Some(&data[offset..end]));
                }
                offset = end;
            }
            5 => {
                offset = offset
                    .checked_add(4)
                    .ok_or_else(|| "Protobuf overflow".to_owned())?
            }
            wire => return Err(format!("Unsupported protobuf wire type {wire}")),
        }
        if offset > data.len() {
            return Err("Truncated protobuf field".to_owned());
        }
    }
    Ok(None)
}

fn read_varint(data: &[u8], offset: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    for shift in (0..70).step_by(7) {
        let byte = *data
            .get(*offset)
            .ok_or_else(|| "Truncated varint".to_owned())?;
        *offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("Varint is too long".to_owned())
}

fn derive_crypt15_key(root: &[u8; 32]) -> [u8; 32] {
    type HmacSha256 = Hmac<Sha256>;
    let mut extract = <HmacSha256 as Mac>::new_from_slice(&[0_u8; 32]).expect("fixed HMAC key");
    extract.update(root);
    let private = extract.finalize().into_bytes();
    let mut expand = <HmacSha256 as Mac>::new_from_slice(&private).expect("fixed HMAC key");
    expand.update(b"backup encryption");
    expand.update(&[1]);
    expand.finalize().into_bytes().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_java_serialized_keys() {
        let mut data = vec![0xac, 0xed, 0, 5, 0x75, 0x72, 0x78, 0x70, 0, 0, 0, 3];
        data.extend_from_slice(&[1, 2, 3]);
        assert_eq!(parse_java_byte_array(&data).unwrap(), [1, 2, 3]);
    }
}

