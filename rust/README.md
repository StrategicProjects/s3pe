# s3pe

[![crates.io](https://img.shields.io/crates/v/s3pe.svg)](https://crates.io/crates/s3pe)
[![docs.rs](https://img.shields.io/docsrs/s3pe)](https://docs.rs/s3pe)
[![CI](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml/badge.svg)](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![DOI](https://zenodo.org/badge/DOI/10.5281/zenodo.23606939.svg)](https://doi.org/10.5281/zenodo.23606939)

A small, dependable client and command line for S3-compatible object
stores (AWS S3, Garage, MinIO, Ceph, …): nine verbs, SigV4 signing over the
real payload hash, retries that know when to stop, and an integrity check
on every transfer.

**Website:** <https://strategicprojects.github.io/s3pe/rust/> ·
**API reference:** <https://docs.rs/s3pe> · [all editions](https://strategicprojects.github.io/s3pe/)

## Install

```bash
cargo add s3pe                       # the library (and the CLI as a build target)
cargo add s3pe --no-default-features # the library only, without clap
cargo install s3pe                   # the `s3pe` command
```

Rust 1.85 or later. TLS by rustls with bundled roots: no system OpenSSL.

## Library

```rust
use s3pe::{read_s3, write_s3, list_s3};

// endpoint and credentials come from the environment
let manifest = read_s3("s3://my-bucket/data/manifest.json")?;
write_s3("s3://my-bucket/reports/figure1.png", std::fs::read("figure1.png")?, None)?;
for object in list_s3("s3://my-bucket/data/")? {
    println!("{:>10} {}", object.size, object.key);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

With an explicit client:

```rust
let s3 = s3pe::Client::builder()
    .endpoint("https://s3.example.org")
    .credentials("AKIA…", "…")
    .bucket("my-bucket")
    .build()?;
let bytes = s3.read("data/manifest.json")?;
s3.upload("sales.parquet", "etl/sales.parquet", None)?;      // streaming
s3.download("data/regions.geojson", "regions.geojson")?;     // streaming, .s3pe-part then rename
if let Some(meta) = s3.head("data/manifest.json")? {
    println!("{} bytes, {}", meta.size, meta.content_type.unwrap_or_default());
}
s3.delete("etl/old.parquet")?;
# Ok::<(), s3pe::Error>(())
```

### Configuration

| Setting | Environment | Default |
|---|---|---|
| endpoint | `S3PE_ENDPOINT`, then `AWS_ENDPOINT_URL` | required (`https://host[:port]`) |
| region | `AWS_REGION`, then `AWS_DEFAULT_REGION` | `us-east-1` |
| credentials | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` (`AWS_SESSION_TOKEN`) | required |
| bucket | `S3PE_BUCKET` | none: use `s3://bucket/key` |

Path-style URLs always (`https://host/bucket/key`), which every store
accepts. Keys are bare (relative to the bucket) or full `s3://bucket/key`
URLs.

### What happens in every call

<img src="https://raw.githubusercontent.com/StrategicProjects/s3pe/main/site/assets/request-flow.svg" alt="Resolve the key, hash the payload, sign with SigV4, send, retry on 429 and 5xx, verify the ETag and Content-Length; success logs one line, errors are typed." width="100%" />

### What "dependable" means

- AWS Signature V4 over the real SHA-256 of the payload: no
  `UNSIGNED-PAYLOAD`, no `aws-chunked` (which some stores reject).
- Connection failures and HTTP 429/500/502/503/504 are retried twice, with
  a 1 s then 2 s pause. Other errors come back at once, typed:
  `Error::NotFound`, `Error::AccessDenied`, `Error::Http { status, code, .. }`,
  `Error::Transport`, `Error::Config`, `Error::Integrity`.
- After an upload the server's ETag is compared with the local MD5; after a
  download the byte count with `Content-Length` and, when the ETag is an
  MD5, the MD5 of what arrived.
- One line per transfer through the [`log`](https://docs.rs/log) facade
  (target `s3pe`): `↑ s3://my-bucket/etl/sales.parquet (12.3 MB, 0.8 s)`.

## Command line

```bash
export S3PE_ENDPOINT=https://s3.example.org
export AWS_ACCESS_KEY_ID=...  AWS_SECRET_ACCESS_KEY=...

s3pe buckets
s3pe ls s3://my-bucket/data/              # keys
s3pe ls -l s3://my-bucket/data/           # date, size, etag, key
s3pe cat s3://my-bucket/data/manifest.json | jq .version
s3pe get s3://my-bucket/data/regions.geojson        # to ./regions.geojson
s3pe put sales.parquet s3://my-bucket/etl/           # keeps the file name
s3pe head s3://my-bucket/etl/sales.parquet
s3pe rm s3://my-bucket/etl/old.parquet
s3pe --endpoint http://127.0.0.1:9000 -b my-bucket ls data/   # a local MinIO or Garage
```

Exit codes: 0 ok, 2 not found, 3 access denied, 4 configuration, 1 anything
else. `-q` or `S3PE_QUIET=1` keeps stderr to errors.

## Architecture

<img src="https://raw.githubusercontent.com/StrategicProjects/s3pe/main/site/assets/architecture.svg" alt="The Rust crate (ureq, rustls, hmac, sha2, plus the s3pe command line) is one of four editions that share one specification; the Python package is a binding on it. All talk to any S3-compatible store." width="100%" />

## The family

The same package exists for Python (`pip install s3pe`), R
(`install.packages("s3pe")`) and Go (`go get github.com/StrategicProjects/s3pe`),
all in [one repository](https://github.com/StrategicProjects/s3pe), with one
version and one changelog. The Python package is a binding on this crate.

## Made in Pernambuco 🌵

s3pe is developed in Recife, Brazil. MIT license.
