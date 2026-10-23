//! A small, dependable S3 client, made for `s3.pe.gov.br` (the Garage
//! object store of the State of Pernambuco, Brazil) and usable with any
//! S3-style store.
//!
//! Nine verbs, one client, nothing else to learn:
//!
//! ```no_run
//! use s3pe::{read_s3, write_s3, list_s3};
//!
//! // credentials and endpoint come from the environment (see below)
//! let manifest = read_s3("s3://iaempe/dados/metadata.json")?;
//! write_s3("s3://iaempe/nota/figura1.png", std::fs::read("figura1.png")?, None)?;
//! for object in list_s3("s3://iaempe/dados/")? {
//!     println!("{} {}", object.size, object.key);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Or with an explicit [`Client`]:
//!
//! ```no_run
//! let s3 = s3pe::Client::builder()
//!     .endpoint("https://s3.pe.gov.br")
//!     .credentials("GK…", "…")
//!     .bucket("iaempe")
//!     .build()?;
//! let bytes = s3.read("dados/metadata.json")?;
//! s3.upload("painel.parquet", "etl/painel.parquet", None)?;
//! # Ok::<(), s3pe::Error>(())
//! ```
//!
//! # Configuration
//!
//! Anything not passed to the builder comes from the environment:
//!
//! | Setting | Variables | Default |
//! |---|---|---|
//! | endpoint | `S3PE_ENDPOINT`, `AWS_ENDPOINT_URL` | `https://s3.pe.gov.br` |
//! | region | `AWS_REGION`, `AWS_DEFAULT_REGION` | `us-east-1` |
//! | credentials | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` | required |
//! | bucket | `S3PE_BUCKET` | none: use `s3://bucket/key` |
//!
//! URLs are always path-style (`https://host/bucket/key`).
//!
//! # What "dependable" means here
//!
//! - Requests are signed with AWS Signature V4 over the real SHA-256 of the
//!   payload (no `aws-chunked`, which Garage rejects).
//! - Connection failures and HTTP 429/500/502/503/504 are retried twice,
//!   with a 1 s then 2 s pause. Other errors are reported at once, typed
//!   ([`Error::NotFound`], [`Error::AccessDenied`], …).
//! - After an upload the server's ETag is compared with the local MD5;
//!   after a download the byte count is compared with `Content-Length`
//!   and, when the ETag is an MD5, with the MD5 of what arrived.
//! - Downloads to a file are written to a `.s3pe-part` file and renamed
//!   when complete.
//! - One line per transfer is logged through the [`log`] facade (target
//!   `s3pe`): `↑ s3://bucket/key (12.3 MB, 0.8 s)`.
//!
//! # Command line
//!
//! `cargo install s3pe` also installs the `s3pe` command:
//!
//! ```text
//! s3pe ls s3://iaempe/dados/
//! s3pe get s3://iaempe/dados/metadata.json
//! s3pe put painel.parquet s3://iaempe/etl/
//! s3pe cat s3://iaempe/dados/metadata.json | jq .ultimo_mes
//! s3pe rm s3://iaempe/etl/old.parquet
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
    content_type_for, format_size, Bucket, Client, ClientBuilder, Object, Transfer,
    DEFAULT_ENDPOINT, DEFAULT_MAX_TRIES, DEFAULT_REGION, DEFAULT_TIMEOUT,
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
