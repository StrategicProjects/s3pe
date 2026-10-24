//! A small, dependable client for S3-compatible object stores (AWS S3,
//! Garage, MinIO, Ceph, …): nine verbs, SigV4 signing, retries and integrity
//! checks, plus the `s3pe` command line.
//!
//! Nine verbs, one client, nothing else to learn (plus `copy_s3`,
//! `delete_many_s3`, `list_dir_s3`, `presign_s3`, `sync_up_s3` and
//! `sync_down_s3` since 0.3.0):
//!
//! ```no_run
//! use s3pe::{read_s3, write_s3, list_s3};
//!
//! // endpoint and credentials come from the environment (see below)
//! let manifest = read_s3("s3://my-bucket/data/manifest.json")?;
//! write_s3("s3://my-bucket/reports/figure1.png", std::fs::read("figure1.png")?, None)?;
//! for object in list_s3("s3://my-bucket/data/")? {
//!     println!("{} {}", object.size, object.key);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Or with an explicit [`Client`]:
//!
//! ```no_run
//! let s3 = s3pe::Client::builder()
//!     .endpoint("https://s3.example.org")
//!     .credentials("AKIA…", "…")
//!     .bucket("my-bucket")
//!     .build()?;
//! let bytes = s3.read("data/manifest.json")?;
//! s3.upload("sales.parquet", "etl/sales.parquet", None)?;
//! # Ok::<(), s3pe::Error>(())
//! ```
//!
//! # What happens in every call
//!
//! ![Resolve the key, hash the payload, sign with SigV4, send, retry on 429 and 5xx, verify the ETag and Content-Length; success logs one line, errors are typed.](https://raw.githubusercontent.com/StrategicProjects/s3pe/main/site/assets/request-flow.svg)
//!
//! # Configuration
//!
//! Anything not passed to the builder comes from the environment:
//!
//! | Setting | Variables | Default |
//! |---|---|---|
//! | endpoint | `S3PE_ENDPOINT`, `AWS_ENDPOINT_URL` | required |
//! | region | `AWS_REGION`, `AWS_DEFAULT_REGION` | `us-east-1` |
//! | credentials | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` | required |
//! | bucket | `S3PE_BUCKET` | none: use `s3://bucket/key` |
//!
//! URLs are always path-style (`https://host/bucket/key`).
//!
//! # What "dependable" means here
//!
//! - Requests are signed with AWS Signature V4 over the real SHA-256 of the
//!   payload (no `aws-chunked`, which some stores, Garage among them, reject).
//! - Connection failures and HTTP 429/500/502/503/504 are retried twice,
//!   with a 1 s then 2 s pause. Other errors are reported at once, typed
//!   ([`Error::NotFound`], [`Error::AccessDenied`], …).
//! - After an upload the server's ETag is compared with the local MD5;
//!   after a download the byte count is compared with `Content-Length`
//!   and, when the ETag is an MD5, with the MD5 of what arrived.
//! - Downloads to a file are written to a `.s3pe-part` file and renamed
//!   when complete.
//! - Bodies above 64 MiB go up as a multipart upload, 16 MiB parts, four
//!   at a time, aborted on failure; the final ETag is checked too.
//! - One line per transfer is logged through the [`log`] facade (target
//!   `s3pe`): `↑ s3://bucket/key (12.3 MB, 0.8 s)`.
//!
//! # Command line
//!
//! `cargo install s3pe` also installs the `s3pe` command:
//!
//! ```text
//! s3pe ls s3://my-bucket/data/
//! s3pe get s3://my-bucket/data/manifest.json
//! s3pe put sales.parquet s3://my-bucket/etl/
//! s3pe cat s3://my-bucket/data/manifest.json | jq .version
//! s3pe rm s3://my-bucket/etl/old.parquet s3://my-bucket/etl/older.parquet
//! s3pe cp s3://my-bucket/etl/sales.parquet s3://my-bucket/archive/sales-2026.parquet
//! s3pe presign s3://my-bucket/reports/report.pdf --expires 3600
//! s3pe sync ./out s3://my-bucket/reports/ --last manifest.json
//! ```
//!
//! The same package exists for Python (`pip install s3pe`), R
//! (`install.packages("s3pe")`) and Go (`go get github.com/StrategicProjects/s3pe`).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod client;
mod error;
pub mod sigv4;
mod xml;

pub use client::{
    content_type_for, expected_etag, format_size, multipart_etag, Bucket, Client, ClientBuilder,
    Listing, Object, PutOptions, SyncOptions, SyncReport, Transfer, DEFAULT_MAX_TRIES,
    DEFAULT_MULTIPART_THRESHOLD, DEFAULT_PARALLEL, DEFAULT_PART_SIZE, DEFAULT_REGION,
    DEFAULT_TIMEOUT, MAX_PRESIGN_SECS, MIN_PART_SIZE,
};
pub use error::{Error, Result};

use std::path::Path;

/// Reads an object into memory, with a client built from the environment.
pub fn read_s3(key: &str) -> Result<Vec<u8>> {
    Client::from_env()?.read(key)
}

/// Writes bytes to an object, with a client built from the environment.
pub fn write_s3(key: &str, body: impl AsRef<[u8]>, content_type: Option<&str>) -> Result<Transfer> {
    Client::from_env()?.write(key, body, content_type)
}

/// Downloads an object to a file, with a client built from the environment.
pub fn download_s3(key: &str, path: impl AsRef<Path>) -> Result<Transfer> {
    Client::from_env()?.download(key, path)
}

/// Uploads a file, with a client built from the environment.
pub fn upload_s3(
    path: impl AsRef<Path>,
    key: &str,
    content_type: Option<&str>,
) -> Result<Transfer> {
    Client::from_env()?.upload(path, key, content_type)
}

/// Lists the objects under a prefix, with a client built from the environment.
pub fn list_s3(prefix: &str) -> Result<Vec<Object>> {
    Client::from_env()?.list(prefix)
}

/// Deletes an object, with a client built from the environment.
pub fn delete_s3(key: &str) -> Result<()> {
    Client::from_env()?.delete(key)
}

/// Whether an object exists, with a client built from the environment.
pub fn exists_s3(key: &str) -> Result<bool> {
    Client::from_env()?.exists(key)
}

/// Metadata of an object (`None` if absent), with a client built from the environment.
pub fn head_s3(key: &str) -> Result<Option<Object>> {
    Client::from_env()?.head(key)
}

/// The buckets the credentials can see, with a client built from the environment.
pub fn buckets_s3() -> Result<Vec<Bucket>> {
    Client::from_env()?.buckets()
}

/// Copies an object inside the store, with a client built from the environment.
pub fn copy_s3(from: &str, to: &str) -> Result<Transfer> {
    Client::from_env()?.copy(from, to)
}

/// Deletes several objects in batches, with a client built from the environment.
pub fn delete_many_s3<S: AsRef<str>>(keys: &[S]) -> Result<()> {
    Client::from_env()?.delete_many(keys)
}

/// One level of a prefix (objects and "folders"), with a client built from the environment.
pub fn list_dir_s3(prefix: &str) -> Result<Listing> {
    Client::from_env()?.list_dir(prefix)
}

/// A pre-signed URL, with a client built from the environment.
pub fn presign_s3(key: &str, method: &str, expires: std::time::Duration) -> Result<String> {
    Client::from_env()?.presign(key, method, expires)
}

/// Uploads a directory to a prefix, with a client built from the environment.
pub fn sync_up_s3(dir: impl AsRef<Path>, prefix: &str, opts: &SyncOptions) -> Result<SyncReport> {
    Client::from_env()?.sync_up(dir, prefix, opts)
}

/// Downloads a prefix to a directory, with a client built from the environment.
pub fn sync_down_s3(prefix: &str, dir: impl AsRef<Path>, opts: &SyncOptions) -> Result<SyncReport> {
    Client::from_env()?.sync_down(prefix, dir, opts)
}
