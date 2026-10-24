//! The client: configuration, the nine operations, retries and integrity.

use crate::error::{Error, Result};
use crate::sigv4::{self, Credentials, SignRequest, EMPTY_SHA256};
use crate::xml;
use md5::Md5;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

/// Signing region used when none is given.
pub const DEFAULT_REGION: &str = "us-east-1";
/// Seconds a single call may take before it is abandoned.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// Attempts per call: the first and two retries.
pub const DEFAULT_MAX_TRIES: u32 = 3;

const TRANSIENT: [u16; 5] = [429, 500, 502, 503, 504];

/// One object as listed or described by the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    /// Key inside the bucket.
    pub key: String,
    /// Size in bytes.
    pub size: u64,
    /// RFC 3339 (listing) or RFC 7231 (`HEAD`), as the server sent it.
    pub last_modified: String,
    /// Without the surrounding quotes.
    pub etag: String,
    /// Only known after a `HEAD`.
    pub content_type: Option<String>,
}

/// A bucket as listed by the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    /// Bucket name.
    pub name: String,
    /// Creation date, as the server sent it.
    pub created: String,
}

/// What a completed upload or download reports.
#[derive(Debug, Clone)]
pub struct Transfer {
    /// Bucket of the object.
    pub bucket: String,
    /// Key of the object.
    pub key: String,
    /// Bytes moved.
    pub size: u64,
    /// ETag reported by the server, without quotes.
    pub etag: String,
    /// Wall time of the whole call, retries included.
    pub elapsed: Duration,
}

impl Transfer {
    /// `s3://bucket/key`.
    pub fn url(&self) -> String {
        format!("s3://{}/{}", self.bucket, self.key)
    }
}

/// Builds a [`Client`]; anything not set comes from the environment, then
/// from the defaults (see the crate documentation for the variables).
#[derive(Debug, Default, Clone)]
pub struct ClientBuilder {
    endpoint: Option<String>,
    region: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    session_token: Option<String>,
    bucket: Option<String>,
    timeout: Option<Duration>,
    max_tries: Option<u32>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

impl ClientBuilder {
    /// `https://host[:port]`, no path.
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }
    /// Signing region (default `us-east-1`; Garage and MinIO accept whatever they are configured with).
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }
    /// Access key and secret key.
    pub fn credentials(
        mut self,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> Self {
        self.access_key = Some(access_key.into());
        self.secret_key = Some(secret_key.into());
        self
    }
    /// Session token, for temporary credentials.
    pub fn session_token(mut self, token: impl Into<String>) -> Self {
        self.session_token = Some(token.into());
        self
    }
    /// Bucket used when a key is not an `s3://` URL.
    pub fn bucket(mut self, bucket: impl Into<String>) -> Self {
        self.bucket = Some(bucket.into());
        self
    }
    /// Time limit of one call (default 300 s).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
    /// Attempts per call, at least 1 (default 3).
    pub fn max_tries(mut self, tries: u32) -> Self {
        self.max_tries = Some(tries.max(1));
        self
    }

    /// Resolves the configuration and builds the client. Fails without
    /// credentials or with an endpoint that is not `http(s)://host[:port]`.
    pub fn build(self) -> Result<Client> {
        let endpoint = self
            .endpoint
            .or_else(|| env("S3PE_ENDPOINT"))
            .or_else(|| env("AWS_ENDPOINT_URL"))
            .ok_or_else(|| {
                Error::Config(
                    "no endpoint: set S3PE_ENDPOINT (or AWS_ENDPOINT_URL) to https://host[:port] \
                     or pass it to the builder"
                        .into(),
                )
            })?;
        let (endpoint, host) = parse_endpoint(&endpoint)?;
        let region = self
            .region
            .or_else(|| env("AWS_REGION"))
            .or_else(|| env("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|| DEFAULT_REGION.to_string());
        let access_key = self.access_key.or_else(|| env("AWS_ACCESS_KEY_ID"));
        let secret_key = self.secret_key.or_else(|| env("AWS_SECRET_ACCESS_KEY"));
        let (access_key, secret_key) = match (access_key, secret_key) {
            (Some(a), Some(s)) => (a, s),
            _ => {
                return Err(Error::Config(
                    "no credentials: set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY \
                     or pass them to the builder"
                        .into(),
                ))
            }
        };
        let credentials = Credentials {
            access_key,
            secret_key,
            session_token: self.session_token.or_else(|| env("AWS_SESSION_TOKEN")),
        };
        let timeout = self.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_per_call(Some(timeout))
            .timeout_connect(Some(Duration::from_secs(20)))
            .user_agent(concat!("s3pe/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Ok(Client {
            agent,
            endpoint,
            host,
            region,
            credentials,
            bucket: self.bucket.or_else(|| env("S3PE_BUCKET")),
            max_tries: self.max_tries.unwrap_or(DEFAULT_MAX_TRIES),
        })
    }
}

/// `(endpoint without trailing slash, host header value)`.
fn parse_endpoint(raw: &str) -> Result<(String, String)> {
    let raw = raw.trim().trim_end_matches('/');
    let (scheme, rest) = raw.split_once("://").ok_or_else(|| {
        Error::Config(format!(
            "endpoint must start with http:// or https://: {raw}"
        ))
    })?;
    if scheme != "http" && scheme != "https" {
        return Err(Error::Config(format!(
            "endpoint scheme must be http or https: {raw}"
        )));
    }
    if rest.is_empty() || rest.contains('/') || rest.contains('?') {
        return Err(Error::Config(format!(
            "endpoint must be scheme://host[:port], no path: {raw}"
        )));
    }
    // Drop a default port so the Host header matches what the HTTP client sends.
    let host = match (scheme, rest.rsplit_once(':')) {
        ("https", Some((h, "443"))) | ("http", Some((h, "80"))) => h.to_string(),
        _ => rest.to_string(),
    };
    Ok((format!("{scheme}://{host}"), host))
}

/// A configured connection to one S3 endpoint.
///
/// Build it with [`Client::from_env`] or [`Client::builder`]. Every method
/// takes a key that is either relative to the client's bucket or a full
/// `s3://bucket/key` URL.
#[derive(Debug, Clone)]
pub struct Client {
    agent: ureq::Agent,
    endpoint: String,
    host: String,
    region: String,
    credentials: Credentials,
    bucket: Option<String>,
    max_tries: u32,
}

enum Payload<'a> {
    Empty,
    Bytes(&'a [u8]),
    File(&'a Path),
}

struct Hashes {
    sha256: String,
    md5: String,
    len: u64,
}

fn hash_bytes(data: &[u8]) -> Hashes {
    Hashes {
        sha256: hex::encode(Sha256::digest(data)),
        md5: hex::encode(Md5::digest(data)),
        len: data.len() as u64,
    }
}

fn hash_file(path: &Path) -> Result<Hashes> {
    let mut f = File::open(path)?;
    let mut sha = Sha256::new();
    let mut md = Md5::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut len = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        md.update(&buf[..n]);
        len += n as u64;
    }
    Ok(Hashes {
        sha256: hex::encode(sha.finalize()),
        md5: hex::encode(md.finalize()),
        len,
    })
}

impl Client {
    /// A client configured entirely from the environment.
    pub fn from_env() -> Result<Client> {
        ClientBuilder::default().build()
    }

    /// A builder to set some values explicitly; the rest comes from the environment.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// The endpoint, as `scheme://host[:port]`.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The default bucket, if any.
    pub fn bucket(&self) -> Option<&str> {
        self.bucket.as_deref()
    }

    /// Splits `s3://bucket/key` or applies the default bucket to a bare key.
    pub fn resolve(&self, key: &str) -> Result<(String, String)> {
        if let Some(rest) = key.strip_prefix("s3://") {
            let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
            if bucket.is_empty() {
                return Err(Error::Config(format!("no bucket in URL: s3://{rest}")));
            }
            return Ok((bucket.to_string(), key.to_string()));
        }
        let key = key.strip_prefix('/').unwrap_or(key);
        match &self.bucket {
            Some(b) => Ok((b.clone(), key.to_string())),
            None => Err(Error::Config(format!(
                "no bucket: write the key as s3://bucket/{key}, set S3PE_BUCKET or build the client with a bucket"
            ))),
        }
    }

    /// Reads an object into memory.
    pub fn read(&self, key: &str) -> Result<Vec<u8>> {
        let (bucket, key) = self.resolve(key)?;
        if key.is_empty() {
            return Err(Error::Config(format!("no key to read in s3://{bucket}/")));
        }
        let start = Instant::now();
        let mut resp = self.send("GET", &bucket, &key, &[], Payload::Empty, None)?;
        let expected = content_length(&resp);
        let etag = etag_of(&resp);
        let mut data = Vec::with_capacity(expected.unwrap_or(0) as usize);
        resp.body_mut()
            .as_reader()
            .read_to_end(&mut data)
            .map_err(|e| Error::Transport(format!("reading s3://{bucket}/{key}: {e}")))?;
        check_download(
            &bucket,
            &key,
            data.len() as u64,
            expected,
            &etag,
            Some(&data),
        )?;
        log::info!(target: "s3pe", "↓ s3://{bucket}/{key} ({}, {:.1} s)", format_size(data.len() as u64), start.elapsed().as_secs_f64());
        Ok(data)
    }

    /// Writes bytes to an object. `content_type` defaults to a guess from
    /// the key's extension. The ETag is checked against the local MD5.
    pub fn write(
        &self,
        key: &str,
        body: impl AsRef<[u8]>,
        content_type: Option<&str>,
    ) -> Result<Transfer> {
        let (bucket, key) = self.resolve(key)?;
        if key.is_empty() {
            return Err(Error::Config(format!("no key to write in s3://{bucket}/")));
        }
        let body = body.as_ref();
        let ct = content_type
            .map(str::to_string)
            .unwrap_or_else(|| content_type_for(&key).to_string());
        let start = Instant::now();
        let resp = self.send("PUT", &bucket, &key, &[], Payload::Bytes(body), Some(&ct))?;
        let etag = etag_of(&resp);
        check_upload(&bucket, &key, &hash_bytes(body).md5, &etag)?;
        let t = Transfer {
            bucket,
            key,
            size: body.len() as u64,
            etag,
            elapsed: start.elapsed(),
        };
        log::info!(target: "s3pe", "↑ {} ({}, {:.1} s)", t.url(), format_size(t.size), t.elapsed.as_secs_f64());
        Ok(t)
    }

    /// Downloads an object to a file, streaming. The file is written next to
    /// its final name and renamed when complete, so a failure leaves nothing
    /// half-written at `path`.
    pub fn download(&self, key: &str, path: impl AsRef<Path>) -> Result<Transfer> {
        let (bucket, key) = self.resolve(key)?;
        if key.is_empty() {
            return Err(Error::Config(format!(
                "no key to download in s3://{bucket}/"
            )));
        }
        let path = path.as_ref();
        let start = Instant::now();
        let mut resp = self.send("GET", &bucket, &key, &[], Payload::Empty, None)?;
        let expected = content_length(&resp);
        let etag = etag_of(&resp);
        let part = path.with_extension(match path.extension() {
            Some(e) => format!("{}.s3pe-part", e.to_string_lossy()),
            None => "s3pe-part".to_string(),
        });
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let result = (|| -> Result<(u64, String)> {
            let mut out = File::create(&part)?;
            let mut reader = resp.body_mut().as_reader();
            let mut md = Md5::new();
            let mut buf = vec![0u8; 1 << 16];
            let mut n_total = 0u64;
            loop {
                let n = reader
                    .read(&mut buf)
                    .map_err(|e| Error::Transport(format!("reading s3://{bucket}/{key}: {e}")))?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                md.update(&buf[..n]);
                n_total += n as u64;
            }
            out.flush()?;
            Ok((n_total, hex::encode(md.finalize())))
        })();
        let (size, md5) = match result {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        };
        if let Err(e) = check_download_md5(&bucket, &key, size, expected, &etag, &md5) {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
        std::fs::rename(&part, path)?;
        let t = Transfer {
            bucket,
            key,
            size,
            etag,
            elapsed: start.elapsed(),
        };
        log::info!(target: "s3pe", "↓ {} → {} ({}, {:.1} s)", t.url(), path.display(), format_size(size), t.elapsed.as_secs_f64());
        Ok(t)
    }

    /// Uploads a file, streaming (two passes over the file: one to hash,
    /// one to send). `content_type` defaults to a guess from the key.
    pub fn upload(
        &self,
        path: impl AsRef<Path>,
        key: &str,
        content_type: Option<&str>,
    ) -> Result<Transfer> {
        let path = path.as_ref();
        let (bucket, key) = self.resolve(key)?;
        let key = if key.is_empty() || key.ends_with('/') {
            let name = path
                .file_name()
                .ok_or_else(|| Error::Config(format!("no file name in {}", path.display())))?
                .to_string_lossy();
            format!("{key}{name}")
        } else {
            key
        };
        let ct = content_type
            .map(str::to_string)
            .unwrap_or_else(|| content_type_for(&key).to_string());
        let start = Instant::now();
        let hashes = hash_file(path)?;
        let resp = self.send_hashed(
            "PUT",
            &bucket,
            &key,
            &[],
            Payload::File(path),
            Some(&ct),
            &hashes,
        )?;
        let etag = etag_of(&resp);
        check_upload(&bucket, &key, &hashes.md5, &etag)?;
        let t = Transfer {
            bucket,
            key,
            size: hashes.len,
            etag,
            elapsed: start.elapsed(),
        };
        log::info!(target: "s3pe", "↑ {} → {} ({}, {:.1} s)", path.display(), t.url(), format_size(t.size), t.elapsed.as_secs_f64());
        Ok(t)
    }

    /// Lists every object under a prefix (all pages). `prefix` may be an
    /// `s3://bucket/prefix` URL, a bare prefix, or `""` for the whole bucket.
    pub fn list(&self, prefix: &str) -> Result<Vec<Object>> {
        let (bucket, prefix) = self.resolve(prefix)?;
        let start = Instant::now();
        let mut objects = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![("list-type".to_string(), "2".to_string())];
            if !prefix.is_empty() {
                query.push(("prefix".into(), prefix.clone()));
            }
            if let Some(t) = &token {
                query.push(("continuation-token".into(), t.clone()));
            }
            let mut resp = self.send("GET", &bucket, "", &query, Payload::Empty, None)?;
            let body = read_text(&mut resp, &bucket, &prefix)?;
            let (page, next) = xml::parse_listing(&body).map_err(|e| {
                Error::Transport(format!("bad listing XML from s3://{bucket}/{prefix}: {e}"))
            })?;
            objects.extend(page);
            match next {
                Some(t) => token = Some(t),
                None => break,
            }
        }
        log::info!(target: "s3pe", "✔ {} object{} under s3://{bucket}/{prefix} ({:.1} s)", objects.len(), if objects.len() == 1 { "" } else { "s" }, start.elapsed().as_secs_f64());
        Ok(objects)
    }

    /// Deletes an object. Deleting what does not exist is not an error.
    pub fn delete(&self, key: &str) -> Result<()> {
        let (bucket, key) = self.resolve(key)?;
        if key.is_empty() {
            return Err(Error::Config(format!("no key to delete in s3://{bucket}/")));
        }
        self.send("DELETE", &bucket, &key, &[], Payload::Empty, None)?;
        log::info!(target: "s3pe", "✖ s3://{bucket}/{key}");
        Ok(())
    }

    /// Metadata of an object, or `None` when it does not exist.
    pub fn head(&self, key: &str) -> Result<Option<Object>> {
        let (bucket, key) = self.resolve(key)?;
        if key.is_empty() {
            return Err(Error::Config(format!(
                "no key to inspect in s3://{bucket}/"
            )));
        }
        match self.send("HEAD", &bucket, &key, &[], Payload::Empty, None) {
            Ok(resp) => Ok(Some(Object {
                key,
                size: content_length(&resp).unwrap_or(0),
                last_modified: header(&resp, "last-modified"),
                etag: etag_of(&resp),
                content_type: Some(header(&resp, "content-type")).filter(|s| !s.is_empty()),
            })),
            Err(Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Whether an object exists.
    pub fn exists(&self, key: &str) -> Result<bool> {
        Ok(self.head(key)?.is_some())
    }

    /// The buckets the credentials can see.
    pub fn buckets(&self) -> Result<Vec<Bucket>> {
        let mut resp = self.send("GET", "", "", &[], Payload::Empty, None)?;
        let body = read_text(&mut resp, "", "")?;
        xml::parse_buckets(&body).map_err(|e| Error::Transport(format!("bad bucket list XML: {e}")))
    }

    fn send(
        &self,
        method: &str,
        bucket: &str,
        key: &str,
        query: &[(String, String)],
        payload: Payload,
        content_type: Option<&str>,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let hashes = match &payload {
            Payload::Empty => Hashes {
                sha256: EMPTY_SHA256.into(),
                md5: String::new(),
                len: 0,
            },
            Payload::Bytes(b) => hash_bytes(b),
            Payload::File(p) => hash_file(p)?,
        };
        self.send_hashed(method, bucket, key, query, payload, content_type, &hashes)
    }

    #[allow(clippy::too_many_arguments)]
    fn send_hashed(
        &self,
        method: &str,
        bucket: &str,
        key: &str,
        query: &[(String, String)],
        payload: Payload,
        content_type: Option<&str>,
        hashes: &Hashes,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let path = if bucket.is_empty() {
            "/".to_string()
        } else if key.is_empty() {
            format!("/{}", sigv4::uri_encode(bucket, false))
        } else {
            format!(
                "/{}/{}",
                sigv4::uri_encode(bucket, false),
                sigv4::uri_encode(key, true)
            )
        };
        let mut url = format!("{}{}", self.endpoint, path);
        let qs = sigv4::canonical_query(query);
        if !qs.is_empty() {
            url.push('?');
            url.push_str(&qs);
        }

        let mut tries = 0;
        loop {
            tries += 1;
            let amz_date = sigv4::amz_date_now();
            let signed = sigv4::sign(
                &SignRequest {
                    method,
                    host: &self.host,
                    path: &path,
                    query,
                    payload_hash: &hashes.sha256,
                    amz_date: &amz_date,
                    extra_headers: &[],
                },
                &self.credentials,
                &self.region,
            );
            let attempt = self.attempt(method, &url, &signed, &payload, content_type, hashes.len);
            match attempt {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if TRANSIENT.contains(&status) && tries < self.max_tries {
                        let wait = backoff(tries);
                        log::warn!(target: "s3pe", "HTTP {status} from {method} s3://{bucket}/{key}; retrying in {} s ({tries}/{})", wait.as_secs(), self.max_tries);
                        std::thread::sleep(wait);
                        continue;
                    }
                    return self.check(resp, method, bucket, key);
                }
                Err(Error::Transport(msg)) if tries < self.max_tries => {
                    let wait = backoff(tries);
                    log::warn!(target: "s3pe", "{msg}; retrying in {} s ({tries}/{})", wait.as_secs(), self.max_tries);
                    std::thread::sleep(wait);
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn attempt(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        payload: &Payload,
        content_type: Option<&str>,
        len: u64,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let transport = |e: ureq::Error| Error::Transport(format!("{method} {url}: {e}"));
        match method {
            "PUT" => {
                let mut req = self.agent.put(url);
                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                if let Some(ct) = content_type {
                    req = req.header("content-type", ct);
                }
                req = req.header("content-length", len.to_string());
                match payload {
                    Payload::Bytes(b) => req.send(*b).map_err(transport),
                    Payload::File(p) => {
                        let f = File::open(p)?;
                        req.send(f).map_err(transport)
                    }
                    Payload::Empty => req.send_empty().map_err(transport),
                }
            }
            _ => {
                let mut req = match method {
                    "GET" => self.agent.get(url),
                    "HEAD" => self.agent.head(url),
                    "DELETE" => self.agent.delete(url),
                    other => return Err(Error::Config(format!("unsupported method {other}"))),
                };
                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                req.call().map_err(transport)
            }
        }
    }

    fn check(
        &self,
        mut resp: ureq::http::Response<ureq::Body>,
        method: &str,
        bucket: &str,
        key: &str,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(resp);
        }
        let (code, message) = if method == "HEAD" {
            (String::new(), String::new())
        } else {
            let body = resp
                .body_mut()
                .with_config()
                .limit(64 * 1024)
                .read_to_string()
                .unwrap_or_default();
            xml::parse_error(&body)
        };
        Err(match status {
            404 => Error::NotFound {
                bucket: bucket.into(),
                key: key.into(),
            },
            403 => Error::AccessDenied {
                bucket: bucket.into(),
                key: key.into(),
                message,
            },
            _ => Error::Http {
                status,
                code,
                message,
                bucket: bucket.into(),
                key: key.into(),
            },
        })
    }
}

fn backoff(tries: u32) -> Duration {
    Duration::from_secs(1u64 << (tries - 1).min(4))
}

fn header(resp: &ureq::http::Response<ureq::Body>, name: &str) -> String {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

fn etag_of(resp: &ureq::http::Response<ureq::Body>) -> String {
    header(resp, "etag").trim_matches('"').to_string()
}

fn content_length(resp: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    header(resp, "content-length").parse().ok()
}

fn read_text(
    resp: &mut ureq::http::Response<ureq::Body>,
    bucket: &str,
    key: &str,
) -> Result<String> {
    resp.body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| Error::Transport(format!("reading s3://{bucket}/{key}: {e}")))
}

fn looks_like_md5(etag: &str) -> bool {
    etag.len() == 32 && etag.bytes().all(|b| b.is_ascii_hexdigit())
}

fn check_upload(bucket: &str, key: &str, md5: &str, etag: &str) -> Result<()> {
    if looks_like_md5(etag) && !etag.eq_ignore_ascii_case(md5) {
        return Err(Error::Integrity(format!(
            "s3://{bucket}/{key}: the server's ETag {etag} differs from the MD5 of what was sent ({md5})"
        )));
    }
    Ok(())
}

fn check_download(
    bucket: &str,
    key: &str,
    got: u64,
    expected: Option<u64>,
    etag: &str,
    data: Option<&[u8]>,
) -> Result<()> {
    if let Some(exp) = expected {
        if exp != got {
            return Err(Error::Integrity(format!(
                "s3://{bucket}/{key}: received {got} bytes, Content-Length said {exp}"
            )));
        }
    }
    if let (true, Some(d)) = (looks_like_md5(etag), data) {
        let md5 = hex::encode(Md5::digest(d));
        if !etag.eq_ignore_ascii_case(&md5) {
            return Err(Error::Integrity(format!(
                "s3://{bucket}/{key}: MD5 of what arrived ({md5}) differs from the ETag {etag}"
            )));
        }
    }
    Ok(())
}

fn check_download_md5(
    bucket: &str,
    key: &str,
    got: u64,
    expected: Option<u64>,
    etag: &str,
    md5: &str,
) -> Result<()> {
    check_download(bucket, key, got, expected, "", None)?;
    if looks_like_md5(etag) && !etag.eq_ignore_ascii_case(md5) {
        return Err(Error::Integrity(format!(
            "s3://{bucket}/{key}: MD5 of what arrived ({md5}) differs from the ETag {etag}"
        )));
    }
    Ok(())
}

/// `12.3 MB`, `1.2 kB`, `512 B` (SI units, one decimal).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// The content type a key's extension suggests (`application/octet-stream`
/// when unknown). The same table is used by every edition of s3pe.
pub fn content_type_for(key: &str) -> &'static str {
    let lower = key.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if name.ends_with(".csv.gz") || name.ends_with(".tsv.gz") || name.ends_with(".json.gz") {
        return "application/gzip";
    }
    match name.rsplit_once('.').map(|(_, e)| e).unwrap_or("") {
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" => "application/json",
        "geojson" => "application/geo+json",
        "parquet" => "application/vnd.apache.parquet",
        "gz" => "application/gzip",
        "zip" => "application/zip",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "js" => "text/javascript",
        "css" => "text/css",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        assert_eq!(
            parse_endpoint("https://s3.example.org/").unwrap(),
            ("https://s3.example.org".into(), "s3.example.org".into())
        );
        assert_eq!(
            parse_endpoint("http://127.0.0.1:9003").unwrap(),
            ("http://127.0.0.1:9003".into(), "127.0.0.1:9003".into())
        );
        assert_eq!(
            parse_endpoint("https://x.org:443").unwrap(),
            ("https://x.org".into(), "x.org".into())
        );
        assert!(parse_endpoint("s3.example.org").is_err());
        assert!(parse_endpoint("https://s3.example.org/bucket").is_err());
    }

    #[test]
    fn sizes_and_types() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1234), "1.2 kB");
        assert_eq!(format_size(12_345_678), "12.3 MB");
        assert_eq!(content_type_for("raw/series.csv.gz"), "application/gzip");
        assert_eq!(
            content_type_for("a/b.PARQUET"),
            "application/vnd.apache.parquet"
        );
        assert_eq!(content_type_for("noext"), "application/octet-stream");
        assert_eq!(content_type_for("x.geojson"), "application/geo+json");
    }

    #[test]
    fn resolve_keys() {
        let c = Client::builder()
            .endpoint("https://s3.example.org")
            .credentials("k", "s")
            .bucket("data")
            .build()
            .unwrap();
        assert_eq!(
            c.resolve("raw/x.json").unwrap(),
            ("data".into(), "raw/x.json".into())
        );
        assert_eq!(
            c.resolve("/raw/x.json").unwrap(),
            ("data".into(), "raw/x.json".into())
        );
        assert_eq!(
            c.resolve("s3://other/k").unwrap(),
            ("other".into(), "k".into())
        );
        assert_eq!(
            c.resolve("s3://other").unwrap(),
            ("other".into(), "".into())
        );
        assert!(c.resolve("s3:///k").is_err());
        let no_bucket = Client::builder()
            .endpoint("https://s3.example.org")
            .credentials("k", "s")
            .build()
            .unwrap();
        assert!(matches!(no_bucket.resolve("k"), Err(Error::Config(_))));
        std::env::remove_var("S3PE_ENDPOINT");
        std::env::remove_var("AWS_ENDPOINT_URL");
        assert!(matches!(
            Client::builder().credentials("k", "s").build(),
            Err(Error::Config(_))
        ));
    }
}
