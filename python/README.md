# s3pe for Python

[![PyPI](https://img.shields.io/pypi/v/s3pe.svg)](https://pypi.org/project/s3pe/)
[![CI](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml/badge.svg)](https://github.com/StrategicProjects/s3pe/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/StrategicProjects/s3pe/blob/main/LICENSE)

Read and write data on **s3.pe.gov.br**, the S3 object store of the State
of Pernambuco (a [Garage](https://garagehq.deuxfleurs.fr) server), or on
any S3-style store. Nine verbs, no configuration beyond two environment
variables, and a Rust core that signs, retries and checks every transfer.

**Website:** <https://strategicprojects.github.io/s3pe/python/> ·
**API reference:** <https://strategicprojects.github.io/s3pe/python/api/> ·
[Português](#em-português)

## Install

```bash
pip install s3pe            # bytes, str, dict/list and files
pip install "s3pe[pandas]"  # also CSV, Parquet and Excel as DataFrames
```

Wheels for Linux, macOS and Windows; Python 3.9 or later.

## Use

```python
import s3pe

# credentials from the environment: AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY
meta = s3pe.read_s3("s3://iaempe/dados/metadata.json")      # dict
meta["ultimo_mes"]

df = s3pe.read_s3("series/estado/icms.csv.gz", bucket="horizonte-fiscal")  # DataFrame
prev = s3pe.read_s3("previsoes/26/icms/ultima.json", bucket="horizonte-fiscal")

s3pe.write_s3(df, "s3://iaempe/nota/icms.parquet")           # by extension
s3pe.upload_s3("figura1.png", "s3://iaempe/nota/")           # keeps the file name
s3pe.download_s3("s3://iaempe/dados/pe_municipios.geojson", "pe.geojson")

for o in s3pe.list_s3("s3://iaempe/dados/"):
    print(o.size, o.key)
s3pe.exists_s3("s3://iaempe/dados/metadata.json")
s3pe.delete_s3("s3://iaempe/nota/old.parquet")
```

Every call prints one line on stderr (`↑ s3://iaempe/nota/icms.parquet
(1.2 MB, 0.3 s)`); `s3pe.quiet()` or `S3PE_QUIET=1` silences it.

With `S3PE_BUCKET=iaempe` in the environment, bare keys work:
`s3pe.read_s3("dados/metadata.json")`. For explicit settings use a client:

```python
s3 = s3pe.Client(endpoint="http://127.0.0.1:9003", bucket="iaempe",
                 access_key="GK...", secret_key="...")
s3.read("dados/metadata.json")
s3.write(df, "nota/icms.csv")
s3.list("dados/")
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
| endpoint | `S3PE_ENDPOINT`, then `AWS_ENDPOINT_URL` | `https://s3.pe.gov.br` |
| region | `AWS_REGION`, then `AWS_DEFAULT_REGION` | `us-east-1` |
| credentials | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` (`AWS_SESSION_TOKEN`) | required |
| bucket | `S3PE_BUCKET` | none: use `s3://bucket/key` or `bucket=` |

### Errors

All errors derive from `s3pe.S3Error`: `NotFoundError` (404),
`AccessDeniedError` (403), `HttpError` (other statuses, with `.status`,
`.code`, `.message`), `TransportError` (network, after retries),
`ConfigError` (missing credentials or bucket) and `IntegrityError` (the
bytes that arrived differ from the bytes sent). `exists_s3` and `head_s3`
turn 404 into `False` / `None`.

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

<img src="https://raw.githubusercontent.com/StrategicProjects/s3pe/main/site/assets/architecture.svg" alt="The Python package is a PyO3 binding on the Rust crate; R and Go have their own implementations of the same specification. All talk to s3.pe.gov.br or any S3 store." width="100%" />

## Em português

`s3pe` lê e grava dados no `s3.pe.gov.br` (o Garage da plataforma do
Condepe/Fidem) com nove verbos: `read_s3`, `write_s3`, `download_s3`,
`upload_s3`, `list_s3`, `delete_s3`, `exists_s3`, `head_s3`, `buckets_s3`.
As credenciais vêm do ambiente (`AWS_ACCESS_KEY_ID`,
`AWS_SECRET_ACCESS_KEY`); o formato vem da extensão do arquivo. Cada
transferência imprime uma linha no terminal; `s3pe.quiet()` silencia.

## The family

The same package exists for R (`install.packages("s3pe")`), Rust
(`cargo add s3pe`, with the `s3pe` command line) and Go
(`go get github.com/StrategicProjects/s3pe`), all in
[one repository](https://github.com/StrategicProjects/s3pe).
