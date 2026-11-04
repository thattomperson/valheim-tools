//! Valheim's `string.GetStableHashCode()`, used to name RPC methods.

/// Port of `StringExtensionMethods.GetStableHashCode`. It walks the string as
/// UTF-16 code units, two at a time, stopping at the first NUL.
pub fn stable_hash(s: &str) -> i32 {
    let c: Vec<u16> = s.encode_utf16().collect();
    let (mut a, mut b) = (5381i32, 5381i32);
    let mut i = 0;
    while i < c.len() && c[i] != 0 {
        a = (a.wrapping_shl(5).wrapping_add(a)) ^ c[i] as i32;
        if i == c.len() - 1 || c[i + 1] == 0 {
            break;
        }
        b = (b.wrapping_shl(5).wrapping_add(b)) ^ c[i + 1] as i32;
        i += 2;
    }
    a.wrapping_add(b.wrapping_mul(1566083941))
}

#[cfg(test)]
mod tests {
    use super::stable_hash;

    /// Reference values computed by running the original C# under .NET 8.
    #[test]
    fn matches_dotnet() {
        for (s, want) in [
            ("ChatMessage", -1182660091),
            ("PeerInfo", -725574882),
            ("ServerHandshake", 1233642074),
            ("ClientHandshake", 1021693670),
            ("RoutedRPC", -667652280),
            ("Error", 22442200),
            ("", 371857150),
            ("a", 372029373),
            ("ab", 1093630535),
            ("é漢😀", 2071414164),
        ] {
            assert_eq!(stable_hash(s), want, "{s:?}");
        }
    }
}
