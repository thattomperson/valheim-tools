//! Server password hashing, as done in `ZNet.HashPassword`.
//!
//! `Encoding.ASCII.GetString(MD5(Encoding.ASCII.GetBytes(password + salt)))`.
//! .NET's ASCII codec replaces anything outside 0x00..=0x7F with `?`, both
//! when encoding the input (per UTF-16 code unit) and when decoding the
//! digest, so the result is always 16 ASCII characters.

use md5::{Digest, Md5};

fn dotnet_ascii_bytes(s: &str) -> Vec<u8> {
    s.encode_utf16().map(|u| if u < 0x80 { u as u8 } else { b'?' }).collect()
}

/// Returns the 16-character password hash the server expects in `PeerInfo`.
pub fn hash_password(password: &str, salt: &str) -> String {
    let mut input = dotnet_ascii_bytes(password);
    input.extend(dotnet_ascii_bytes(salt));
    let digest = Md5::digest(&input);
    digest.iter().map(|&b| if b < 0x80 { b as char } else { '?' }).collect()
}

#[cfg(test)]
mod tests {
    use super::hash_password;

    /// Reference values computed by running the original C# under .NET 8.
    #[test]
    fn matches_dotnet() {
        let salt = "ab\u{1}?cd\u{7f}??xyz0123";
        for (pw, want) in [
            ("hunter2", "3F-3F-58-3F-3F-3F-7E-05-79-3F-3F-06-3F-3F-3B-31"),
            ("pässwörd", "3E-49-3F-65-3F-3F-3F-0B-45-3F-58-75-3F-33-3F-3F"),
            ("😀", "3F-2A-3F-3B-5E-3F-31-3F-3F-67-3F-12-3F-3F-3B-63"),
        ] {
            let got = hash_password(pw, salt);
            let hex: Vec<String> = got.bytes().map(|b| format!("{b:02X}")).collect();
            assert_eq!(hex.join("-"), want, "{pw:?}");
        }
    }
}
