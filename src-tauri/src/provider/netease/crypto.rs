//! 网易云 weapi 加密（§4.3.1）。
//!
//! **必须精确实现，否则接口返回 `{"code":400}`。** 链路：
//!
//! ```text
//! 1. secretKey = 16 位随机字符（[0-9a-zA-Z]）
//! 2. encSecKey  = RSA(secretKey 反转 → ASCII bytes → hex → 大整数)
//!                 大整数^0x10001 mod MODULUS → 十六进制左补零至 256 位
//! 3. params     = Base64( AES-128-CBC( AES-128-CBC(json, key=NONCE),
//!                                     key=secretKey ) )
//!    IV 固定 = "0102030405060708"，PKCS7 填充
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use aes::Aes128;
use base64::Engine;
use num_bigint::BigUint;
use serde_json::Value;

type Aes128CbcEnc = cbc::Encryptor<Aes128>;

const MODULUS: &str = "00e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b725152b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbda92557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe4875d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7";
/// 第一层 AES 的固定密钥
const NONCE: &str = "0CoJUm6Qyw8W8jud";
const PUBKEY: &str = "010001";
const IV: &str = "0102030405060708";

const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// 生成一个 16 位的 secretKey。
///
/// 不做密码学随机：这个值只用于本次请求的对称加密，
/// 服务端不校验其随机性，只要求两次 AES 用的是同一个 key。
/// 但**必须每次都不同**——固定值会让请求之间产生可关联的指纹。
fn random_secret_key() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut hasher = blake3::Hasher::new();
    hasher.update(&nanos.to_le_bytes());
    hasher.update(&seq.to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    // 叠加一个栈地址，进一步降低同纳秒内的碰撞概率
    let addr = &seq as *const _ as usize;
    hasher.update(&addr.to_le_bytes());
    let digest = hasher.finalize();

    (0..16)
        .map(|i| CHARS[digest.as_bytes()[i] as usize % CHARS.len()] as char)
        .collect()
}

fn aes128_cbc_b64(plain: &[u8], key: &[u8], iv: &[u8]) -> String {
    let mut buf = plain.to_vec();
    buf.resize(buf.len() + 16, 0);
    let ct = Aes128CbcEnc::new(key.into(), iv.into())
        .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
        .expect("AES-128-CBC 加密失败");
    base64::engine::general_purpose::STANDARD.encode(ct)
}

/// RSA 公钥加密：把 secretKey 反转后当作大整数做模幂。
fn rsa_encode(secret_key: &str) -> String {
    let reversed: String = secret_key.chars().rev().collect();
    let hex: String = reversed.bytes().map(|b| format!("{b:02x}")).collect();
    let a = BigUint::parse_bytes(hex.as_bytes(), 16).unwrap_or_default();
    let e = BigUint::parse_bytes(PUBKEY.as_bytes(), 16).expect("公钥指数");
    let n = BigUint::parse_bytes(MODULUS.as_bytes(), 16).expect("模数");
    let c = a.modpow(&e, &n);
    format!("{:0>256}", format!("{c:x}"))
}

/// 把明文载荷加密成 weapi 表单参数。
pub fn weapi_form(payload: &Value) -> Vec<(&'static str, String)> {
    let key = random_secret_key();
    let enc_sec_key = rsa_encode(&key);
    let json = serde_json::to_string(payload).unwrap_or_else(|_| "{}".into());
    let once = aes128_cbc_b64(json.as_bytes(), NONCE.as_bytes(), IV.as_bytes());
    let params = aes128_cbc_b64(once.as_bytes(), key.as_bytes(), IV.as_bytes());
    vec![("params", params), ("encSecKey", enc_sec_key)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RSA 结果必须恰好 256 个十六进制字符——少一位服务端就解不出来
    #[test]
    fn rsa_output_is_padded_to_256_hex_chars() {
        for _ in 0..8 {
            let key = random_secret_key();
            assert_eq!(key.len(), 16);
            let enc = rsa_encode(&key);
            assert_eq!(enc.len(), 256, "{enc}");
            assert!(enc.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    /// 每次请求的 secretKey 必须不同，否则请求之间可被关联
    #[test]
    fn secret_keys_do_not_repeat() {
        let a = random_secret_key();
        let b = random_secret_key();
        let c = random_secret_key();
        assert!(a != b && b != c && a != c, "{a} {b} {c}");
    }

    #[test]
    fn form_has_both_required_fields() {
        let form = weapi_form(&serde_json::json!({"s":"夜曲 周杰伦","type":1}));
        let keys: Vec<&str> = form.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec!["params", "encSecKey"]);
        assert!(form.iter().all(|(_, v)| !v.is_empty()));
    }

    /// 加密必须可复现：同样的 key 与明文产出同样的密文
    #[test]
    fn aes_layer_is_deterministic_given_key() {
        let a = aes128_cbc_b64(b"hello", NONCE.as_bytes(), IV.as_bytes());
        let b = aes128_cbc_b64(b"hello", NONCE.as_bytes(), IV.as_bytes());
        assert_eq!(a, b);
        assert!(a.len() > 8);
    }
}
