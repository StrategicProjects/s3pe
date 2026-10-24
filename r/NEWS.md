# s3pe 0.2.0

* The endpoint has no default any more: set `S3PE_ENDPOINT`,
  `options(s3pe.endpoint)` or call `use_endpoint()`. A missing endpoint is
  an `s3pe_error_config` at request time, like missing credentials.
* `use_bucket()` and `use_endpoint()` set the bucket and the endpoint for
  the session.
* Documentation and examples are generic: the package serves any
  S3-compatible store.

# s3pe 0.1.0

* Initial release.
* `read_s3()` and `write_s3()` move R objects to and from the store, with
  the format chosen by the key's extension (CSV, TSV, JSON, GeoJSON,
  Parquet, RDS, XLSX, text, raw).
* `download_s3()` and `upload_s3()` move files, streaming.
* `list_s3()`, `head_s3()`, `exists_s3()`, `delete_s3()` and
  `buckets_s3()` for the rest of the bucket.
* `s3_config()` resolves endpoint, region, credentials and bucket from
  arguments, options and the environment, in that order.
* Requests signed with AWS Signature Version 4 (header style, payload
  hashed); transient failures retried with exponential backoff; ETag and
  content length checked after every transfer.
