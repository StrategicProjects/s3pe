# s3pe for Python

[![PyPI](https://img.shields.io/pypi/v/s3pe.svg)](https://pypi.org/project/s3pe/)
[![CI](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml/badge.svg)](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/StrategicProjects/s3pe/blob/main/LICENSE)
[![DOI](https://zenodo.org/badge/DOI/10.5281/zenodo.23606939.svg)](https://doi.org/10.5281/zenodo.23606939)

Read and write data on any S3-compatible object store (AWS S3, Garage,
MinIO, Ceph, …). Nine verbs plus copy, pre-signed URLs and sync, three
environment variables, and a Rust core that signs, retries and checks
every transfer. Big files go up as multipart uploads by themselves.

**Website:** <https://strategicprojects.github.io/s3pe/python/> ·
**API reference:** <https://strategicprojects.github.io/s3pe/python/api/> ·
[Português](#em-português)

## Install

```bash
pip install s3pe            # bytes, str, dict/list and files
pip install "s3pe[pandas]"  # also CSV, Parquet and Excel as DataFrames
```

Wheels for Linux (x86_64, aarch64), macOS (Intel, Apple silicon) and
Windows; Python 3.9 or later.

## Use

```python
import s3pe

# S3PE_ENDPOINT, AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY in the environment
meta = s3pe.read_s3("s3://my-bucket/data/manifest.json")      # dict
meta["version"]

df = s3pe.read_s3("series/sales.csv.gz", bucket="my-bucket")  # DataFrame
s3pe.write_s3(df, "s3://my-bucket/reports/sales.parquet")      # by extension
s3pe.upload_s3("figure1.png", "s3://my-bucket/reports/")       # keeps the file name
s3pe.download_s3("s3://my-bucket/data/regions.geojson", "regions.geojson")

for o in s3pe.list_s3("s3://my-bucket/data/"):
    print(o.size, o.key)
s3pe.exists_s3("s3://my-bucket/data/manifest.json")
s3pe.delete_s3("s3://my-bucket/reports/old.parquet")
```

Every call prints one line on stderr (`↑ s3://my-bucket/reports/sales.parquet
(1.2 MB, 0.3 s)`); `s3pe.quiet()` or `S3PE_QUIET=1` silences it.

With `S3PE_BUCKET=my-bucket` in the environment, or after
`s3pe.configure(bucket="my-bucket")`, bare keys work:
`s3pe.read_s3("data/manifest.json")`. For explicit settings use a client:

```python
s3 = s3pe.Client(endpoint="http://127.0.0.1:9000", bucket="my-bucket",
                 access_key="...", secret_key="...")
s3.read("data/manifest.json")
s3.write(df, "reports/sales.csv")
s3.list("data/")
```

### Big files, copies, batches, folders, metadata

```python
s3pe.upload_s3("backup.tar", "s3://my-bucket/backups/")   # > 64 MiB: multipart, 4 parts at a time
s3pe.copy_s3("s3://my-bucket/etl/sales.parquet", "s3://my-bucket/archive/sales-2026.parquet")
s3pe.delete_s3([o.key for o in s3pe.list_s3("s3://my-bucket/tmp/")], bucket="my-bucket")
for e in s3pe.list_s3("s3://my-bucket/", delimiter="/"):      # one level: "folders" have size None
    print("dir " if e.is_prefix else "file", e.key)
s3pe.write_s3(df, "s3://my-bucket/reports/sales.csv",
              metadata={"author": "ana"}, headers={"Cache-Control": "max-age=3600"})
s3pe.head_s3("s3://my-bucket/reports/sales.csv").metadata     # {'author': 'ana'}
```

`metadata` becomes `x-amz-meta-*`; `headers` are sent as given, so
`{"x-amz-server-side-encryption": "AES256"}` turns on SSE where the store
supports it. `Client(multipart_threshold=..., part_size=..., parallel=...)`
or `S3PE_MULTIPART_THRESHOLD`, `S3PE_PART_SIZE`, `S3PE_PARALLEL` tune the
multipart upload (defaults 64 MiB, 16 MiB, 4).

### Pre-signed URLs and sync

```python
url = s3pe.presign_s3("s3://my-bucket/reports/report.pdf", expires=3600)   # anyone can GET it for an hour
url = s3pe.presign_s3("s3://my-bucket/inbox/upload.csv", method="PUT")

r = s3pe.sync_up_s3("./out", "s3://my-bucket/reports/", last="manifest.json")   # by ETag; manifest last
r.transferred, r.skipped, r.deleted
s3pe.sync_down_s3("s3://my-bucket/reports/", "./mirror", delete=True)
```

### Formats

| Extension | `read_s3` returns | `write_s3` accepts |
|---|---|---|
| `.json`, `.geojson` | `dict` / `list` | `dict`, `list`, `str`, DataFrame (`orient="records"`) |
| `.csv`, `.csv.gz`, `.tsv` | `pandas.DataFrame` | DataFrame |
| `.parquet` | `pandas.DataFrame` | DataFrame |
| `.xlsx` | `pandas.DataFrame` | DataFrame |
| `.txt`, `.md`, `.html`, `.xml`, `.svg`, ... | `str` | `str` |
| anything else | `bytes` | `bytes`, file-like, `pathlib.Path` |

`format="bytes"` (or `"text"`, `"json"`, `"csv"`, `"parquet"`, `"xlsx"`)
overrides the extension; extra keyword arguments go to the reader or writer
(`s3pe.read_s3("x.csv", bucket="b", sep=";", dtype=str)`). A `.gz` suffix
is compressed and decompressed on the way.

### Configuration

| Setting | Environment | Default |
|---|---|---|
| endpoint | `S3PE_ENDPOINT`, then `AWS_ENDPOINT_URL` | required (`https://host[:port]`) |
| region | `AWS_REGION`, then `AWS_DEFAULT_REGION` | `us-east-1` |
| credentials | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` (`AWS_SESSION_TOKEN`) | required |
| bucket | `S3PE_BUCKET` | none: use `s3://bucket/key` or `bucket=` |

### Errors

All errors derive from `s3pe.S3Error`: `NotFoundError` (404),
`AccessDeniedError` (403), `HttpError` (other statuses, with `.status`,
`.code`, `.message`), `TransportError` (network, after retries),
`ConfigError` (missing endpoint, credentials or bucket) and
`IntegrityError` (the bytes that arrived differ from the bytes sent).
`exists_s3` and `head_s3` turn 404 into `False` / `None`.

### What happens in every call

<img src="https://raw.githubusercontent.com/StrategicProjects/s3pe/main/site/assets/request-flow.svg" alt="Resolve the key, hash the payload, sign with SigV4, send, retry on 429 and 5xx, verify the ETag and Content-Length; success logs one line, errors are typed." width="100%" />

### Dependable by default

- SigV4 signatures over the real SHA-256 of the payload, path-style URLs.
- Connection failures and HTTP 429/500/502/503/504 are retried twice (1 s,
  then 2 s). Other errors are raised at once, typed.
- After an upload the server's ETag is compared with the local MD5; after a
  download the byte count with `Content-Length` and, when the ETag is an
  MD5, the MD5 of what arrived.
- Files are streamed to a `.s3pe-part` file and renamed when complete.
- The Rust core releases the GIL during transfers.

## Architecture

<img src="https://raw.githubusercontent.com/StrategicProjects/s3pe/main/site/assets/architecture.svg" alt="The Python package is a PyO3 binding on the Rust crate; R and Go have their own implementations of the same specification. All talk to any S3-compatible store." width="100%" />

## Em português

`s3pe` lê e grava dados em qualquer armazenamento de objetos compatível com
S3, com nove verbos: `read_s3`, `write_s3`, `download_s3`, `upload_s3`,
`list_s3`, `delete_s3`, `exists_s3`, `head_s3`, `buckets_s3`, mais
`copy_s3`, `presign_s3` (URL temporária), `sync_up_s3` e `sync_down_s3`
(espelho por ETag). Arquivos grandes sobem em partes automaticamente. O endpoint e
as credenciais vêm do ambiente (`S3PE_ENDPOINT`, `AWS_ACCESS_KEY_ID`,
`AWS_SECRET_ACCESS_KEY`); o formato vem da extensão do arquivo. Cada
transferência imprime uma linha no terminal; `s3pe.quiet()` silencia.

## The family

The same package exists for R (`install.packages("s3pe")`), Rust
(`cargo add s3pe`, with the `s3pe` command line) and Go
(`go get github.com/StrategicProjects/s3pe`), all in
[one repository](https://github.com/StrategicProjects/s3pe).

## Made in Pernambuco 🌵

s3pe is developed in Recife, Brazil. MIT license.
