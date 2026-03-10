//! WeCom (企业微信/WeChat Work) channel with webhook callback mode + REST API.
//!
//! Uses a Custom App with message callback URL for receiving messages,
//! and the WeCom REST API for sending messages.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use crew_core::{InboundMessage, OutboundMessage};
use eyre::{Result, WrapErr};
use reqwest::Client;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::channel::Channel;
use crate::media::{download_media, is_image};

/// Token refresh interval (slightly under 2 hours).
const TOKEN_TTL_SECS: u64 = 7000;
/// Maximum message IDs to track for dedup.
const MAX_SEEN_IDS: usize = 1000;
/// WeCom API base URL.
const WECOM_API: &str = "https://qyapi.weixin.qq.com/cgi-bin";

// ---------------------------------------------------------------------------
// Crypto helpers (pure Rust, no external deps)
// ---------------------------------------------------------------------------

/// SHA-1 hash (pure Rust).
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    // Pre-processing: padding
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit block
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);

        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };

            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut result = [0u8; 20];
    result[0..4].copy_from_slice(&h0.to_be_bytes());
    result[4..8].copy_from_slice(&h1.to_be_bytes());
    result[8..12].copy_from_slice(&h2.to_be_bytes());
    result[12..16].copy_from_slice(&h3.to_be_bytes());
    result[16..20].copy_from_slice(&h4.to_be_bytes());
    result
}

/// Verify WeCom callback signature.
/// Sort [token, timestamp, nonce, encrypt_msg], concatenate, SHA1, return hex.
fn verify_wecom_signature(token: &str, timestamp: &str, nonce: &str, encrypt_msg: &str) -> String {
    let mut parts = [token, timestamp, nonce, encrypt_msg];
    parts.sort();
    let combined: String = parts.concat();
    let hash = sha1(combined.as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Minimal base64 decode (standard alphabet with padding).
fn base64_decode(input: &str) -> Result<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn val(c: u8) -> Result<u8> {
        if c == b'=' {
            return Ok(0);
        }
        TABLE
            .iter()
            .position(|&x| x == c)
            .map(|p| p as u8)
            .ok_or_else(|| eyre::eyre!("invalid base64 character"))
    }

    let input = input.trim();
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|&b| b != b'\n' && b != b'\r')
        .collect();
    if bytes.len() % 4 != 0 {
        return Err(eyre::eyre!("invalid base64 length"));
    }

    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        let c = val(chunk[2])?;
        let d = val(chunk[3])?;

        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            out.push((c << 6) | d);
        }
    }
    Ok(out)
}

/// Minimal base64 encode (standard alphabet with padding).
#[cfg(test)]
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        result.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// AES-256 S-Box.
const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// AES-256 inverse S-Box.
const INV_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
    0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
    0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
    0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
    0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
    0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
    0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
    0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
    0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
    0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
    0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
    0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
    0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
    0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
    0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
];

const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

fn sub_word(w: [u8; 4]) -> [u8; 4] {
    [
        SBOX[w[0] as usize],
        SBOX[w[1] as usize],
        SBOX[w[2] as usize],
        SBOX[w[3] as usize],
    ]
}

fn rot_word(w: [u8; 4]) -> [u8; 4] {
    [w[1], w[2], w[3], w[0]]
}

/// AES-256 key expansion (15 round keys).
fn aes256_key_expansion(key: &[u8; 32]) -> [[u8; 16]; 15] {
    let mut w = [[0u8; 4]; 60];
    for i in 0..8 {
        w[i] = [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]];
    }

    for i in 8..60 {
        let mut temp = w[i - 1];
        if i % 8 == 0 {
            temp = sub_word(rot_word(temp));
            temp[0] ^= RCON[i / 8 - 1];
        } else if i % 8 == 4 {
            temp = sub_word(temp);
        }
        for j in 0..4 {
            w[i][j] = w[i - 8][j] ^ temp[j];
        }
    }

    let mut round_keys = [[0u8; 16]; 15];
    for r in 0..15 {
        for c in 0..4 {
            round_keys[r][c * 4..c * 4 + 4].copy_from_slice(&w[r * 4 + c]);
        }
    }
    round_keys
}

/// AES-256 decrypt a single 16-byte block.
fn aes256_decrypt_block(round_keys: &[[u8; 16]; 15], block: &mut [u8; 16]) {
    fn gmul(mut a: u8, mut b: u8) -> u8 {
        let mut p = 0u8;
        for _ in 0..8 {
            if b & 1 != 0 {
                p ^= a;
            }
            let hi = a & 0x80;
            a <<= 1;
            if hi != 0 {
                a ^= 0x1b;
            }
            b >>= 1;
        }
        p
    }

    fn add_round_key(state: &mut [u8; 16], rk: &[u8; 16]) {
        for i in 0..16 {
            state[i] ^= rk[i];
        }
    }

    fn inv_sub_bytes(state: &mut [u8; 16]) {
        for b in state.iter_mut() {
            *b = INV_SBOX[*b as usize];
        }
    }

    fn inv_shift_rows(state: &mut [u8; 16]) {
        // Row 1: shift right by 1
        let t = state[13];
        state[13] = state[9];
        state[9] = state[5];
        state[5] = state[1];
        state[1] = t;
        // Row 2: shift right by 2
        let (t0, t1) = (state[2], state[6]);
        state[2] = state[10];
        state[6] = state[14];
        state[10] = t0;
        state[14] = t1;
        // Row 3: shift right by 3
        let t = state[3];
        state[3] = state[7];
        state[7] = state[11];
        state[11] = state[15];
        state[15] = t;
    }

    fn inv_mix_columns(state: &mut [u8; 16]) {
        for c in 0..4 {
            let i = c * 4;
            let (s0, s1, s2, s3) = (state[i], state[i + 1], state[i + 2], state[i + 3]);
            state[i] = gmul(s0, 14) ^ gmul(s1, 11) ^ gmul(s2, 13) ^ gmul(s3, 9);
            state[i + 1] = gmul(s0, 9) ^ gmul(s1, 14) ^ gmul(s2, 11) ^ gmul(s3, 13);
            state[i + 2] = gmul(s0, 13) ^ gmul(s1, 9) ^ gmul(s2, 14) ^ gmul(s3, 11);
            state[i + 3] = gmul(s0, 11) ^ gmul(s1, 13) ^ gmul(s2, 9) ^ gmul(s3, 14);
        }
    }

    add_round_key(block, &round_keys[14]);

    for round in (1..14).rev() {
        inv_shift_rows(block);
        inv_sub_bytes(block);
        add_round_key(block, &round_keys[round]);
        inv_mix_columns(block);
    }

    inv_shift_rows(block);
    inv_sub_bytes(block);
    add_round_key(block, &round_keys[0]);
}

/// AES-256 encrypt a single 16-byte block.
#[cfg(test)]
fn aes256_encrypt_block(round_keys: &[[u8; 16]; 15], block: &mut [u8; 16]) {
    fn gmul(mut a: u8, mut b: u8) -> u8 {
        let mut p = 0u8;
        for _ in 0..8 {
            if b & 1 != 0 {
                p ^= a;
            }
            let hi = a & 0x80;
            a <<= 1;
            if hi != 0 {
                a ^= 0x1b;
            }
            b >>= 1;
        }
        p
    }

    fn add_round_key(state: &mut [u8; 16], rk: &[u8; 16]) {
        for i in 0..16 {
            state[i] ^= rk[i];
        }
    }

    fn sub_bytes(state: &mut [u8; 16]) {
        for b in state.iter_mut() {
            *b = SBOX[*b as usize];
        }
    }

    fn shift_rows(state: &mut [u8; 16]) {
        // Row 1: shift left by 1
        let t = state[1];
        state[1] = state[5];
        state[5] = state[9];
        state[9] = state[13];
        state[13] = t;
        // Row 2: shift left by 2
        let (t0, t1) = (state[2], state[6]);
        state[2] = state[10];
        state[6] = state[14];
        state[10] = t0;
        state[14] = t1;
        // Row 3: shift left by 3
        let t = state[15];
        state[15] = state[11];
        state[11] = state[7];
        state[7] = state[3];
        state[3] = t;
    }

    fn mix_columns(state: &mut [u8; 16]) {
        for c in 0..4 {
            let i = c * 4;
            let (s0, s1, s2, s3) = (state[i], state[i + 1], state[i + 2], state[i + 3]);
            state[i] = gmul(s0, 2) ^ gmul(s1, 3) ^ s2 ^ s3;
            state[i + 1] = s0 ^ gmul(s1, 2) ^ gmul(s2, 3) ^ s3;
            state[i + 2] = s0 ^ s1 ^ gmul(s2, 2) ^ gmul(s3, 3);
            state[i + 3] = gmul(s0, 3) ^ s1 ^ s2 ^ gmul(s3, 2);
        }
    }

    add_round_key(block, &round_keys[0]);

    for round in 1..14 {
        sub_bytes(block);
        shift_rows(block);
        mix_columns(block);
        add_round_key(block, &round_keys[round]);
    }

    sub_bytes(block);
    shift_rows(block);
    add_round_key(block, &round_keys[14]);
}

/// AES-256-CBC decrypt in place (PKCS7 padding NOT removed).
fn aes256_cbc_decrypt(key: &[u8; 32], iv: &[u8], data: &mut [u8]) -> Result<()> {
    if data.len() % 16 != 0 {
        return Err(eyre::eyre!("data not aligned to 16 bytes"));
    }

    let round_keys = aes256_key_expansion(key);
    let mut prev_block = [0u8; 16];
    prev_block.copy_from_slice(iv);

    for i in (0..data.len()).step_by(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(&data[i..i + 16]);
        let cipher_block = block;

        aes256_decrypt_block(&round_keys, &mut block);

        for j in 0..16 {
            block[j] ^= prev_block[j];
        }
        prev_block = cipher_block;
        data[i..i + 16].copy_from_slice(&block);
    }
    Ok(())
}

/// AES-256-CBC encrypt in place.
#[cfg(test)]
fn aes256_cbc_encrypt(key: &[u8; 32], iv: &[u8], data: &mut [u8]) -> Result<()> {
    if data.len() % 16 != 0 {
        return Err(eyre::eyre!("data not aligned to 16 bytes"));
    }

    let round_keys = aes256_key_expansion(key);
    let mut prev_block = [0u8; 16];
    prev_block.copy_from_slice(iv);

    for i in (0..data.len()).step_by(16) {
        for j in 0..16 {
            data[i + j] ^= prev_block[j];
        }
        let mut block = [0u8; 16];
        block.copy_from_slice(&data[i..i + 16]);
        aes256_encrypt_block(&round_keys, &mut block);
        data[i..i + 16].copy_from_slice(&block);
        prev_block.copy_from_slice(&block);
    }
    Ok(())
}

/// Decode WeCom EncodingAESKey (43-char base64 + "=") into 32-byte AES key.
fn decode_aes_key(encoding_aes_key: &str) -> Result<[u8; 32]> {
    let padded = format!("{encoding_aes_key}=");
    let bytes = base64_decode(&padded)?;
    if bytes.len() != 32 {
        return Err(eyre::eyre!(
            "EncodingAESKey decoded to {} bytes, expected 32",
            bytes.len()
        ));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Decrypt a WeCom encrypted message.
/// Returns (xml_content, corp_id).
fn decrypt_wecom_message(aes_key: &[u8; 32], ciphertext_b64: &str) -> Result<(String, String)> {
    let mut buf = base64_decode(ciphertext_b64).wrap_err("base64 decode failed")?;
    if buf.len() < 32 || buf.len() % 16 != 0 {
        return Err(eyre::eyre!("invalid ciphertext length"));
    }

    // IV is first 16 bytes of the AES key
    let iv: [u8; 16] = aes_key[..16].try_into().unwrap();
    aes256_cbc_decrypt(aes_key, &iv, &mut buf)?;

    // PKCS7 unpad
    if let Some(&pad_len) = buf.last() {
        let pad_len = pad_len as usize;
        if pad_len > 0 && pad_len <= 16 && buf.len() >= pad_len {
            buf.truncate(buf.len() - pad_len);
        }
    }

    // WeCom format: 16 bytes random + 4 bytes msg_len (big endian) + xml_content + corp_id
    if buf.len() < 20 {
        return Err(eyre::eyre!("decrypted data too short"));
    }

    let msg_len = u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]) as usize;
    if buf.len() < 20 + msg_len {
        return Err(eyre::eyre!("msg_len exceeds decrypted data"));
    }

    let xml_content = String::from_utf8(buf[20..20 + msg_len].to_vec())
        .wrap_err("decrypted XML not valid UTF-8")?;
    let corp_id =
        String::from_utf8(buf[20 + msg_len..].to_vec()).wrap_err("corp_id not valid UTF-8")?;

    Ok((xml_content, corp_id))
}

/// Encrypt a plaintext for WeCom response (URL verification echo).
#[cfg(test)]
fn encrypt_wecom_message(aes_key: &[u8; 32], plaintext: &str, corp_id: &str) -> Result<String> {
    // 16 bytes random + 4 bytes msg_len + plaintext + corp_id
    let msg_bytes = plaintext.as_bytes();
    let corp_bytes = corp_id.as_bytes();
    let msg_len = msg_bytes.len() as u32;

    let mut data = Vec::new();
    // 16 random bytes
    for i in 0..16u8 {
        data.push(i.wrapping_mul(7).wrapping_add(3));
    }
    data.extend_from_slice(&msg_len.to_be_bytes());
    data.extend_from_slice(msg_bytes);
    data.extend_from_slice(corp_bytes);

    // PKCS7 pad to 16-byte boundary
    let pad_len = 16 - (data.len() % 16);
    for _ in 0..pad_len {
        data.push(pad_len as u8);
    }

    let iv: [u8; 16] = aes_key[..16].try_into().unwrap();
    aes256_cbc_encrypt(aes_key, &iv, &mut data)?;

    Ok(base64_encode(&data))
}

// ---------------------------------------------------------------------------
// Simple XML field extraction
// ---------------------------------------------------------------------------

/// Extract the text content of an XML tag, e.g. `<Content>hello</Content>` → "hello".
/// Handles `<![CDATA[...]]>` wrappers.
fn xml_extract(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    let inner = xml[start..end].trim();
    // Strip CDATA wrapper
    let inner = inner
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(inner);
    Some(inner.to_string())
}

// ---------------------------------------------------------------------------
// WeComChannel
// ---------------------------------------------------------------------------

pub struct WeComChannel {
    corp_id: String,
    agent_id: String,
    agent_secret: String,
    verification_token: String,
    aes_key: [u8; 32],
    allowed_senders: HashSet<String>,
    shutdown: Arc<AtomicBool>,
    http: Client,
    media_dir: PathBuf,
    token_cache: Arc<tokio::sync::Mutex<Option<(String, Instant)>>>,
    seen_ids: Arc<std::sync::Mutex<HashSet<String>>>,
    webhook_port: u16,
}

impl WeComChannel {
    pub fn new(
        corp_id: &str,
        agent_id: &str,
        agent_secret: &str,
        verification_token: &str,
        encoding_aes_key: &str,
        allowed_senders: Vec<String>,
        shutdown: Arc<AtomicBool>,
        media_dir: PathBuf,
    ) -> Self {
        let aes_key = decode_aes_key(encoding_aes_key).unwrap_or_else(|e| {
            warn!("WeCom: failed to decode EncodingAESKey: {e}, using zero key");
            [0u8; 32]
        });
        Self {
            corp_id: corp_id.to_string(),
            agent_id: agent_id.to_string(),
            agent_secret: agent_secret.to_string(),
            verification_token: verification_token.to_string(),
            aes_key,
            allowed_senders: allowed_senders.into_iter().collect(),
            shutdown,
            http: Client::new(),
            media_dir,
            token_cache: Arc::new(tokio::sync::Mutex::new(None)),
            seen_ids: Arc::new(std::sync::Mutex::new(HashSet::new())),
            webhook_port: 9322,
        }
    }

    /// Set webhook port (default 9322).
    pub fn with_webhook_port(mut self, port: u16) -> Self {
        self.webhook_port = port;
        self
    }

    fn check_allowed(&self, sender_id: &str) -> bool {
        self.allowed_senders.is_empty() || self.allowed_senders.contains(sender_id)
    }

    /// Get or refresh access token.
    async fn get_token(&self) -> Result<String> {
        let mut cache = self.token_cache.lock().await;
        if let Some((ref token, ref created)) = *cache {
            if created.elapsed().as_secs() < TOKEN_TTL_SECS {
                return Ok(token.clone());
            }
        }

        let url = format!(
            "{WECOM_API}/gettoken?corpid={}&corpsecret={}",
            self.corp_id, self.agent_secret
        );
        let resp: serde_json::Value = self
            .http
            .get(&url)
            .send()
            .await
            .wrap_err("failed to get WeCom access token")?
            .json()
            .await?;

        let errcode = resp.get("errcode").and_then(|v| v.as_i64()).unwrap_or(-1);
        if errcode != 0 {
            let errmsg = resp
                .get("errmsg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(eyre::eyre!("WeCom token error: {errmsg} (code {errcode})"));
        }

        let token = resp
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("WeCom: no access_token in response"))?
            .to_string();

        *cache = Some((token.clone(), Instant::now()));
        Ok(token)
    }

    /// Download a media resource from WeCom.
    async fn download_media_file(&self, media_id: &str, ext: &str) -> Result<PathBuf> {
        let token = self.get_token().await?;
        let url = format!("{WECOM_API}/media/get?access_token={token}&media_id={media_id}");
        let filename = format!("wecom_{}{}", Utc::now().timestamp_millis(), ext);
        download_media(&self.http, &url, &[], &self.media_dir, &filename).await
    }

    /// Upload a media file and return the media_id.
    async fn upload_media(&self, file_path: &str, media_type: &str) -> Result<String> {
        let token = self.get_token().await?;
        let data = std::fs::read(file_path).wrap_err("failed to read media file")?;
        let filename = std::path::Path::new(file_path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());

        let part = reqwest::multipart::Part::bytes(data)
            .file_name(filename)
            .mime_str("application/octet-stream")?;
        let form = reqwest::multipart::Form::new().part("media", part);

        let url = format!("{WECOM_API}/media/upload?access_token={token}&type={media_type}");
        let resp: serde_json::Value = self
            .http
            .post(&url)
            .multipart(form)
            .send()
            .await
            .wrap_err("failed to upload media to WeCom")?
            .json()
            .await?;

        resp.get("media_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| {
                let errmsg = resp
                    .get("errmsg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                eyre::eyre!("WeCom media upload error: {errmsg}")
            })
    }

    /// Send a message via WeCom REST API.
    async fn send_wecom_message(&self, user_id: &str, body: &serde_json::Value) -> Result<()> {
        let token = self.get_token().await?;
        let url = format!("{WECOM_API}/message/send?access_token={token}");

        let mut payload = body.clone();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("touser".into(), serde_json::json!(user_id));
            obj.insert(
                "agentid".into(),
                serde_json::json!(self.agent_id.parse::<i64>().unwrap_or(0)),
            );
        }

        let resp: serde_json::Value = self
            .http
            .post(&url)
            .json(&payload)
            .send()
            .await
            .wrap_err("failed to send WeCom message")?
            .json()
            .await?;

        let errcode = resp.get("errcode").and_then(|v| v.as_i64()).unwrap_or(-1);
        if errcode != 0 {
            let errmsg = resp
                .get("errmsg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            warn!("WeCom send error: {errmsg} (code {errcode})");
        }

        Ok(())
    }

    /// Check if a message ID has been seen; add if not.
    fn dedup_check(&self, msg_id: &str) -> bool {
        let mut seen = self.seen_ids.lock().unwrap_or_else(|e| e.into_inner());
        if seen.contains(msg_id) {
            return true;
        }
        if seen.len() >= MAX_SEEN_IDS {
            seen.clear();
        }
        seen.insert(msg_id.to_string());
        false
    }

    /// Parse a decrypted WeCom XML message into an InboundMessage.
    async fn parse_message(&self, xml: &str) -> Option<InboundMessage> {
        let msg_type = xml_extract(xml, "MsgType")?;
        let from_user = xml_extract(xml, "FromUserName")?;
        let msg_id = xml_extract(xml, "MsgId").unwrap_or_default();

        if !msg_id.is_empty() && self.dedup_check(&msg_id) {
            debug!(msg_id, "WeCom: dedup filtered message");
            return None;
        }

        if !self.check_allowed(&from_user) {
            debug!(from_user, "WeCom: sender not allowed");
            return None;
        }

        let mut content = String::new();
        let mut media = Vec::new();

        match msg_type.as_str() {
            "text" => {
                content = xml_extract(xml, "Content").unwrap_or_default();
            }
            "image" => {
                if let Some(media_id) = xml_extract(xml, "MediaId") {
                    match self.download_media_file(&media_id, ".png").await {
                        Ok(path) => media.push(path.display().to_string()),
                        Err(e) => warn!("failed to download WeCom image: {e}"),
                    }
                }
            }
            "voice" => {
                if let Some(media_id) = xml_extract(xml, "MediaId") {
                    match self.download_media_file(&media_id, ".amr").await {
                        Ok(path) => media.push(path.display().to_string()),
                        Err(e) => warn!("failed to download WeCom voice: {e}"),
                    }
                }
            }
            "video" => {
                if let Some(media_id) = xml_extract(xml, "MediaId") {
                    match self.download_media_file(&media_id, ".mp4").await {
                        Ok(path) => media.push(path.display().to_string()),
                        Err(e) => warn!("failed to download WeCom video: {e}"),
                    }
                }
            }
            "location" => {
                let label = xml_extract(xml, "Label").unwrap_or_default();
                content = format!("[location: {label}]");
            }
            _ => {
                content = format!("[{msg_type} message]");
            }
        }

        if content.is_empty() && media.is_empty() {
            debug!(msg_type, "WeCom: empty content and media, skipping");
            return None;
        }

        info!(
            msg_id,
            msg_type,
            from_user,
            media_count = media.len(),
            "WeCom: parsed message"
        );

        Some(InboundMessage {
            channel: "wecom".into(),
            sender_id: from_user,
            chat_id: xml_extract(xml, "FromUserName").unwrap_or_default(),
            content,
            timestamp: Utc::now(),
            media,
            metadata: serde_json::json!({
                "wecom": {
                    "msg_id": msg_id,
                    "msg_type": msg_type,
                    "agent_id": xml_extract(xml, "AgentID").unwrap_or_default(),
                }
            }),
            message_id: None,
        })
    }

    /// Run webhook HTTP server.
    async fn start_webhook(&self, inbound_tx: mpsc::Sender<InboundMessage>) -> Result<()> {
        use axum::{
            Router,
            extract::{Query, State},
            response::IntoResponse,
            routing::get,
        };

        #[derive(Clone)]
        struct WebhookState {
            verification_token: String,
            aes_key: [u8; 32],
            corp_id: String,
            inbound_tx: mpsc::Sender<String>,
        }

        #[derive(serde::Deserialize)]
        struct VerifyParams {
            msg_signature: String,
            timestamp: String,
            nonce: String,
            echostr: String,
        }

        #[derive(serde::Deserialize)]
        struct CallbackParams {
            msg_signature: String,
            timestamp: String,
            nonce: String,
        }

        // GET handler for URL verification
        async fn handle_verify(
            State(state): State<WebhookState>,
            Query(params): Query<VerifyParams>,
        ) -> impl IntoResponse {
            let computed = verify_wecom_signature(
                &state.verification_token,
                &params.timestamp,
                &params.nonce,
                &params.echostr,
            );
            if computed != params.msg_signature {
                warn!("WeCom verify: signature mismatch");
                return axum::http::Response::builder()
                    .status(403)
                    .body("signature mismatch".to_string())
                    .unwrap();
            }

            match decrypt_wecom_message(&state.aes_key, &params.echostr) {
                Ok((echostr, _corp_id)) => {
                    info!("WeCom: URL verification succeeded");
                    axum::http::Response::builder()
                        .status(200)
                        .body(echostr)
                        .unwrap()
                }
                Err(e) => {
                    warn!("WeCom verify: decryption failed: {e}");
                    axum::http::Response::builder()
                        .status(400)
                        .body("decryption failed".to_string())
                        .unwrap()
                }
            }
        }

        // POST handler for message callbacks
        async fn handle_callback(
            State(state): State<WebhookState>,
            Query(params): Query<CallbackParams>,
            body: String,
        ) -> impl IntoResponse {
            // Extract Encrypt field from outer XML
            let encrypt_msg = match xml_extract(&body, "Encrypt") {
                Some(e) => e,
                None => {
                    warn!("WeCom callback: no <Encrypt> in body");
                    return axum::http::Response::builder()
                        .status(400)
                        .body("no Encrypt field".to_string())
                        .unwrap();
                }
            };

            // Verify signature
            let computed = verify_wecom_signature(
                &state.verification_token,
                &params.timestamp,
                &params.nonce,
                &encrypt_msg,
            );
            if computed != params.msg_signature {
                warn!("WeCom callback: signature mismatch");
                return axum::http::Response::builder()
                    .status(403)
                    .body("signature mismatch".to_string())
                    .unwrap();
            }

            // Decrypt
            match decrypt_wecom_message(&state.aes_key, &encrypt_msg) {
                Ok((xml_content, ref received_corp_id)) if *received_corp_id == state.corp_id => {
                    let _ = state.inbound_tx.send(xml_content).await;
                    axum::http::Response::builder()
                        .status(200)
                        .body("success".to_string())
                        .unwrap()
                }
                Ok((_, corp_id)) => {
                    warn!(corp_id, "WeCom callback: corp_id mismatch");
                    axum::http::Response::builder()
                        .status(403)
                        .body("corp_id mismatch".to_string())
                        .unwrap()
                }
                Err(e) => {
                    warn!("WeCom callback: decryption failed: {e}");
                    axum::http::Response::builder()
                        .status(400)
                        .body("decryption failed".to_string())
                        .unwrap()
                }
            }
        }

        // Internal channel for passing decrypted XML messages
        let (event_tx, mut event_rx) = mpsc::channel::<String>(100);

        let state = WebhookState {
            verification_token: self.verification_token.clone(),
            aes_key: self.aes_key,
            corp_id: self.corp_id.clone(),
            inbound_tx: event_tx,
        };

        let app = Router::new()
            .route("/wecom/webhook", get(handle_verify).post(handle_callback))
            .with_state(state);

        let addr = format!("0.0.0.0:{}", self.webhook_port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .wrap_err_with(|| format!("failed to bind WeCom webhook server to {addr}"))?;
        info!(port = self.webhook_port, "WeCom webhook server listening");

        let shutdown = self.shutdown.clone();

        // Spawn the HTTP server
        let server_shutdown = shutdown.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    while !server_shutdown.load(Ordering::Relaxed) {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                })
                .await
                .ok();
        });

        // Process incoming decrypted XML messages
        while let Some(xml) = event_rx.recv().await {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }

            if let Some(inbound) = self.parse_message(&xml).await {
                info!(
                    sender = %inbound.sender_id,
                    "WeCom: sending to inbound bus"
                );
                if inbound_tx.send(inbound).await.is_err() {
                    error!("WeCom: inbound_tx send failed (receiver dropped)");
                    break;
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for WeComChannel {
    fn name(&self) -> &str {
        "wecom"
    }

    async fn start(&self, inbound_tx: mpsc::Sender<InboundMessage>) -> Result<()> {
        info!("Starting WeCom channel (webhook mode)");
        self.start_webhook(inbound_tx).await?;
        info!("WeCom channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<()> {
        // Send text content as markdown
        if !msg.content.is_empty() {
            let body = serde_json::json!({
                "msgtype": "markdown",
                "markdown": {
                    "content": msg.content,
                }
            });
            self.send_wecom_message(&msg.chat_id, &body).await?;
        }

        // Send media files
        for path in &msg.media {
            if is_image(path) {
                match self.upload_media(path, "image").await {
                    Ok(media_id) => {
                        let body = serde_json::json!({
                            "msgtype": "image",
                            "image": { "media_id": media_id }
                        });
                        self.send_wecom_message(&msg.chat_id, &body).await?;
                    }
                    Err(e) => warn!("failed to upload WeCom image: {e}"),
                }
            } else {
                match self.upload_media(path, "file").await {
                    Ok(media_id) => {
                        let body = serde_json::json!({
                            "msgtype": "file",
                            "file": { "media_id": media_id }
                        });
                        self.send_wecom_message(&msg.chat_id, &body).await?;
                    }
                    Err(e) => warn!("failed to upload WeCom file: {e}"),
                }
            }
        }

        Ok(())
    }

    fn is_allowed(&self, sender_id: &str) -> bool {
        self.check_allowed(sender_id)
    }

    fn max_message_length(&self) -> usize {
        2048
    }

    async fn stop(&self) -> Result<()> {
        self.shutdown.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel(allowed: Vec<&str>) -> WeComChannel {
        // Use a valid 43-char base64 key for tests
        let test_key = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";
        WeComChannel {
            corp_id: "test_corp".into(),
            agent_id: "1000002".into(),
            agent_secret: "test_secret".into(),
            verification_token: "test_token".into(),
            aes_key: decode_aes_key(test_key).unwrap_or([0u8; 32]),
            allowed_senders: allowed.into_iter().map(String::from).collect(),
            shutdown: Arc::new(AtomicBool::new(false)),
            http: Client::new(),
            media_dir: PathBuf::from("/tmp/test-wecom-media"),
            token_cache: Arc::new(tokio::sync::Mutex::new(None)),
            seen_ids: Arc::new(std::sync::Mutex::new(HashSet::new())),
            webhook_port: 9322,
        }
    }

    #[test]
    fn test_sha1_basic() {
        let hash = sha1(b"");
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn test_sha1_hello() {
        let hash = sha1(b"hello");
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    }

    #[test]
    fn test_verify_wecom_signature() {
        // Signature is SHA1(sorted concat of [token, timestamp, nonce, encrypt_msg])
        let sig = verify_wecom_signature("token123", "1234567890", "nonce456", "encrypted_data");
        assert_eq!(sig.len(), 40); // SHA-1 hex is 40 chars
        // Deterministic
        let sig2 = verify_wecom_signature("token123", "1234567890", "nonce456", "encrypted_data");
        assert_eq!(sig, sig2);
    }

    #[test]
    fn test_verify_signature_sorted() {
        // Verify that sorting works correctly
        let sig1 = verify_wecom_signature("a", "b", "c", "d");
        // All parts sorted alphabetically: a, b, c, d → "abcd"
        let expected = sha1(b"abcd");
        let expected_hex: String = expected.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(sig1, expected_hex);
    }

    #[test]
    fn test_decode_aes_key() {
        // 43-char base64 string + "=" should decode to 32 bytes
        let key = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";
        let result = decode_aes_key(key);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 32);
    }

    #[test]
    fn test_decrypt_encrypt_roundtrip() {
        let key = decode_aes_key("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG").unwrap();
        let plaintext = "Hello WeCom!";
        let corp_id = "test_corp";

        let encrypted = encrypt_wecom_message(&key, plaintext, corp_id).unwrap();
        let (decrypted, dec_corp) = decrypt_wecom_message(&key, &encrypted).unwrap();

        assert_eq!(decrypted, plaintext);
        assert_eq!(dec_corp, corp_id);
    }

    #[test]
    fn test_xml_extract_simple() {
        let xml = "<xml><Content>Hello World</Content><MsgType>text</MsgType></xml>";
        assert_eq!(xml_extract(xml, "Content"), Some("Hello World".into()));
        assert_eq!(xml_extract(xml, "MsgType"), Some("text".into()));
        assert_eq!(xml_extract(xml, "Missing"), None);
    }

    #[test]
    fn test_xml_extract_cdata() {
        let xml = "<xml><Content><![CDATA[Hello & World]]></Content></xml>";
        assert_eq!(xml_extract(xml, "Content"), Some("Hello & World".into()));
    }

    #[test]
    fn test_xml_extract_image() {
        let xml = r#"<xml>
            <MsgType><![CDATA[image]]></MsgType>
            <MediaId><![CDATA[media_abc123]]></MediaId>
            <PicUrl><![CDATA[http://example.com/pic.jpg]]></PicUrl>
        </xml>"#;
        assert_eq!(xml_extract(xml, "MsgType"), Some("image".into()));
        assert_eq!(xml_extract(xml, "MediaId"), Some("media_abc123".into()));
    }

    #[test]
    fn test_dedup() {
        let ch = make_channel(vec![]);
        assert!(!ch.dedup_check("msg1"));
        assert!(ch.dedup_check("msg1")); // duplicate
        assert!(!ch.dedup_check("msg2"));
    }

    #[test]
    fn test_dedup_overflow_clears() {
        let ch = make_channel(vec![]);
        for i in 0..MAX_SEEN_IDS {
            ch.dedup_check(&format!("msg_{i}"));
        }
        assert!(!ch.dedup_check("new_msg"));
        assert!(!ch.dedup_check("msg_0"));
    }

    #[test]
    fn test_is_allowed_empty_list() {
        let ch = make_channel(vec![]);
        assert!(ch.is_allowed("anyone"));
    }

    #[test]
    fn test_is_allowed_matching() {
        let ch = make_channel(vec!["user1", "user2"]);
        assert!(ch.is_allowed("user1"));
        assert!(!ch.is_allowed("user3"));
    }

    #[test]
    fn test_with_webhook_port() {
        let ch = WeComChannel::new(
            "corp",
            "1000002",
            "secret",
            "token",
            "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG",
            vec![],
            Arc::new(AtomicBool::new(false)),
            PathBuf::from("/tmp"),
        )
        .with_webhook_port(8080);
        assert_eq!(ch.webhook_port, 8080);
    }

    #[test]
    fn test_base64_roundtrip() {
        let data = b"hello world from wecom";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_channel_name() {
        let ch = make_channel(vec![]);
        assert_eq!(ch.name(), "wecom");
    }

    #[test]
    fn test_max_message_length() {
        let ch = make_channel(vec![]);
        assert_eq!(ch.max_message_length(), 2048);
    }
}
