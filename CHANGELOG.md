# Changelog

All four editions (Rust crate, Python package, R package, Go module) share
one version and this file.

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
