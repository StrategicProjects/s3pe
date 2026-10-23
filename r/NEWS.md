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
