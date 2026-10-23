# s3pe

[![CI](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml/badge.svg)](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/s3pe.svg)](https://crates.io/crates/s3pe)
[![PyPI](https://img.shields.io/pypi/v/s3pe.svg)](https://pypi.org/project/s3pe/)
[![CRAN](https://www.r-pkg.org/badges/version/s3pe)](https://CRAN.R-project.org/package=s3pe)
[![Go Reference](https://pkg.go.dev/badge/github.com/StrategicProjects/s3pe.svg)](https://pkg.go.dev/github.com/StrategicProjects/s3pe)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Read and write data on **s3.pe.gov.br**, the S3 object store of the State of
Pernambuco, Brazil (a [Garage](https://garagehq.deuxfleurs.fr) server), or on
any S3-style store. The same nine verbs in R, Python, Rust and Go, with the
same defaults, the same signing, the same retries and the same integrity
checks. Credentials come from two environment variables; the format comes
from the file extension; every transfer prints one line.

**Website:** <https://strategicprojects.github.io/s3pe/> ·
[Português](README.pt-BR.md)

| Edition | Install | Docs |
|---|---|---|
| R | `install.packages("s3pe")` · `pak::pak("StrategicProjects/s3pe/r")` | [pkgdown](https://strategicprojects.github.io/s3pe/r/) |
| Python | `pip install s3pe` (`"s3pe[pandas]"` for tables) | [API](https://strategicprojects.github.io/s3pe/python/) |
| Rust | `cargo add s3pe` · `cargo install s3pe` (the `s3pe` command) | [docs.rs](https://docs.rs/s3pe) |
| Go | `go get github.com/StrategicProjects/s3pe` | [pkg.go.dev](https://pkg.go.dev/github.com/StrategicProjects/s3pe) |

## The nine verbs

```r
library(s3pe)                                         # R
meta <- read_s3("s3://iaempe/dados/metadata.json")    # list, from JSON
icms <- read_s3("series/estado/icms.csv.gz", bucket = "horizonte-fiscal")  # tibble
write_s3(icms, "s3://iaempe/nota/icms.parquet")
list_s3("s3://iaempe/dados/")
```

```python
import s3pe                                           # Python
meta = s3pe.read_s3("s3://iaempe/dados/metadata.json")          # dict
icms = s3pe.read_s3("series/estado/icms.csv.gz", bucket="horizonte-fiscal")  # DataFrame
s3pe.write_s3(icms, "s3://iaempe/nota/icms.parquet")
s3pe.list_s3("s3://iaempe/dados/")
```

```rust
let meta = s3pe::read_s3("s3://iaempe/dados/metadata.json")?;    // Rust: Vec<u8>
s3pe::upload_s3("icms.parquet", "s3://iaempe/nota/icms.parquet", None)?;
for o in s3pe::list_s3("s3://iaempe/dados/")? { println!("{} {}", o.size, o.key); }
```

```go
meta, err := s3pe.ReadS3(ctx, "s3://iaempe/dados/metadata.json") // Go: []byte
_, err = s3pe.UploadS3(ctx, "icms.parquet", "s3://iaempe/nota/icms.parquet", "")
objects, err := s3pe.ListS3(ctx, "s3://iaempe/dados/")
```

```bash
s3pe ls s3://iaempe/dados/                                       # command line
s3pe get s3://iaempe/dados/pe_municipios.geojson
s3pe put painel.parquet s3://iaempe/etl/
```

| Verb | Does |
|---|---|
| `read_s3` / `write_s3` | move an object in memory: decoded by extension in R and Python, bytes in Rust and Go |
| `download_s3` / `upload_s3` | move a file, streaming, written as `.s3pe-part` and renamed when complete |
| `list_s3` | every object under a prefix, all pages |
| `exists_s3` / `head_s3` | whether an object exists; its size, type, date and ETag |
| `delete_s3` | delete (idempotent) |
| `buckets_s3` | the buckets the credentials can see |

Every edition also has a `Client` (R: `s3_config()`) for explicit settings.

## Configuration

Argument, then environment, then default:

| Setting | Environment | Default |
|---|---|---|
| endpoint | `S3PE_ENDPOINT`, then `AWS_ENDPOINT_URL` | `https://s3.pe.gov.br` |
| region | `AWS_REGION`, then `AWS_DEFAULT_REGION` | `us-east-1` (the Garage's `s3_region`) |
| credentials | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` | required |
| bucket | `S3PE_BUCKET` | none: write `s3://bucket/key` |
| quiet | `S3PE_QUIET=1` | one log line per transfer |

In R the usual place is `~/.Renviron`; in Python and the CLI any `.env`
loader or the shell. Never put the key in a script or a repository. Inside
the condepe server use `S3PE_ENDPOINT=http://127.0.0.1:9003`.

## Dependable by default

- **Signing**: AWS Signature V4, header style, over the real SHA-256 of the
  payload. No `aws-chunked` (Garage rejects it), no unsigned payloads. The
  four signers reproduce the AWS documentation vector.
- **Retries**: connection failures and HTTP 429/500/502/503/504, 3 tries,
  1 s then 2 s. Never on other 4xx.
- **Typed errors**: not found, access denied, other HTTP (with the S3 code),
  transport, configuration, integrity. `exists_s3`/`head_s3` turn 404 into
  `FALSE`/`None`/`nil`.
- **Integrity**: ETag vs MD5 after uploads; `Content-Length` and MD5 after
  downloads.
- **Tests**: each edition runs against a local fake S3 that verifies every
  signature, and against the real server when `S3PE_TEST_BUCKET` is set.

Design decisions are recorded in [DESIGN.md](DESIGN.md); changes in
[CHANGELOG.md](CHANGELOG.md).

## Repository layout

```
r/        R package (CRAN)                  rust/    Rust crate and CLI (crates.io)
python/   Python package, PyO3 on the crate  *.go     Go module (this directory)
site/     landing page (GitHub Pages)        DESIGN.md decisions shared by the four
```

## Authors

André Leite, Marcos Wasiliew, Hugo Vasconcelos, Carlos Amorim, Diogo Bezerra
and Júlia Nascimento Barreto. MIT license. Cite with [CITATION.cff](CITATION.cff).
