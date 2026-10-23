# s3pe

[![crates.io](https://img.shields.io/crates/v/s3pe.svg)](https://crates.io/crates/s3pe)
[![docs.rs](https://img.shields.io/docsrs/s3pe)](https://docs.rs/s3pe)
[![CI](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml/badge.svg)](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A small, dependable S3 client and command line, made for **s3.pe.gov.br**
(the [Garage](https://garagehq.deuxfleurs.fr) object store of the State of
Pernambuco, Brazil) and usable with any S3-style store.

**Website:** <https://strategicprojects.github.io/s3pe/> ·
**API reference:** <https://docs.rs/s3pe>

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

// credentials and endpoint come from the environment
let manifest = read_s3("s3://iaempe/dados/metadata.json")?;
write_s3("s3://iaempe/nota/figura1.png", std::fs::read("figura1.png")?, None)?;
for object in list_s3("s3://iaempe/dados/")? {
    println!("{:>10} {}", object.size, object.key);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

With an explicit client:

```rust
let s3 = s3pe::Client::builder()
    .endpoint("http://127.0.0.1:9003")
    .credentials("GK…", "…")
    .bucket("iaempe")
    .build()?;
let bytes = s3.read("dados/metadata.json")?;
s3.upload("painel.parquet", "etl/painel.parquet", None)?;   // streaming
s3.download("dados/pe_municipios.geojson", "pe.geojson")?;  // streaming, .s3pe-part then rename
if let Some(meta) = s3.head("dados/metadata.json")? {
    println!("{} bytes, {}", meta.size, meta.content_type.unwrap_or_default());
}
s3.delete("etl/old.parquet")?;
# Ok::<(), s3pe::Error>(())
```

### Configuration

| Setting | Environment | Default |
|---|---|---|
| endpoint | `S3PE_ENDPOINT`, then `AWS_ENDPOINT_URL` | `https://s3.pe.gov.br` |
| region | `AWS_REGION`, then `AWS_DEFAULT_REGION` | `us-east-1` |
| credentials | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` (`AWS_SESSION_TOKEN`) | required |
| bucket | `S3PE_BUCKET` | none: use `s3://bucket/key` |

Path-style URLs always. Keys are bare (relative to the bucket) or full
`s3://bucket/key` URLs.

### What "dependable" means

- AWS Signature V4 over the real SHA-256 of the payload: no
  `UNSIGNED-PAYLOAD`, no `aws-chunked` (which Garage 2.4 rejects).
- Connection failures and HTTP 429/500/502/503/504 are retried twice, with
  a 1 s then 2 s pause. Other errors come back at once, typed:
  `Error::NotFound`, `Error::AccessDenied`, `Error::Http { status, code, .. }`,
  `Error::Transport`, `Error::Config`, `Error::Integrity`.
- After an upload the server's ETag is compared with the local MD5; after a
  download the byte count with `Content-Length` and, when the ETag is an
  MD5, the MD5 of what arrived.
- One line per transfer through the [`log`](https://docs.rs/log) facade
  (target `s3pe`): `↑ s3://iaempe/etl/painel.parquet (12.3 MB, 0.8 s)`.

## Command line

```bash
export AWS_ACCESS_KEY_ID=GK...  AWS_SECRET_ACCESS_KEY=...   # or ~/.Renviron, .env, ...

s3pe buckets
s3pe ls s3://iaempe/dados/              # keys
s3pe ls -l s3://iaempe/dados/           # date, size, etag, key
s3pe cat s3://iaempe/dados/metadata.json | jq .ultimo_mes
s3pe get s3://iaempe/dados/pe_municipios.geojson        # to ./pe_municipios.geojson
s3pe put painel.parquet s3://iaempe/etl/                # keeps the file name
s3pe head s3://iaempe/etl/painel.parquet
s3pe rm s3://iaempe/etl/old.parquet
s3pe --endpoint http://127.0.0.1:9003 -b iaempe ls dados/   # inside the server
```

Exit codes: 0 ok, 2 not found, 3 access denied, 4 configuration, 1 anything
else. `-q` or `S3PE_QUIET=1` keeps stderr to errors.

## The family

The same package exists for Python (`pip install s3pe`), R
(`install.packages("s3pe")`) and Go (`go get github.com/StrategicProjects/s3pe`),
all in [one repository](https://github.com/StrategicProjects/s3pe), with one
version and one changelog. The Python package is a binding on this crate.
