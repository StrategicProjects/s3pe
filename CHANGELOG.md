# Changelog

All four editions (Rust crate, Python package, R package, Go module) share
one version and this file.

## 0.3.0 — 2026-10-24

- Multipart upload, automatic above 64 MiB (16 MiB parts, 4 in parallel,
  abort on failure, final ETag checked); `S3PE_MULTIPART_THRESHOLD`,
  `S3PE_PART_SIZE`, `S3PE_PARALLEL`. Parts carry
  `Content-Type: application/octet-stream` (some web application firewalls
  refuse untyped large bodies).
- `presign_s3`: pre-signed GET/PUT URLs (SigV4 query string, up to 7 days),
  checked against the AWS documentation vector in the four editions.
- `copy_s3` (server-side copy), `delete_s3` with many keys (`DeleteObjects`,
  1000 per request), `list_s3(delimiter = "/")` (one level, common
  prefixes), `metadata` and `headers` on `write_s3`/`upload_s3`
  (`x-amz-meta-*`, Cache-Control, SSE headers, …), user metadata on
  `head_s3`.
- `sync_up_s3` / `sync_down_s3`: a directory and a prefix mirrored by ETag,
  with a `last` file (a manifest) and optional deletion; `s3pe sync` in the
  CLI, plus `cp`, `presign`, `ls -d`, batch `rm` and `put --metadata/--header`.
- Non-XML error bodies (proxy pages) are kept in the error message.
- Out of scope, documented: ACLs and versioning (DESIGN.md D19).

## 0.2.0 — 2026-10-23

A generic client. The endpoint is no longer defaulted: set `S3PE_ENDPOINT`
(or `AWS_ENDPOINT_URL`), pass it to the client, or in R call
`use_endpoint()`. Without one, every edition raises a configuration error
before any request. Documentation, examples and package descriptions no
longer refer to a particular store; `DEFAULT_ENDPOINT` (Rust, Python) and
`DefaultEndpoint` (Go) were removed. R gains `use_bucket()` and
`use_endpoint()` for the session.

## 0.1.0 — 2026-10-23

Initial release.

- Nine verbs in every edition: `read_s3`, `write_s3`, `download_s3`,
  `upload_s3`, `list_s3`, `delete_s3`, `exists_s3`, `head_s3`, `buckets_s3`
  (Go: `ReadS3`, ... and the methods of `Client`).
- Configuration from the environment (`AWS_ACCESS_KEY_ID`,
  `AWS_SECRET_ACCESS_KEY`, `S3PE_ENDPOINT`, `S3PE_BUCKET`, `AWS_REGION`),
  with `https://s3.pe.gov.br` and `us-east-1` as defaults; `s3://bucket/key`
  URLs accepted everywhere; path-style requests.
- AWS Signature V4 over the real payload hash, checked against the AWS
  documentation vector in the four editions.
- Retries on connection failures and HTTP 429/500/502/503/504 (3 tries,
  1 s then 2 s); typed errors for 404, 403, other statuses, transport,
  configuration and integrity.
- Integrity: ETag vs MD5 after uploads; `Content-Length` and MD5 after
  downloads; downloads written to a `.s3pe-part` file and renamed.
- One log line per transfer, silenceable (`S3PE_QUIET=1`).
- Formats by extension in R and Python (CSV, CSV.GZ, TSV, JSON, GeoJSON,
  Parquet, XLSX, RDS, text; raw bytes otherwise).
- `s3pe` command line in the Rust crate (`ls`, `cat`, `get`, `put`, `rm`,
  `head`, `buckets`).
