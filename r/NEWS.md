# s3pe 0.3.0

* Multipart upload: `write_s3()` and `upload_s3()` send bodies larger than
  `multipart_threshold` (64 MiB by default; `s3_config()`,
  `S3PE_MULTIPART_THRESHOLD`) in parts of `part_size` (16 MiB), one by
  one, aborted on any failure, with every part's ETag and the final ETag
  checked.
* `write_s3()` and `upload_s3()` take `metadata` (user metadata, returned
  by `head_s3()` in its new `metadata` column) and `headers` (any extra
  header: `Cache-Control`, `Content-Disposition`, `x-amz-storage-class`,
  server-side encryption, ...).
* `copy_s3()` copies an object on the server.
* `delete_s3()` accepts several keys and deletes them in batches of 1000;
  it now returns the number of keys deleted.
* `list_s3()` gains `delimiter`: `delimiter = "/"` lists one level,
  "folders" included.
* `presign_s3()` returns a pre-signed URL (GET or PUT) valid for up to
  seven days.
* `sync_up_s3()` and `sync_down_s3()` mirror a directory and a prefix by
  size and ETag, optionally deleting extras and uploading a manifest last.
* Not implemented, by design: ACLs (manage access with keys and policies)
  and versioning (no `versionId` on read, head or delete).

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
