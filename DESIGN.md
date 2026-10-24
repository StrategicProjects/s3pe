# Design decisions — s3pe

Record of the non-obvious choices, in the order they were made. The four
editions (Rust, Python, R, Go) follow the same spec; a change in behaviour
belongs in all four.

## D1. Name and scope

`s3pe`: a thin, dependable client for any S3-compatible object store
(AWS S3, Garage, MinIO, Ceph, …). It was born for the Garage server of the
State of Pernambuco, Brazil, hence the name; since 0.2.0 (D14) nothing in
the code or the documentation is specific to that store. Free on CRAN,
PyPI, crates.io and GitHub on 2026-10-23.

## D2. One repository, four editions, one version

`github.com/StrategicProjects/s3pe` holds the Go module at the root (so its
import path is the repository URL and tags are plain `vX.Y.Z`), the Rust
crate in `rust/`, the Python package in `python/` (PyO3 bindings on the
crate) and the R package in `r/`. All four share one version and one
changelog. GitHub Pages serves a landing page at the root, pkgdown under
`/r/`, rustdoc under `/rust/` and pdoc under `/python/`.

## D3. Rust core for Rust and Python; R and Go on their own

The Python package is a binding on the crate (like `pyforesight`). The R
package is pure R: a Rust core on CRAN would mean vendoring a TLS stack
(far beyond CRAN's size limit) and a Rust toolchain on every user's
machine, for a job that httr2 already does. Go is standard library only,
as its users expect. The four are kept equal by one spec and one shared
test vector (D8).

## D4. Configuration: argument > environment > default

| Setting | Environment | Default |
|---|---|---|
| endpoint | `S3PE_ENDPOINT`, then `AWS_ENDPOINT_URL` | required (D14) |
| region | `AWS_REGION`, then `AWS_DEFAULT_REGION` | `us-east-1` |
| access key | `AWS_ACCESS_KEY_ID` | required |
| secret key | `AWS_SECRET_ACCESS_KEY` | required |
| session token | `AWS_SESSION_TOKEN` | none |
| bucket | `S3PE_BUCKET` | none (required unless the key is an `s3://` URL) |

Path-style URLs always (`https://host/bucket/key`): virtual-host style
needs DNS the store may not have, and path-style works on every store. A key may be written as
`s3://bucket/key`, which sets the bucket for that call.

## D5. Verbs: `<verb>_s3`

`read_s3`, `write_s3`, `download_s3`, `upload_s3`, `list_s3`, `delete_s3`,
`exists_s3`, `head_s3`, `buckets_s3`. `read`/`write` move R and Python
objects (the format comes from the key's extension, or `format=`);
`download`/`upload` move files; the rest are plain S3 operations. Rust and
Go expose the same verbs as free functions over a default client and as
methods of `Client`.

## D6. Signing: our own SigV4, header-signed, payload hashed

httr2's `req_auth_aws_v4()` only signs empty, JSON and form bodies, so the
R edition signs by itself (openssl); the others do the same in ~60 lines.
Signed headers are `host`, `x-amz-content-sha256`, `x-amz-date` (and
`x-amz-security-token` when present). The payload hash is the real SHA-256
of the body, never `UNSIGNED-PAYLOAD` and never `aws-chunked` (some stores, Garage
2.4 among them, reject the chunked trailer the AWS SDKs send by default).

## D7. Robustness

- Retries: 3 tries, backoff 1 s then 2 s, on connection errors and on
  HTTP 429/500/502/503/504 only. Never on 4xx (those are the caller's).
- Timeouts: one `timeout` per call (default 300 s) for big transfers.
- Integrity: after a `PUT`, the ETag is compared with the local MD5 (when
  the ETag is a plain MD5; multipart ETags are not). After a `GET`, the
  byte count is compared with `Content-Length`. A mismatch is an error.
- Errors are typed: `NotFound` (404), `AccessDenied` (403), `Http` (other
  status, with the S3 error code and message), `Transport`, `Config`
  (missing credentials or bucket), `Integrity`.
- `exists_s3` and `head_s3` turn 404 into `FALSE`/`None`/`NULL`; everything
  else raises.

## D8. One test vector for the four signers

The AWS documentation example (GET `examplebucket/test.txt`, key
`AKIAIOSFODNN7EXAMPLE`, 2013-05-24, `Range: bytes=0-9`) must give the
signature `f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41`
in every edition. Each edition also runs against a local fake S3 server in
its tests and, when `S3PE_ENDPOINT` and `S3PE_TEST_BUCKET` are set, against
a real one.

## D9. Logging: one line per transfer, on stderr, silenceable

`↑ s3://bucket/key (12.3 MB, 0.8 s)` for uploads, `↓` for downloads,
`✔` for listings and `✖` for deletions. R: `cli`, silenced with
`options(s3pe.quiet = TRUE)`; Python: the `s3pe` logger (`s3pe.quiet()`);
Rust: the `log` facade (the CLI prints it); Go: `Client.Quiet`.
`S3PE_QUIET=1` silences every edition.

## D10. Formats by extension

| Extension | R | Python |
|---|---|---|
| `.csv`, `.csv.gz`, `.tsv` | readr | pandas |
| `.json` | jsonlite | `json` |
| `.geojson` | sf if installed, else jsonlite | `json` |
| `.parquet` | nanoparquet | pandas (pyarrow) |
| `.rds` | base | — |
| `.xlsx` | readxl | pandas (openpyxl) |
| `.txt`, `.md`, `.html`, `.xml`, `.svg` | character | `str` |
| anything else | raw | `bytes` |

`format=` overrides the extension; `format = "raw"`/`"bytes"` always works.
Content types on upload come from the same table.

## D11. No multipart upload in 0.1

A single `PUT` carries up to 5 GB, enough for the files this client was
made to move. Multipart is a planned addition, not a 0.1 promise.

## D12. Session helpers for the bucket and the endpoint

Typing `s3://my-bucket/...` on every call is the explicit form, not the
required one. Each edition has a cheaper way to name the bucket once:
`bucket=` on every verb, `S3PE_BUCKET` in the environment, and a session
setter: R `use_bucket("my-bucket")` / `use_endpoint("https://s3.example.org")`
(they set `options(s3pe.bucket)` and `options(s3pe.endpoint)`), Python
`s3pe.configure(bucket = "my-bucket")`, Rust `Client::builder().bucket()`, Go
`Config{Bucket: "my-bucket"}`, CLI `-b my-bucket`.

## D13. Hashing files in R needs `file(path, raw = TRUE)`

R's `file()` transparently gunzips a `.gz` file, so the SHA-256 computed
over the connection would not match the bytes sent and the server would
answer `XAmzContentSHA256Mismatch`. The R edition opens files with
`raw = TRUE` before hashing and before sending.

## D14. Generic by default: no endpoint is assumed (0.2.0)

Until 0.1.0 the endpoint defaulted to the store the package was written
for. A generic client should not carry one organisation's server in its
defaults, so from 0.2.0 the endpoint is required: `S3PE_ENDPOINT`,
`AWS_ENDPOINT_URL`, the client argument, or in R `use_endpoint()`. The
documentation and the examples use neutral names (`s3://my-bucket/…`,
`https://s3.example.org`). The only trace of origin is a footer, "Made in
Pernambuco 🌵". Worked examples for a specific store belong in that store's
own documentation, not in the package.
