use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha1 = Hmac<Sha1>;

const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const DEFAULT_STEP_SECONDS: i64 = 30;
const DEFAULT_DIGITS: u32 = 6;

pub fn generate_secret() -> String {
    let mut bytes = [0_u8; 20];
    rand::thread_rng().fill_bytes(&mut bytes);
    encode_base32(&bytes)
}

pub fn current_code(secret: &str) -> Option<String> {
    code_at(
        secret,
        current_unix_time(),
        DEFAULT_STEP_SECONDS,
        DEFAULT_DIGITS,
    )
}

pub fn verify_code(secret: &str, code: &str) -> bool {
    verify_code_at(
        secret,
        code,
        current_unix_time(),
        DEFAULT_STEP_SECONDS,
        DEFAULT_DIGITS,
        1,
    )
}

pub fn code_at(secret: &str, unix_time: i64, step_seconds: i64, digits: u32) -> Option<String> {
    if step_seconds <= 0 || digits == 0 || digits > 9 {
        return None;
    }
    let key = decode_base32(secret)?;
    let counter = unix_time.div_euclid(step_seconds) as u64;
    hotp(&key, counter, digits)
}

pub fn verify_code_at(
    secret: &str,
    code: &str,
    unix_time: i64,
    step_seconds: i64,
    digits: u32,
    window: i64,
) -> bool {
    let code = code.trim();
    if code.len() != digits as usize || !code.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    for offset in -window..=window {
        let candidate_time = unix_time.saturating_add(offset.saturating_mul(step_seconds));
        if code_at(secret, candidate_time, step_seconds, digits).as_deref() == Some(code) {
            return true;
        }
    }
    false
}

fn hotp(key: &[u8], counter: u64, digits: u32) -> Option<String> {
    let mut mac = HmacSha1::new_from_slice(key).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = ((u32::from(digest[offset]) & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    let modulo = 10_u32.checked_pow(digits)?;
    Some(format!(
        "{:0width$}",
        binary % modulo,
        width = digits as usize
    ))
}

fn encode_base32(bytes: &[u8]) -> String {
    let mut output = String::new();
    let mut buffer = 0_u16;
    let mut bits_left = 0_u8;
    for byte in bytes {
        buffer = (buffer << 8) | u16::from(*byte);
        bits_left += 8;
        while bits_left >= 5 {
            let index = ((buffer >> (bits_left - 5)) & 0x1f) as usize;
            output.push(BASE32_ALPHABET[index] as char);
            bits_left -= 5;
        }
    }
    if bits_left > 0 {
        let index = ((buffer << (5 - bits_left)) & 0x1f) as usize;
        output.push(BASE32_ALPHABET[index] as char);
    }
    output
}

fn decode_base32(value: &str) -> Option<Vec<u8>> {
    let mut buffer = 0_u32;
    let mut bits_left = 0_u8;
    let mut bytes = Vec::new();
    for ch in value.chars() {
        let ch = ch.to_ascii_uppercase();
        if ch == '=' || ch.is_whitespace() {
            continue;
        }
        let value = match ch {
            'A'..='Z' => ch as u8 - b'A',
            '2'..='7' => ch as u8 - b'2' + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | u32::from(value);
        bits_left += 5;
        if bits_left >= 8 {
            bytes.push(((buffer >> (bits_left - 8)) & 0xff) as u8);
            bits_left -= 8;
        }
    }
    if bytes.is_empty() {
        return None;
    }
    Some(bytes)
}

fn current_unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_matches_rfc6238_sha1_vectors() {
        let secret = encode_base32(b"12345678901234567890");
        assert_eq!(code_at(&secret, 59, 30, 8).unwrap(), "94287082");
        assert_eq!(code_at(&secret, 1_111_111_109, 30, 8).unwrap(), "07081804");
        assert_eq!(code_at(&secret, 2_000_000_000, 30, 8).unwrap(), "69279037");
    }

    #[test]
    fn verify_code_accepts_adjacent_window_and_rejects_bad_codes() {
        let secret = "JBSWY3DPEHPK3PXP";
        let code = code_at(secret, 1_700_000_000, 30, 6).unwrap();
        assert!(verify_code_at(secret, &code, 1_700_000_030, 30, 6, 1));
        assert!(!verify_code_at(secret, "12345", 1_700_000_000, 30, 6, 1));
        assert!(!verify_code_at(secret, "abcdef", 1_700_000_000, 30, 6, 1));
    }

    #[test]
    fn generated_secret_round_trips() {
        let secret = generate_secret();
        assert!(decode_base32(&secret).unwrap().len() >= 16);
        assert!(current_code(&secret).is_some());
    }
}
