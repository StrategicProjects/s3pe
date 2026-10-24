//! The client: configuration, the operations, retries and integrity.

use crate::error::{Error, Result};
use crate::sigv4::{self, Credentials, PresignRequest, SignRequest, EMPTY_SHA256};
use crate::xml;
use md5::Md5;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Signing region used when none is given.
pub const DEFAULT_REGION: &str = "us-east-1";
/// Seconds a single call may take before it is abandoned.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// Attempts per call: the first and two retries.
pub const DEFAULT_MAX_TRIES: u32 = 3;
/// Bodies larger than this go up as a multipart upload (D15).
pub const DEFAULT_MULTIPART_THRESHOLD: u64 = 64 * 1024 * 1024;
/// Size of each part of a multipart upload (D15).
pub const DEFAULT_PART_SIZE: u64 = 16 * 1024 * 1024;
/// Parts sent at the same time (D15).
pub const DEFAULT_PARALLEL: usize = 4;
/// Smallest part S3 accepts, except for the last one.
pub const MIN_PART_SIZE: u64 = 5 * 1024 * 1024;
/// Longest validity of a pre-signed URL (seven days).
pub const MAX_PRESIGN_SECS: u64 = 7 * 24 * 3600;

const TRANSIENT: [u16; 5] = [429, 500, 502, 503, 504];

/// One object as listed or described by the store.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
    /// User metadata (`x-amz-meta-*`, names without the prefix), after a `HEAD`.
    pub metadata: BTreeMap<String, String>,
}

/// One level of a bucket: the objects directly under a prefix and the
/// common prefixes ("folders") below it (D17).
#[derive(Debug, Clone, Default)]
pub struct Listing {
    /// Objects directly under the prefix.
    pub objects: Vec<Object>,
    /// Common prefixes, each ending with the delimiter.
    pub prefixes: Vec<String>,
}

/// A bucket as listed by the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    /// Bucket name.
    pub name: String,
    /// Creation date, as the server sent it.
    pub created: String,
}

/// What a completed upload, download or copy reports.
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

/// Options of `write_with` and `upload_with` (D17).
#[derive(Debug, Clone, Default)]
pub struct PutOptions {
    /// Content type; guessed from the key when `None`.
    pub content_type: Option<String>,
    /// User metadata, sent as `x-amz-meta-<name>`.
    pub metadata: Vec<(String, String)>,
    /// Any other header (`Cache-Control`, `Content-Disposition`,
    /// `x-amz-storage-class`, `x-amz-server-side-encryption`, …).
    pub headers: Vec<(String, String)>,
}

impl PutOptions {
    /// Options with only a content type.
    pub fn content_type(ct: &str) -> Self {
        PutOptions {
            content_type: Some(ct.to_string()),
            ..Default::default()
        }
    }
}

/// Options of `sync_up` and `sync_down` (D18).
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Remove what exists on the destination but not on the source.
    pub delete: bool,
    /// Relative path of the file to transfer last (a manifest).
    pub last: Option<String>,
}

/// What a sync did.
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    /// Relative paths uploaded or downloaded.
    pub transferred: Vec<String>,
    /// Relative paths already identical on both sides.
    pub skipped: Vec<String>,
    /// Relative paths removed from the destination.
    pub deleted: Vec<String>,
    /// Wall time.
    pub elapsed: Duration,
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
    multipart_threshold: Option<u64>,
    part_size: Option<u64>,
    parallel: Option<usize>,
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
    /// Signing region (default `us-east-1`; on AWS it must be the bucket's region).
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
    /// Bodies above this size go up in parts (default 64 MiB).
    pub fn multipart_threshold(mut self, bytes: u64) -> Self {
        self.multipart_threshold = Some(bytes);
        self
    }
    /// Part size, at least 5 MiB (default 16 MiB).
    pub fn part_size(mut self, bytes: u64) -> Self {
        self.part_size = Some(bytes.max(MIN_PART_SIZE));
        self
    }
    /// Parts sent at the same time, at least 1 (default 4; `S3PE_PARALLEL`).
    pub fn parallel(mut self, n: usize) -> Self {
        self.parallel = Some(n.max(1));
        self
    }

    /// Resolves the configuration and builds the client. Fails without an
    /// endpoint, without credentials, or with an endpoint that is not
    /// `http(s)://host[:port]`.
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
        let env_u64 = |name: &str| env(name).and_then(|v| v.parse::<u64>().ok());
        Ok(Client {
            agent,
            endpoint,
            host,
            region,
            credentials,
            bucket: self.bucket.or_else(|| env("S3PE_BUCKET")),
            max_tries: self.max_tries.unwrap_or(DEFAULT_MAX_TRIES),
            multipart_threshold: self
                .multipart_threshold
                .or_else(|| env_u64("S3PE_MULTIPART_THRESHOLD"))
                .unwrap_or(DEFAULT_MULTIPART_THRESHOLD),
            part_size: self
                .part_size
                .or_else(|| env_u64("S3PE_PART_SIZE").map(|v| v.max(MIN_PART_SIZE)))
                .unwrap_or(DEFAULT_PART_SIZE),
            parallel: self
                .parallel
                .or_else(|| env_u64("S3PE_PARALLEL").map(|v| (v as usize).max(1)))
                .unwrap_or(DEFAULT_PARALLEL),
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
    multipart_threshold: u64,
    part_size: u64,
    parallel: usize,
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

/// Reads `len` bytes at `offset` of a file.
fn read_range(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Source of a body: memory or a file.
#[derive(Clone, Copy)]
enum Source<'a> {
    Bytes(&'a [u8]),
    File(&'a Path),
}

impl Source<'_> {
    fn part(&self, offset: u64, len: u64) -> Result<Vec<u8>> {
        match self {
            Source::Bytes(b) => Ok(b[offset as usize..(offset + len) as usize].to_vec()),
            Source::File(p) => read_range(p, offset, len),
        }
    }
}

/// The ETag S3 gives a multipart upload: `md5(concat(part MD5 bytes))-N`.
pub fn multipart_etag(part_md5s_hex: &[String]) -> String {
    let mut all = Vec::with_capacity(part_md5s_hex.len() * 16);
    for h in part_md5s_hex {
        all.extend(hex::decode(h).unwrap_or_default());
    }
    format!("{}-{}", hex::encode(Md5::digest(&all)), part_md5s_hex.len())
}

/// The ETag the store would give a local file uploaded by this client:
/// its MD5, or the multipart ETag (D15) when it is above the threshold.
pub fn expected_etag(path: &Path, threshold: u64, part_size: u64) -> Result<String> {
    let len = std::fs::metadata(path)?.len();
    if len <= threshold {
        return Ok(hash_file(path)?.md5);
    }
    let mut f = File::open(path)?;
    let mut md5s = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut remaining = len;
    while remaining > 0 {
        let this = remaining.min(part_size);
        let mut md = Md5::new();
        let mut left = this;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            let n = f.read(&mut buf[..want])?;
            if n == 0 {
                break;
            }
            md.update(&buf[..n]);
            left -= n as u64;
        }
        md5s.push(hex::encode(md.finalize()));
        remaining -= this;
    }
    Ok(multipart_etag(&md5s))
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

    fn resolve_object(&self, key: &str, verb: &str) -> Result<(String, String)> {
        let (bucket, key) = self.resolve(key)?;
        if key.is_empty() {
            return Err(Error::Config(format!("no key to {verb} in s3://{bucket}/")));
        }
        Ok((bucket, key))
    }

    /// Reads an object into memory.
    pub fn read(&self, key: &str) -> Result<Vec<u8>> {
        let (bucket, key) = self.resolve_object(key, "read")?;
        let start = Instant::now();
        let mut resp = self.send("GET", &bucket, &key, &[], Payload::Empty, &[])?;
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
    /// the key's extension. Large bodies go up in parts (D15).
    pub fn write(
        &self,
        key: &str,
        body: impl AsRef<[u8]>,
        content_type: Option<&str>,
    ) -> Result<Transfer> {
        let opts = PutOptions {
            content_type: content_type.map(str::to_string),
            ..Default::default()
        };
        self.write_with(key, body, &opts)
    }

    /// Writes bytes to an object with metadata and headers (D17).
    pub fn write_with(
        &self,
        key: &str,
        body: impl AsRef<[u8]>,
        opts: &PutOptions,
    ) -> Result<Transfer> {
        let (bucket, key) = self.resolve_object(key, "write")?;
        let body = body.as_ref();
        let start = Instant::now();
        let t = self.put_source(
            &bucket,
            &key,
            Source::Bytes(body),
            body.len() as u64,
            opts,
            start,
        )?;
        log::info!(target: "s3pe", "↑ {} ({}, {:.1} s)", t.url(), format_size(t.size), t.elapsed.as_secs_f64());
        Ok(t)
    }

    /// Uploads a file, streaming. `content_type` defaults to a guess from
    /// the key. A key that is empty or ends with `/` gets the file's name.
    pub fn upload(
        &self,
        path: impl AsRef<Path>,
        key: &str,
        content_type: Option<&str>,
    ) -> Result<Transfer> {
        let opts = PutOptions {
            content_type: content_type.map(str::to_string),
            ..Default::default()
        };
        self.upload_with(path, key, &opts)
    }

    /// Uploads a file with metadata and headers (D17).
    pub fn upload_with(
        &self,
        path: impl AsRef<Path>,
        key: &str,
        opts: &PutOptions,
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
        let start = Instant::now();
        let len = std::fs::metadata(path)?.len();
        let t = self.put_source(&bucket, &key, Source::File(path), len, opts, start)?;
        log::info!(target: "s3pe", "↑ {} → {} ({}, {:.1} s)", path.display(), t.url(), format_size(t.size), t.elapsed.as_secs_f64());
        Ok(t)
    }

    fn put_headers(&self, key: &str, opts: &PutOptions) -> Vec<(String, String)> {
        let ct = opts
            .content_type
            .clone()
            .unwrap_or_else(|| content_type_for(key).to_string());
        let mut headers = vec![("content-type".to_string(), ct)];
        for (k, v) in &opts.metadata {
            headers.push((format!("x-amz-meta-{}", k.to_ascii_lowercase()), v.clone()));
        }
        for (k, v) in &opts.headers {
            headers.push((k.to_ascii_lowercase(), v.clone()));
        }
        headers
    }

    fn put_source(
        &self,
        bucket: &str,
        key: &str,
        source: Source,
        len: u64,
        opts: &PutOptions,
        start: Instant,
    ) -> Result<Transfer> {
        let headers = self.put_headers(key, opts);
        if len > self.multipart_threshold {
            return self.multipart(bucket, key, source, len, &headers, start);
        }
        let (payload, hashes) = match source {
            Source::Bytes(b) => (Payload::Bytes(b), hash_bytes(b)),
            Source::File(p) => (Payload::File(p), hash_file(p)?),
        };
        let resp = self.send_hashed("PUT", bucket, key, &[], payload, &headers, &hashes)?;
        let etag = etag_of(&resp);
        check_upload(bucket, key, &hashes.md5, &etag)?;
        Ok(Transfer {
            bucket: bucket.into(),
            key: key.into(),
            size: len,
            etag,
            elapsed: start.elapsed(),
        })
    }

    fn multipart(
        &self,
        bucket: &str,
        key: &str,
        source: Source,
        len: u64,
        headers: &[(String, String)],
        start: Instant,
    ) -> Result<Transfer> {
        let uploads = [("uploads".to_string(), String::new())];
        let mut resp = self.send("POST", bucket, key, &uploads, Payload::Empty, headers)?;
        let body = read_text(&mut resp, bucket, key)?;
        let upload_id = xml::first_text(&body, "UploadId").ok_or_else(|| {
            Error::Transport(format!("no UploadId in the answer for s3://{bucket}/{key}"))
        })?;
        log::debug!(target: "s3pe", "multipart upload {upload_id} of s3://{bucket}/{key}: {} bytes in parts of {}", len, self.part_size);

        let n_parts = len.div_ceil(self.part_size) as usize;
        let next = AtomicUsize::new(0);
        let results: Mutex<Vec<Option<Result<String>>>> =
            Mutex::new((0..n_parts).map(|_| None).collect());
        let workers = self.parallel.min(n_parts).max(1);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= n_parts {
                        break;
                    }
                    // Stop early once another part failed.
                    if results.lock().unwrap().iter().flatten().any(|r| r.is_err()) {
                        break;
                    }
                    let offset = i as u64 * self.part_size;
                    let this = (len - offset).min(self.part_size);
                    let r = self.upload_part(bucket, key, &upload_id, i + 1, source, offset, this);
                    results.lock().unwrap()[i] = Some(r);
                });
            }
        });

        let results = results.into_inner().unwrap();
        let mut md5s = Vec::with_capacity(n_parts);
        for (i, r) in results.into_iter().enumerate() {
            match r {
                Some(Ok(md5)) => md5s.push(md5),
                Some(Err(e)) => {
                    self.abort_multipart(bucket, key, &upload_id);
                    return Err(e);
                }
                None => {
                    self.abort_multipart(bucket, key, &upload_id);
                    return Err(Error::Transport(format!(
                        "part {} of s3://{bucket}/{key} was not uploaded",
                        i + 1
                    )));
                }
            }
        }

        let mut complete = String::from("<CompleteMultipartUpload>");
        for (i, md5) in md5s.iter().enumerate() {
            complete.push_str(&format!(
                "<Part><PartNumber>{}</PartNumber><ETag>\"{}\"</ETag></Part>",
                i + 1,
                md5
            ));
        }
        complete.push_str("</CompleteMultipartUpload>");
        let q = [("uploadId".to_string(), upload_id.clone())];
        let ct = [("content-type".to_string(), "application/xml".to_string())];
        let mut resp = match self.send(
            "POST",
            bucket,
            key,
            &q,
            Payload::Bytes(complete.as_bytes()),
            &ct,
        ) {
            Ok(r) => r,
            Err(e) => {
                self.abort_multipart(bucket, key, &upload_id);
                return Err(e);
            }
        };
        let body = read_text(&mut resp, bucket, key)?;
        if let Some((code, message)) = Some(xml::parse_error(&body)).filter(|(c, _)| !c.is_empty())
        {
            self.abort_multipart(bucket, key, &upload_id);
            return Err(Error::Http {
                status: 200,
                code,
                message,
                bucket: bucket.into(),
                key: key.into(),
            });
        }
        let etag = xml::first_text(&body, "ETag").unwrap_or_default();
        let expected = multipart_etag(&md5s);
        if !etag.is_empty() && !etag.eq_ignore_ascii_case(&expected) {
            return Err(Error::Integrity(format!(
                "s3://{bucket}/{key}: multipart ETag {etag} differs from the expected {expected}"
            )));
        }
        Ok(Transfer {
            bucket: bucket.into(),
            key: key.into(),
            size: len,
            etag,
            elapsed: start.elapsed(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn upload_part(
        &self,
        bucket: &str,
        key: &str,
        upload_id: &str,
        number: usize,
        source: Source,
        offset: u64,
        len: u64,
    ) -> Result<String> {
        let data = source.part(offset, len)?;
        let hashes = hash_bytes(&data);
        let q = [
            ("partNumber".to_string(), number.to_string()),
            ("uploadId".to_string(), upload_id.to_string()),
        ];
        // An explicit binary content type: some web application firewalls parse
        // untyped or text bodies as form parameters and refuse large ones.
        let ct = [(
            "content-type".to_string(),
            "application/octet-stream".to_string(),
        )];
        let resp = self.send_hashed("PUT", bucket, key, &q, Payload::Bytes(&data), &ct, &hashes)?;
        let etag = etag_of(&resp);
        if looks_like_md5(&etag) && !etag.eq_ignore_ascii_case(&hashes.md5) {
            return Err(Error::Integrity(format!(
                "s3://{bucket}/{key} part {number}: ETag {etag} differs from the MD5 of what was sent ({})",
                hashes.md5
            )));
        }
        log::debug!(target: "s3pe", "part {number} of s3://{bucket}/{key}: {} bytes", len);
        Ok(hashes.md5)
    }

    fn abort_multipart(&self, bucket: &str, key: &str, upload_id: &str) {
        let q = [("uploadId".to_string(), upload_id.to_string())];
        match self.send("DELETE", bucket, key, &q, Payload::Empty, &[]) {
            Ok(_) => log::warn!(target: "s3pe", "multipart upload of s3://{bucket}/{key} aborted"),
            Err(e) => {
                log::warn!(target: "s3pe", "could not abort the multipart upload of s3://{bucket}/{key}: {e}")
            }
        }
    }

    /// Downloads an object to a file, streaming. The file is written next to
    /// its final name and renamed when complete, so a failure leaves nothing
    /// half-written at `path`.
    pub fn download(&self, key: &str, path: impl AsRef<Path>) -> Result<Transfer> {
        let (bucket, key) = self.resolve_object(key, "download")?;
        let path = path.as_ref();
        let start = Instant::now();
        let mut resp = self.send("GET", &bucket, &key, &[], Payload::Empty, &[])?;
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

    /// Lists every object under a prefix (all pages). `prefix` may be an
    /// `s3://bucket/prefix` URL, a bare prefix, or `""` for the whole bucket.
    pub fn list(&self, prefix: &str) -> Result<Vec<Object>> {
        Ok(self.list_with(prefix, None)?.objects)
    }

    /// Lists one level: the objects directly under the prefix and the
    /// common prefixes below it (delimiter `/`, D17).
    pub fn list_dir(&self, prefix: &str) -> Result<Listing> {
        self.list_with(prefix, Some("/"))
    }

    /// Lists with an optional delimiter, all pages.
    pub fn list_with(&self, prefix: &str, delimiter: Option<&str>) -> Result<Listing> {
        let (bucket, prefix) = self.resolve(prefix)?;
        let start = Instant::now();
        let mut listing = Listing::default();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![("list-type".to_string(), "2".to_string())];
            if !prefix.is_empty() {
                query.push(("prefix".into(), prefix.clone()));
            }
            if let Some(d) = delimiter {
                query.push(("delimiter".into(), d.to_string()));
            }
            if let Some(t) = &token {
                query.push(("continuation-token".into(), t.clone()));
            }
            let mut resp = self.send("GET", &bucket, "", &query, Payload::Empty, &[])?;
            let body = read_text(&mut resp, &bucket, &prefix)?;
            let page = xml::parse_listing(&body).map_err(|e| {
                Error::Transport(format!("bad listing XML from s3://{bucket}/{prefix}: {e}"))
            })?;
            listing.objects.extend(page.objects);
            listing.prefixes.extend(page.prefixes);
            match page.next {
                Some(t) => token = Some(t),
                None => break,
            }
        }
        let n = listing.objects.len();
        log::info!(target: "s3pe", "✔ {n} object{} under s3://{bucket}/{prefix} ({:.1} s)", if n == 1 { "" } else { "s" }, start.elapsed().as_secs_f64());
        Ok(listing)
    }

    /// Deletes an object. Deleting what does not exist is not an error.
    pub fn delete(&self, key: &str) -> Result<()> {
        let (bucket, key) = self.resolve_object(key, "delete")?;
        self.send("DELETE", &bucket, &key, &[], Payload::Empty, &[])?;
        log::info!(target: "s3pe", "✖ s3://{bucket}/{key}");
        Ok(())
    }

    /// Deletes several objects with `DeleteObjects`, 1000 per request
    /// (D17). Keys may name different buckets. Any object the store could
    /// not delete is reported in one error.
    pub fn delete_many<S: AsRef<str>>(&self, keys: &[S]) -> Result<()> {
        let mut by_bucket: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for k in keys {
            let (b, key) = self.resolve_object(k.as_ref(), "delete")?;
            by_bucket.entry(b).or_default().push(key);
        }
        let mut failed: Vec<String> = Vec::new();
        for (bucket, keys) in &by_bucket {
            for chunk in keys.chunks(1000) {
                let mut body = String::from("<Delete><Quiet>true</Quiet>");
                for k in chunk {
                    body.push_str(&format!("<Object><Key>{}</Key></Object>", xml::escape(k)));
                }
                body.push_str("</Delete>");
                let bytes = body.as_bytes();
                let headers = [
                    ("content-type".to_string(), "application/xml".to_string()),
                    (
                        "content-md5".to_string(),
                        sigv4::base64(&Md5::digest(bytes)),
                    ),
                ];
                let q = [("delete".to_string(), String::new())];
                let mut resp =
                    self.send("POST", bucket, "", &q, Payload::Bytes(bytes), &headers)?;
                let text = read_text(&mut resp, bucket, "")?;
                for (k, code, msg) in xml::parse_delete_errors(&text) {
                    failed.push(format!("s3://{bucket}/{k} ({code}: {msg})"));
                }
            }
            log::info!(target: "s3pe", "✖ {} object{} in s3://{bucket}/", keys.len(), if keys.len() == 1 { "" } else { "s" });
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Http {
                status: 200,
                code: "DeleteErrors".into(),
                message: format!("{} not deleted: {}", failed.len(), failed.join(", ")),
                bucket: by_bucket.keys().next().cloned().unwrap_or_default(),
                key: String::new(),
            })
        }
    }

    /// Copies an object inside the store (`CopyObject`, D17). Both keys may
    /// be `s3://` URLs; the copy keeps the source's metadata.
    pub fn copy(&self, from: &str, to: &str) -> Result<Transfer> {
        let (sb, sk) = self.resolve_object(from, "copy")?;
        let (bucket, key) = self.resolve_object(to, "copy to")?;
        let start = Instant::now();
        let source = format!(
            "/{}/{}",
            sigv4::uri_encode(&sb, false),
            sigv4::uri_encode(&sk, true)
        );
        let headers = [("x-amz-copy-source".to_string(), source)];
        let mut resp = self.send("PUT", &bucket, &key, &[], Payload::Empty, &headers)?;
        let body = read_text(&mut resp, &bucket, &key)?;
        let (code, message) = xml::parse_error(&body);
        if !code.is_empty() {
            return Err(Error::Http {
                status: 200,
                code,
                message,
                bucket,
                key,
            });
        }
        let etag = xml::first_text(&body, "ETag").unwrap_or_default();
        let size = self
            .head(&format!("s3://{bucket}/{key}"))?
            .map(|o| o.size)
            .unwrap_or(0);
        let t = Transfer {
            bucket,
            key,
            size,
            etag,
            elapsed: start.elapsed(),
        };
        log::info!(target: "s3pe", "⧉ s3://{sb}/{sk} → {} ({}, {:.1} s)", t.url(), format_size(size), t.elapsed.as_secs_f64());
        Ok(t)
    }

    /// Metadata of an object, or `None` when it does not exist.
    pub fn head(&self, key: &str) -> Result<Option<Object>> {
        let (bucket, key) = self.resolve_object(key, "inspect")?;
        match self.send("HEAD", &bucket, &key, &[], Payload::Empty, &[]) {
            Ok(resp) => {
                let mut metadata = BTreeMap::new();
                for (name, value) in resp.headers() {
                    if let Some(short) = name.as_str().strip_prefix("x-amz-meta-") {
                        metadata
                            .insert(short.to_string(), value.to_str().unwrap_or("").to_string());
                    }
                }
                Ok(Some(Object {
                    key,
                    size: content_length(&resp).unwrap_or(0),
                    last_modified: header(&resp, "last-modified"),
                    etag: etag_of(&resp),
                    content_type: Some(header(&resp, "content-type")).filter(|s| !s.is_empty()),
                    metadata,
                }))
            }
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
        let mut resp = self.send("GET", "", "", &[], Payload::Empty, &[])?;
        let body = read_text(&mut resp, "", "")?;
        xml::parse_buckets(&body).map_err(|e| Error::Transport(format!("bad bucket list XML: {e}")))
    }

    /// A URL that reads (`GET`) or writes (`PUT`) the object without
    /// credentials for `expires` seconds, at most seven days (D16).
    pub fn presign(&self, key: &str, method: &str, expires: Duration) -> Result<String> {
        let (bucket, key) = self.resolve_object(key, "pre-sign")?;
        let method = method.to_ascii_uppercase();
        if method != "GET" && method != "PUT" {
            return Err(Error::Config(format!(
                "presign: method must be GET or PUT, not {method}"
            )));
        }
        let secs = expires.as_secs().clamp(1, MAX_PRESIGN_SECS);
        let path = format!(
            "/{}/{}",
            sigv4::uri_encode(&bucket, false),
            sigv4::uri_encode(&key, true)
        );
        let qs = sigv4::presign(
            &PresignRequest {
                method: &method,
                host: &self.host,
                path: &path,
                query: &[],
                amz_date: &sigv4::amz_date_now(),
                expires: secs,
            },
            &self.credentials,
            &self.region,
        );
        Ok(format!("{}{}?{}", self.endpoint, path, qs))
    }

    /// Uploads every file under `dir` whose remote ETag differs from the
    /// local one or which is missing; see [`SyncOptions`] (D18).
    pub fn sync_up(
        &self,
        dir: impl AsRef<Path>,
        prefix: &str,
        opts: &SyncOptions,
    ) -> Result<SyncReport> {
        let dir = dir.as_ref();
        let (bucket, prefix) = self.resolve(prefix)?;
        let prefix = normalize_prefix(&prefix);
        let start = Instant::now();
        let mut local = Vec::new();
        walk(dir, dir, &mut local)?;
        local.sort();
        let remote: BTreeMap<String, Object> = self
            .list(&format!("s3://{bucket}/{prefix}"))?
            .into_iter()
            .filter_map(|o| {
                let rel = o.key.strip_prefix(&prefix)?.to_string();
                Some((rel, o))
            })
            .collect();

        let mut report = SyncReport::default();
        let mut queue: Vec<String> = local
            .iter()
            .filter(|r| Some(*r) != opts.last.as_ref())
            .cloned()
            .collect();
        if let Some(last) = &opts.last {
            if local.contains(last) {
                queue.push(last.clone());
            } else {
                return Err(Error::Config(format!(
                    "last = {last}: no such file under {}",
                    dir.display()
                )));
            }
        }
        for rel in queue {
            let path = dir.join(&rel);
            let same = match remote.get(&rel) {
                Some(o) => {
                    let len = std::fs::metadata(&path)?.len();
                    len == o.size
                        && expected_etag(&path, self.multipart_threshold, self.part_size)?
                            .eq_ignore_ascii_case(&o.etag)
                }
                None => false,
            };
            if same {
                report.skipped.push(rel);
            } else {
                self.upload(&path, &format!("s3://{bucket}/{prefix}{rel}"), None)?;
                report.transferred.push(rel);
            }
        }
        if opts.delete {
            let gone: Vec<String> = remote
                .keys()
                .filter(|r| !local.contains(r))
                .map(|r| format!("s3://{bucket}/{prefix}{r}"))
                .collect();
            if !gone.is_empty() {
                self.delete_many(&gone)?;
                report.deleted = remote
                    .keys()
                    .filter(|r| !local.contains(r))
                    .cloned()
                    .collect();
            }
        }
        report.elapsed = start.elapsed();
        log::info!(target: "s3pe", "✔ sync {} → s3://{bucket}/{prefix}: {} uploaded, {} unchanged, {} deleted ({:.1} s)", dir.display(), report.transferred.len(), report.skipped.len(), report.deleted.len(), report.elapsed.as_secs_f64());
        Ok(report)
    }

    /// Downloads every object under `prefix` whose local copy is missing or
    /// differs; with `delete`, removes local files that are not remote (D18).
    pub fn sync_down(
        &self,
        prefix: &str,
        dir: impl AsRef<Path>,
        opts: &SyncOptions,
    ) -> Result<SyncReport> {
        let dir = dir.as_ref();
        let (bucket, prefix) = self.resolve(prefix)?;
        let prefix = normalize_prefix(&prefix);
        let start = Instant::now();
        std::fs::create_dir_all(dir)?;
        let remote: Vec<(String, Object)> = self
            .list(&format!("s3://{bucket}/{prefix}"))?
            .into_iter()
            .filter_map(|o| {
                let rel = o.key.strip_prefix(&prefix)?.to_string();
                Some((rel, o))
            })
            .filter(|(r, _)| !r.is_empty() && !r.ends_with('/'))
            .collect();
        let mut report = SyncReport::default();
        for (rel, o) in &remote {
            let path = dir.join(rel);
            let same = path.is_file()
                && std::fs::metadata(&path)?.len() == o.size
                && expected_etag(&path, self.multipart_threshold, self.part_size)?
                    .eq_ignore_ascii_case(&o.etag);
            if same {
                report.skipped.push(rel.clone());
            } else {
                self.download(&format!("s3://{bucket}/{}", o.key), &path)?;
                report.transferred.push(rel.clone());
            }
        }
        if opts.delete {
            let mut local = Vec::new();
            walk(dir, dir, &mut local)?;
            for rel in local {
                if !remote.iter().any(|(r, _)| *r == rel) {
                    std::fs::remove_file(dir.join(&rel))?;
                    report.deleted.push(rel);
                }
            }
        }
        report.elapsed = start.elapsed();
        log::info!(target: "s3pe", "✔ sync s3://{bucket}/{prefix} → {}: {} downloaded, {} unchanged, {} deleted ({:.1} s)", dir.display(), report.transferred.len(), report.skipped.len(), report.deleted.len(), report.elapsed.as_secs_f64());
        Ok(report)
    }

    fn send(
        &self,
        method: &str,
        bucket: &str,
        key: &str,
        query: &[(String, String)],
        payload: Payload,
        headers: &[(String, String)],
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
        self.send_hashed(method, bucket, key, query, payload, headers, &hashes)
    }

    #[allow(clippy::too_many_arguments)]
    fn send_hashed(
        &self,
        method: &str,
        bucket: &str,
        key: &str,
        query: &[(String, String)],
        payload: Payload,
        headers: &[(String, String)],
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
        // Every x-amz-* header is signed; the others travel unsigned.
        let signed_extra: Vec<(String, String)> = headers
            .iter()
            .filter(|(k, _)| k.starts_with("x-amz-"))
            .cloned()
            .collect();
        let unsigned: Vec<(String, String)> = headers
            .iter()
            .filter(|(k, _)| !k.starts_with("x-amz-"))
            .cloned()
            .collect();

        let mut tries = 0;
        loop {
            tries += 1;
            let amz_date = sigv4::amz_date_now();
            let mut signed = sigv4::sign(
                &SignRequest {
                    method,
                    host: &self.host,
                    path: &path,
                    query,
                    payload_hash: &hashes.sha256,
                    amz_date: &amz_date,
                    extra_headers: &signed_extra,
                },
                &self.credentials,
                &self.region,
            );
            signed.extend(unsigned.iter().cloned());
            let attempt = self.attempt(method, &url, &signed, &payload, hashes.len);
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
        len: u64,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let transport = |e: ureq::Error| Error::Transport(format!("{method} {url}: {e}"));
        match method {
            "PUT" | "POST" => {
                let mut req = if method == "PUT" {
                    self.agent.put(url)
                } else {
                    self.agent.post(url)
                };
                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
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
            let (code, message) = xml::parse_error(&body);
            if code.is_empty() && message.is_empty() && !body.trim().is_empty() {
                // not an S3 error document (a proxy page, plain text): keep a glimpse of it
                let snippet: String = body.trim().chars().take(200).collect();
                (String::new(), snippet)
            } else {
                (code, message)
            }
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

/// A prefix that is empty or ends with `/`.
fn normalize_prefix(p: &str) -> String {
    if p.is_empty() || p.ends_with('/') {
        p.to_string()
    } else {
        format!("{p}/")
    }
}

/// Relative paths (with `/`) of every file under `dir`, recursively.
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path: PathBuf = entry.path();
        if path.is_dir() {
            walk(root, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if !rel.ends_with(".s3pe-part")
                && !rel
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .starts_with(".DS_Store")
            {
                out.push(rel);
            }
        }
    }
    Ok(())
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

    #[test]
    fn multipart_etags() {
        // md5("a") = 0cc175b9c0f1b6a831c399e269772661, md5("b") = 92eb5ffee6ae2fec3ad71c777531578f
        let e = multipart_etag(&[
            "0cc175b9c0f1b6a831c399e269772661".into(),
            "92eb5ffee6ae2fec3ad71c777531578f".into(),
        ]);
        assert!(e.ends_with("-2") && e.len() == 34, "{e}");
        let dir = std::env::temp_dir().join(format!("s3pe-etag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("f.bin");
        std::fs::write(&f, vec![7u8; 3000]).unwrap();
        assert_eq!(
            expected_etag(&f, 5000, MIN_PART_SIZE).unwrap(),
            hex::encode(Md5::digest(vec![7u8; 3000]))
        );
        let parts = expected_etag(&f, 1000, MIN_PART_SIZE).unwrap();
        // part_size is clamped by the builder, not here: 3000 bytes in one 5 MiB part
        assert!(parts.ends_with("-1"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prefixes_and_walk() {
        assert_eq!(normalize_prefix(""), "");
        assert_eq!(normalize_prefix("a"), "a/");
        assert_eq!(normalize_prefix("a/"), "a/");
        let dir = std::env::temp_dir().join(format!("s3pe-walk-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub/deeper")).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("sub/deeper/b.txt"), "b").unwrap();
        std::fs::write(dir.join("c.s3pe-part"), "x").unwrap();
        let mut out = Vec::new();
        walk(&dir, &dir, &mut out).unwrap();
        out.sort();
        assert_eq!(out, vec!["a.txt", "sub/deeper/b.txt"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
