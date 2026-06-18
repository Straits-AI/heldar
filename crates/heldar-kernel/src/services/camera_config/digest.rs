//! HTTP Digest access authentication (RFC 2617) — hand-rolled for HikVision ISAPI.
//!
//! The flow lives in the service layer: send a request unauthenticated, and on `401` read the
//! `WWW-Authenticate` header, build an `Authorization: Digest ...` header with
//! [`digest_auth_header`], and retry once. The MD5 hashing uses the `md-5` crate; the client nonce
//! is drawn from the OS CSPRNG (mirroring `auth.rs` / `services/onvif.rs`).

use std::collections::HashMap;

use argon2::password_hash::rand_core::OsRng;
use md5::{Digest, Md5};
use rand_core::RngCore;

/// Lowercase hex MD5 of `data`.
fn md5_hex(data: &str) -> String {
    let mut h = Md5::new();
    h.update(data.as_bytes());
    crate::auth::hex_encode(&h.finalize())
}

/// Parse a `WWW-Authenticate: Digest ...` challenge into its `key="value"` (or `key=value`) params.
/// Keys are lowercased; surrounding quotes are stripped. Commas inside quoted values (e.g.
/// `qop="auth,auth-int"`) are respected.
fn parse_challenge(header: &str) -> HashMap<String, String> {
    let s = header.trim();
    let s = s
        .strip_prefix("Digest")
        .or_else(|| s.strip_prefix("digest"))
        .unwrap_or(s)
        .trim_start();

    let mut params = HashMap::new();
    let mut start = 0;
    let mut in_quotes = false;
    let bytes = s.as_bytes();
    let push = |chunk: &str, params: &mut HashMap<String, String>| {
        let chunk = chunk.trim();
        if let Some(eq) = chunk.find('=') {
            let key = chunk[..eq].trim().to_ascii_lowercase();
            let mut val = chunk[eq + 1..].trim();
            if val.len() >= 2 && val.starts_with('"') && val.ends_with('"') {
                val = &val[1..val.len() - 1];
            }
            if !key.is_empty() {
                params.insert(key, val.to_string());
            }
        }
    };
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b',' if !in_quotes => {
                push(&s[start..i], &mut params);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&s[start..], &mut params);
    params
}

/// Escape a value for an RFC 7616 §3.4 `quoted-string` parameter: each backslash and double-quote
/// is prefixed with a backslash (`\` -> `\\`, `"` -> `\"`). All other characters pass through.
/// This keeps the header well-formed when a credential/server field contains `"` or `\`.
fn quote_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Build the `Authorization` header value from the parsed challenge fields and a chosen client nonce.
#[allow(clippy::too_many_arguments)]
fn build_header(
    method: &str,
    uri: &str,
    username: &str,
    password: &str,
    realm: &str,
    nonce: &str,
    qop: Option<&str>,
    opaque: Option<&str>,
    cnonce: &str,
    nc: &str,
) -> String {
    let ha1 = md5_hex(&format!("{username}:{realm}:{password}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    // RFC 7616 §3.4: values emitted as `quoted-string` params must have `\` and `"` escaped. The
    // digest above is computed over the *unescaped* values; only the rendered header is escaped.
    // `response`/`nc`/`qop` are hex/tokens and are emitted verbatim.
    let username_q = quote_escape(username);
    let realm_q = quote_escape(realm);
    let nonce_q = quote_escape(nonce);
    let uri_q = quote_escape(uri);
    let cnonce_q = quote_escape(cnonce);
    let mut header = match qop {
        Some(qop) => {
            let response = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"));
            format!(
                "Digest username=\"{username_q}\", realm=\"{realm_q}\", nonce=\"{nonce_q}\", \
uri=\"{uri_q}\", response=\"{response}\", qop={qop}, nc={nc}, cnonce=\"{cnonce_q}\""
            )
        }
        None => {
            let response = md5_hex(&format!("{ha1}:{nonce}:{ha2}"));
            format!(
                "Digest username=\"{username_q}\", realm=\"{realm_q}\", nonce=\"{nonce_q}\", \
uri=\"{uri_q}\", response=\"{response}\""
            )
        }
    };
    if let Some(opaque) = opaque {
        let opaque_q = quote_escape(opaque);
        header.push_str(&format!(", opaque=\"{opaque_q}\""));
    }
    header
}

/// Compute an HTTP Digest `Authorization` header for `method`+`uri` given a `WWW-Authenticate`
/// challenge. Returns `None` when the challenge lacks the required `realm`/`nonce`. When the server
/// offers `qop=auth`, a fresh client nonce (`cnonce`) is generated and `nc` is `00000001`.
pub fn digest_auth_header(
    method: &str,
    uri: &str,
    username: &str,
    password: &str,
    www_auth: &str,
) -> Option<String> {
    let challenge = parse_challenge(www_auth);
    let realm = challenge.get("realm")?;
    let nonce = challenge.get("nonce")?;
    let opaque = challenge.get("opaque").map(String::as_str);
    // Select the `auth` qop if the server offers it (the list may also include `auth-int`).
    let qop = challenge
        .get("qop")
        .and_then(|q| q.split(',').map(str::trim).find(|t| *t == "auth"));

    let (cnonce, nc) = if qop.is_some() {
        let mut buf = [0u8; 8];
        OsRng.fill_bytes(&mut buf);
        (crate::auth::hex_encode(&buf), "00000001")
    } else {
        (String::new(), "")
    };

    Some(build_header(
        method, uri, username, password, realm, nonce, qop, opaque, &cnonce, nc,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_rfc2617_qop_auth_response() {
        // Canonical RFC 2617 §3.5 example (fixed cnonce/nc so the response is deterministic).
        let header = build_header(
            "GET",
            "/dir/index.html",
            "Mufasa",
            "Circle Of Life",
            "testrealm@host.com",
            "dcd98b7102dd2f0e8b11d0f600bfb0c093",
            Some("auth"),
            None,
            "0a4f113b",
            "00000001",
        );
        assert!(header.contains("response=\"6629fae49393a05397450978507c4ef1\""));
        assert!(header.contains("qop=auth"));
        assert!(header.contains("cnonce=\"0a4f113b\""));
        assert!(header.contains("uri=\"/dir/index.html\""));
    }

    #[test]
    fn parses_challenge_with_quoted_qop_list() {
        let c = parse_challenge(
            "Digest realm=\"DS-2CD\", qop=\"auth,auth-int\", nonce=\"abc123\", opaque=\"xyz\"",
        );
        assert_eq!(c.get("realm").map(String::as_str), Some("DS-2CD"));
        assert_eq!(c.get("nonce").map(String::as_str), Some("abc123"));
        assert_eq!(c.get("qop").map(String::as_str), Some("auth,auth-int"));
        assert_eq!(c.get("opaque").map(String::as_str), Some("xyz"));
    }

    #[test]
    fn selects_auth_qop_and_emits_client_nonce() {
        let h = digest_auth_header(
            "GET",
            "/ISAPI/System/deviceInfo",
            "admin",
            "secret",
            "Digest realm=\"r\", nonce=\"n\", qop=\"auth\"",
        )
        .expect("header");
        assert!(h.contains("qop=auth"));
        assert!(h.contains("nc=00000001"));
        assert!(h.contains("cnonce="));
        assert!(h.contains("uri=\"/ISAPI/System/deviceInfo\""));
    }

    #[test]
    fn legacy_no_qop_response() {
        let h =
            digest_auth_header("GET", "/x", "u", "p", "Digest realm=\"r\", nonce=\"n\"").unwrap();
        assert!(h.contains("response=\""));
        assert!(!h.contains("qop="));
        assert!(!h.contains("cnonce="));
    }

    #[test]
    fn missing_realm_or_nonce_yields_none() {
        assert!(digest_auth_header("GET", "/x", "u", "p", "Digest nonce=\"n\"").is_none());
        assert!(digest_auth_header("GET", "/x", "u", "p", "Digest realm=\"r\"").is_none());
    }

    #[test]
    fn escapes_quote_and_backslash_in_quoted_string_params() {
        // A username containing a double-quote must be backslash-escaped so the `quoted-string`
        // stays well-formed (RFC 7616 §3.4); the bare `user"x` would prematurely close the value.
        let header = build_header(
            "GET",
            "/x",
            "user\"x",
            "p",
            "r",
            "n",
            Some("auth"),
            None,
            "0a4f113b",
            "00000001",
        );
        assert!(
            header.contains("username=\"user\\\"x\""),
            "double-quote in username must be escaped: {header}"
        );

        // A backslash is doubled (legacy no-qop branch).
        let header2 = build_header("GET", "/x", "ab\\cd", "p", "r", "n", None, None, "", "");
        assert!(
            header2.contains("username=\"ab\\\\cd\""),
            "backslash in username must be doubled: {header2}"
        );

        // The helper itself escapes both specials and passes other characters through unchanged.
        assert_eq!(quote_escape("a\"b\\c"), "a\\\"b\\\\c");
        assert_eq!(quote_escape("plain"), "plain");
    }
}
