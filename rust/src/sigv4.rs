//! AWS Signature Version 4 for S3, header style.
//!
//! Signed headers are `host`, `x-amz-content-sha256`, `x-amz-date` and, when
//! a session token is present, `x-amz-security-token`. The payload hash is
//! always the real SHA-256 of the body: never `UNSIGNED-PAYLOAD`, never
//! `aws-chunked` (some stores, Garage among them, reject the chunked trailer).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

/// SHA-256 of the empty string, the payload hash of GET, HEAD and DELETE.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any size");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

/// Percent-encode the way AWS canonicalises: unreserved characters
/// (`A-Z a-z 0-9 - _ . ~`) are kept, everything else becomes `%XX` with
/// upper-case hex. `/` is kept only when `keep_slash` is true (object keys
/// in the path); in query strings it is encoded.
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Canonical query string: encoded pairs sorted by name, then by value.
pub fn canonical_query(params: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Access key, secret key and optional session token.
#[derive(Clone)]
pub struct Credentials {
    /// Access key id (`AKIA…` on AWS, `GK…` on Garage, …).
    pub access_key: String,
    /// Secret key. Never printed by `Debug`.
    pub secret_key: String,
    /// Session token of temporary credentials, if any.
    pub session_token: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key", &self.access_key)
            .field("secret_key", &"***")
            .field("session_token", &self.session_token.as_ref().map(|_| "***"))
            .finish()
    }
}

/// What the signer needs to know about a request.
pub struct SignRequest<'a> {
    /// HTTP method, upper case.
    pub method: &'a str,
    /// Value of the `Host` header the HTTP client will send (`host[:port]`).
    pub host: &'a str,
    /// Canonical path, already percent-encoded, starting with `/`.
    pub path: &'a str,
    /// Raw (unencoded) query parameters.
    pub query: &'a [(String, String)],
    /// Hex SHA-256 of the payload ([`EMPTY_SHA256`] when there is none).
    pub payload_hash: &'a str,
    /// Timestamp `YYYYMMDDTHHMMSSZ`.
    pub amz_date: &'a str,
    /// Extra headers to sign, lower-case names (rarely needed).
    pub extra_headers: &'a [(String, String)],
}

/// Signs the request and returns the headers to add to it: `x-amz-date`,
/// `x-amz-content-sha256`, `x-amz-security-token` (when present) and
/// `authorization`, plus the `extra_headers` given.
pub fn sign(req: &SignRequest, creds: &Credentials, region: &str) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = vec![
        ("host".into(), req.host.to_string()),
        ("x-amz-content-sha256".into(), req.payload_hash.to_string()),
        ("x-amz-date".into(), req.amz_date.to_string()),
    ];
    if let Some(token) = &creds.session_token {
        headers.push(("x-amz-security-token".into(), token.clone()));
    }
    for (k, v) in req.extra_headers {
        headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
    }
    headers.sort();

    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method,
        req.path,
        canonical_query(req.query),
        canonical_headers,
        signed_headers,
        req.payload_hash
    );

    let date = &req.amz_date[..8];
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        req.amz_date,
        scope,
        sha256_hex(canonical_request.as_bytes())
    );

    let k_date = hmac(
        format!("AWS4{}", creds.secret_key).as_bytes(),
        date.as_bytes(),
    );
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, b"s3");
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        creds.access_key, scope, signed_headers, signature
    );

    // `host` is set by the HTTP client itself; everything else goes out.
    let mut out: Vec<(String, String)> = headers.into_iter().filter(|(k, _)| k != "host").collect();
    out.push(("authorization".into(), authorization));
    out
}

/// Current UTC time as `YYYYMMDDTHHMMSSZ`.
pub fn amz_date_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    amz_date_from_unix(secs)
}

/// `YYYYMMDDTHHMMSSZ` for a Unix timestamp (no leap seconds, as AWS).
pub fn amz_date_from_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example of the AWS documentation ("GET Object", signature v4,
    /// header authorisation). Every edition of s3pe reproduces it (D8).
    #[test]
    fn aws_documentation_example() {
        let creds = Credentials {
            access_key: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        };
        let extra = vec![("range".to_string(), "bytes=0-9".to_string())];
        let req = SignRequest {
            method: "GET",
            host: "examplebucket.s3.amazonaws.com",
            path: "/test.txt",
            query: &[],
            payload_hash: EMPTY_SHA256,
            amz_date: "20130524T000000Z",
            extra_headers: &extra,
        };
        let headers = sign(&req, &creds, "us-east-1");
        let auth = &headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .unwrap()
            .1;
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn encoding_keeps_unreserved_and_slashes_in_paths() {
        assert_eq!(uri_encode("a b/ç~x.y_z-1", true), "a%20b/%C3%A7~x.y_z-1");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(uri_encode("*+", false), "%2A%2B");
    }

    #[test]
    fn query_is_sorted_and_encoded() {
        let q = vec![
            ("prefix".to_string(), "dados/x y".to_string()),
            ("list-type".to_string(), "2".to_string()),
        ];
        assert_eq!(canonical_query(&q), "list-type=2&prefix=dados%2Fx%20y");
        assert_eq!(canonical_query(&[]), "");
    }

    #[test]
    fn dates() {
        assert_eq!(amz_date_from_unix(0), "19700101T000000Z");
        assert_eq!(amz_date_from_unix(1_369_353_600), "20130524T000000Z");
        assert_eq!(amz_date_from_unix(1_761_264_000), "20251024T000000Z");
        assert_eq!(amz_date_from_unix(951_782_400), "20000229T000000Z");
    }
}
