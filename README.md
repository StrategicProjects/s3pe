# s3pe

[![CI](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml/badge.svg)](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/s3pe.svg)](https://crates.io/crates/s3pe)
[![PyPI](https://img.shields.io/pypi/v/s3pe.svg)](https://pypi.org/project/s3pe/)
[![CRAN](https://www.r-pkg.org/badges/version/s3pe)](https://CRAN.R-project.org/package=s3pe)
[![Go Reference](https://pkg.go.dev/badge/github.com/StrategicProjects/s3pe.svg)](https://pkg.go.dev/github.com/StrategicProjects/s3pe)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![DOI](https://zenodo.org/badge/DOI/10.5281/zenodo.23606939.svg)](https://doi.org/10.5281/zenodo.23606939)

Read and write data on any S3-compatible object store (AWS S3, Garage,
MinIO, Ceph, …) with the same nine verbs in R, Python, Rust and Go: the
same defaults, the same signing, the same retries and the same integrity
checks. The endpoint and the credentials come from the environment; the
format comes from the file extension; every transfer prints one line.

**Website:** <https://strategicprojects.github.io/s3pe/> ·
[Português](README.pt-BR.md)

| Edition | Install | Docs |
|---|---|---|
| R | `install.packages("s3pe")` · `pak::pak("StrategicProjects/s3pe/r")` | [pkgdown](https://strategicprojects.github.io/s3pe/r/) |
| Python | `pip install s3pe` (`"s3pe[pandas]"` for tables) | [site](https://strategicprojects.github.io/s3pe/python/) · [API](https://strategicprojects.github.io/s3pe/python/api/) |
| Rust | `cargo add s3pe` · `cargo install s3pe` (the `s3pe` command) | [site](https://strategicprojects.github.io/s3pe/rust/) · [docs.rs](https://docs.rs/s3pe) |
| Go | `go get github.com/StrategicProjects/s3pe` | [site](https://strategicprojects.github.io/s3pe/go/) · [pkg.go.dev](https://pkg.go.dev/github.com/StrategicProjects/s3pe) |

## The nine verbs

```r
library(s3pe)                                           # R
use_endpoint("https://s3.example.org"); use_bucket("my-bucket")
meta  <- read_s3("data/manifest.json")                  # list, from JSON
sales <- read_s3("series/sales.csv.gz")                 # tibble
write_s3(sales, "reports/sales.parquet")
list_s3("data/")
```

```python
import s3pe                                             # Python
meta  = s3pe.read_s3("s3://my-bucket/data/manifest.json")          # dict
sales = s3pe.read_s3("series/sales.csv.gz", bucket="my-bucket")    # DataFrame
s3pe.write_s3(sales, "s3://my-bucket/reports/sales.parquet")
s3pe.list_s3("s3://my-bucket/data/")
```

```rust
let meta = s3pe::read_s3("s3://my-bucket/data/manifest.json")?;    // Rust: Vec<u8>
s3pe::upload_s3("sales.parquet", "s3://my-bucket/reports/sales.parquet", None)?;
for o in s3pe::list_s3("s3://my-bucket/data/")? { println!("{} {}", o.size, o.key); }
```

```go
meta, err := s3pe.ReadS3(ctx, "s3://my-bucket/data/manifest.json")  // Go: []byte
_, err = s3pe.UploadS3(ctx, "sales.parquet", "s3://my-bucket/reports/sales.parquet", "")
objects, err := s3pe.ListS3(ctx, "s3://my-bucket/data/")
```

```bash
s3pe ls s3://my-bucket/data/                                         # command line
s3pe get s3://my-bucket/data/regions.geojson
s3pe put sales.parquet s3://my-bucket/reports/
```

| Verb | Does |
|---|---|
| `read_s3` / `write_s3` | move an object in memory: decoded by extension in R and Python, bytes in Rust and Go |
| `download_s3` / `upload_s3` | move a file, streaming, written as `.s3pe-part` and renamed when complete |
| `list_s3` | every object under a prefix, all pages |
| `exists_s3` / `head_s3` | whether an object exists; its size, type, date and ETag |
| `delete_s3` | delete (idempotent) |
| `buckets_s3` | the buckets the credentials can see |
| `copy_s3` | server-side copy |
| `presign_s3` | a URL that works without credentials for a while (GET or PUT) |
| `sync_up_s3` / `sync_down_s3` | mirror a directory and a prefix by ETag, a manifest last |

`write_s3`/`upload_s3` take `metadata` and `headers` (so SSE is one header
away where the store supports it); `delete_s3` takes many keys at once;
`list_s3(delimiter = "/")` lists one level. Files above 64 MiB go up as a
multipart upload, parts in parallel, aborted cleanly on failure. No ACLs
and no versioning, by design (DESIGN.md D19). Every edition also has a
`Client` (R: `s3_config()`) for explicit settings.

## Architecture

<img src="site/assets/architecture.svg" alt="Four editions (R on httr2 and openssl; Python as a PyO3 binding on the Rust crate; the Rust crate on ureq and rustls with the s3pe command line; Go on net/http) share one specification: resolve the key, sign with SigV4, retry, verify, log one line. All talk to any S3-compatible store through path-style URLs." width="100%" />

## Configuration

Argument, then environment, then default:

| Setting | Environment | Default |
|---|---|---|
| endpoint | `S3PE_ENDPOINT`, then `AWS_ENDPOINT_URL` | required (`https://host[:port]`) |
| region | `AWS_REGION`, then `AWS_DEFAULT_REGION` | `us-east-1` |
| credentials | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` | required |
| bucket | `S3PE_BUCKET` | none: write `s3://bucket/key` |
| quiet | `S3PE_QUIET=1` | one log line per transfer |

In R the usual place is `~/.Renviron` (or `use_endpoint()` / `use_bucket()`
for the session); in Python and the CLI any `.env` loader or the shell.
Never put the key in a script or a repository.

## What happens in every call

<img src="site/assets/request-flow.svg" alt="Six steps: resolve the key, hash the payload, sign with SigV4, send with Content-Length, retry on 429 and 5xx after 1 and 2 seconds, verify the ETag and Content-Length. Success prints one log line; errors are typed: not found, access denied, HTTP, transport, configuration, integrity." width="100%" />

## Dependable by default

- **Signing**: AWS Signature V4, header style, over the real SHA-256 of the
  payload. No `aws-chunked` (which some stores reject), no unsigned
  payloads. The four signers reproduce the AWS documentation vector.
- **Retries**: connection failures and HTTP 429/500/502/503/504, 3 tries,
  1 s then 2 s. Never on other 4xx.
- **Typed errors**: not found, access denied, other HTTP (with the S3 code),
  transport, configuration, integrity. `exists_s3`/`head_s3` turn 404 into
  `FALSE`/`None`/`nil`.
- **Integrity**: ETag vs MD5 after uploads; `Content-Length` and MD5 after
  downloads.
- **Tests**: each edition runs against a local fake S3 that verifies every
  signature, and against a real store when `S3PE_ENDPOINT` and
  `S3PE_TEST_BUCKET` are set.

Design decisions are recorded in [DESIGN.md](DESIGN.md); changes in
[CHANGELOG.md](CHANGELOG.md).

## Repository layout

```
r/        R package (CRAN)                  rust/    Rust crate and CLI (crates.io)
python/   Python package, PyO3 on the crate  *.go     Go module (this directory)
site/     landing pages (GitHub Pages)       DESIGN.md decisions shared by the four
```

## Authors

André Leite, Marcos Wasiliew, Hugo Vasconcelos, Carlos Amorim, Diogo Bezerra
and Júlia Nascimento Barreto. MIT license. Cite with [CITATION.cff](CITATION.cff).

## Made in Pernambuco 🌵

s3pe is developed in Recife, Brazil.
