//! URL normalization, article basename slugging, and content-hash
//! comparison for article deduplication (task_40, ADR-030, ADR-031).
//!
//! The Python twin lives in `skills/_lib/urlnorm.py`; both sides are
//! checked against the shared fixture `skills/_lib/fixtures/urlnorm.json`
//! so the courier and the daemon agree byte for byte on what "the same
//! URL" means. The rule:
//!
//! - lowercase the scheme and host;
//! - strip query parameters named `utm_*` (prefix), `fbclid`, `gclid`,
//!   `mc_cid`, `mc_eid`, keeping the rest in their original order;
//! - drop the fragment;
//! - collapse a trailing slash on a non-root path (a lone `/` stays);
//! - leave path case and the remaining query untouched.
//!
//! The article's entity id is opaque and minted once (ADR-030); the
//! `<publication>-<date>-<title>` slug is a projection basename and a
//! resolver blocking key, never the identity.

use crate::multibase::decode_base58btc;

/// Query parameter names dropped by [`normalize_url`].
const STRIPPED_PARAMS: &[&str] = &["fbclid", "gclid", "mc_cid", "mc_eid"];

fn is_tracking_param(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("utm_") || STRIPPED_PARAMS.contains(&lower.as_str())
}

/// Decode a base64url (or standard base64) string without a dependency.
/// Returns `None` on any non-alphabet byte or bad length.
fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Known tracking-link wrappers: the real URL is base64url in a path
/// segment. The courier's table is extensible in `courier.toml`; the
/// daemon carries the default entry so both sides agree on the fixture.
fn unwrap_tracking_link(scheme: &str, host: &str, path: &str) -> Option<String> {
    if host != "link.bizjournals.com" {
        return None;
    }
    let _ = scheme;
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let seg = segments.get(2)?;
    let bytes = base64url_decode(seg)?;
    let decoded = String::from_utf8(bytes).ok()?;
    if decoded.starts_with("http://") || decoded.starts_with("https://") {
        Some(decoded)
    } else {
        None
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Python `urllib.parse.quote_plus` semantics: letters, digits, `_.-~`
/// kept; space becomes `+`; everything else `%XX` uppercase.
fn quote_plus(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Normalize a URL for dedup comparison. Idempotent: normalizing twice
/// equals normalizing once. Strings that do not look like URLs (no
/// scheme) are trimmed and returned otherwise unchanged. Known
/// tracking wrappers are decoded first, then the result is normalized.
pub fn normalize_url(raw: &str) -> String {
    let s = raw.trim();
    // Split off the fragment first: it never survives.
    let s = s.split('#').next().unwrap_or("");
    let Some((scheme, rest)) = s.split_once("://") else {
        return s.to_string();
    };
    let scheme = scheme.to_ascii_lowercase();
    let (authority_and_path, query) = match rest.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (rest, None),
    };
    let (authority, path) = match authority_and_path.find('/') {
        Some(i) => (&authority_and_path[..i], &authority_and_path[i..]),
        None => (authority_and_path, ""),
    };
    let authority = authority.to_ascii_lowercase();
    // Strip the scheme's default port.
    let authority = match (scheme.as_str(), authority.rsplit_once(':')) {
        ("https", Some((h, "443"))) | ("http", Some((h, "80"))) => h.to_string(),
        _ => authority,
    };
    if let Some(inner) = unwrap_tracking_link(&scheme, &authority, path) {
        return normalize_url(&inner);
    }
    let path = if path.len() > 1 && path.ends_with('/') {
        path.trim_end_matches('/')
    } else {
        path
    };
    let path = if path.is_empty() { "/" } else { path };
    let mut out = format!("{scheme}://{authority}{path}");
    if let Some(q) = query {
        let kept: Vec<String> = q
            .split('&')
            .filter(|kv| !kv.is_empty())
            .filter_map(|kv| {
                let (name, value) = match kv.split_once('=') {
                    Some((n, v)) => (n, Some(v)),
                    None => (kv, None),
                };
                if is_tracking_param(name) {
                    return None;
                }
                let name = quote_plus(&percent_decode(name));
                Some(match value {
                    Some(v) => format!("{name}={}", quote_plus(&percent_decode(v))),
                    None => name,
                })
            })
            .collect();
        if !kept.is_empty() {
            out.push('?');
            out.push_str(&kept.join("&"));
        }
    }
    out
}

/// Slug rule shared with the courier: lowercase, runs of
/// non-alphanumerics to single hyphens, trimmed, at most 80 chars.
pub fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_hyphen = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if pending_hyphen && !out.is_empty() {
                out.push('-');
            }
            pending_hyphen = false;
            for lc in c.to_lowercase() {
                out.push(lc);
            }
        } else {
            pending_hyphen = true;
        }
    }
    if out.len() > 80 {
        let mut cut = 80;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

/// `<publication-slug>-<date>-<title-slug>`: the article's projection
/// basename and resolver blocking key (never its entity id).
pub fn article_basename(publication: &str, date: &str, title: &str) -> String {
    let parts: Vec<String> = [slug(publication), slug(date), slug(title)]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    let joined = parts.join("-");
    if joined.len() > 80 {
        let mut cut = 80;
        while !joined.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut t = joined[..cut].to_string();
        while t.ends_with('-') {
            t.pop();
        }
        t
    } else {
        joined
    }
}

/// Multihash prefixes accepted for `content_hash`: blake3-256
/// (`0x1e 0x20`) and blake2b-256 (varint `0xb220` = `0xa0 0xe4 0x02`,
/// then length `0x20`).
const BLAKE3_256_PREFIX: &[u8] = &[0x1e, 0x20];
const BLAKE2B_256_PREFIX: &[u8] = &[0xa0, 0xe4, 0x02, 0x20];

/// True when `s` is a base58btc multibase string whose decoded bytes
/// start with an accepted multihash prefix.
pub fn is_supported_content_hash(s: &str) -> bool {
    match decode_base58btc(s.trim()) {
        Ok(bytes) => bytes.starts_with(BLAKE3_256_PREFIX) || bytes.starts_with(BLAKE2B_256_PREFIX),
        Err(_) => false,
    }
}

/// Two `content_hash` values name the same page bytes when both are
/// supported multihashes and the full strings are equal. Unsupported
/// or malformed values never match anything.
pub fn same_content_hash(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    a == b && is_supported_content_hash(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn strips_tracking_params_fragment_and_trailing_slash_and_lowercases_host() {
        let raw = "HTTPS://WWW.Example.com/News/Story/?utm_source=x&id=7&fbclid=abc#top";
        assert_eq!(
            normalize_url(raw),
            "https://www.example.com/News/Story?id=7"
        );
    }

    #[test]
    fn normalize_url_is_idempotent() {
        let raw = "https://Example.com/a/b/?gclid=1&q=2&utm_medium=m#frag";
        let once = normalize_url(raw);
        assert_eq!(normalize_url(&once), once);
    }

    #[test]
    fn lone_root_slash_survives_and_empty_query_is_dropped() {
        assert_eq!(normalize_url("https://a.com/"), "https://a.com/");
        assert_eq!(normalize_url("https://a.com/?utm_x=1"), "https://a.com/");
        assert_eq!(normalize_url("https://a.com"), "https://a.com/");
        assert_eq!(normalize_url("https://A.com:443/x"), "https://a.com/x");
        assert_eq!(normalize_url("http://a.com:8080/x"), "http://a.com:8080/x");
        assert_eq!(
            normalize_url("https://a.com/x?q=a%20b"),
            "https://a.com/x?q=a+b"
        );
    }

    #[test]
    fn article_basename_slug_is_deterministic_and_not_an_id() {
        let a = article_basename("Example Ledger", "2026-09-10", "Riverside Mill to reopen!");
        let b = article_basename("Example Ledger", "2026-09-10", "Riverside Mill to reopen!");
        assert_eq!(a, b);
        assert_eq!(a, "example-ledger-2026-09-10-riverside-mill-to-reopen");
        assert_ne!(
            a,
            article_basename("Example Ledger", "2026-09-11", "Riverside Mill to reopen!")
        );
        assert!(article_basename("p", "d", &"x".repeat(200)).len() <= 80);
        // Not multibase: the slug is a basename, never an entity id.
        assert!(decode_base58btc(&a).is_err());
    }

    #[test]
    fn content_hash_accepts_blake3_and_blake2b_prefixes() {
        use crate::multibase::encode_base58btc;
        let mut b3 = vec![0x1e, 0x20];
        b3.extend([7u8; 32]);
        let mut b2 = vec![0xa0, 0xe4, 0x02, 0x20];
        b2.extend([9u8; 32]);
        let h3 = encode_base58btc(&b3);
        let h2 = encode_base58btc(&b2);
        assert!(is_supported_content_hash(&h3));
        assert!(is_supported_content_hash(&h2));
        assert!(same_content_hash(&h3, &h3));
        assert!(!same_content_hash(&h3, &h2));
        let sha = encode_base58btc(&[0x12, 0x20, 1, 2, 3]);
        assert!(!is_supported_content_hash(&sha));
        assert!(!same_content_hash(&sha, &sha));
        assert!(!same_content_hash("nonsense", "nonsense"));
    }

    #[test]
    fn url_normalization_matches_shared_fixtures() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/_lib/fixtures/urlnorm.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!(
                "skipping: shared fixture {} not present yet (written by the courier bundle)",
                path.display()
            );
            return;
        };
        let doc: serde_json::Value = serde_json::from_str(&text).expect("fixture json");
        let pairs: Vec<serde_json::Value> = match &doc {
            serde_json::Value::Array(a) => a.clone(),
            serde_json::Value::Object(o) => o
                .get("pairs")
                .and_then(|p| p.as_array())
                .cloned()
                .expect("fixture object has a pairs array"),
            _ => panic!("unexpected fixture shape"),
        };
        assert!(!pairs.is_empty(), "fixture must not be empty");
        for pair in pairs {
            let raw = pair["raw"].as_str().expect("raw");
            let expected = pair["normalized"].as_str().expect("normalized");
            assert_eq!(normalize_url(raw), expected, "raw={raw}");
        }
    }
}
