// Package s3pe is a small, dependable client for S3-compatible object
// stores (AWS S3, Garage, MinIO, Ceph, ...). Standard library only, no cgo.
//
// Nine verbs, one client, nothing else to learn:
//
//	ctx := context.Background()
//	manifest, err := s3pe.ReadS3(ctx, "s3://my-bucket/data/manifest.json")
//	_, err = s3pe.WriteS3(ctx, "s3://my-bucket/reports/figure1.png", png, "")
//	objects, err := s3pe.ListS3(ctx, "s3://my-bucket/data/")
//	_, err = s3pe.UploadS3(ctx, "sales.parquet", "s3://my-bucket/etl/", "")
//	_, err = s3pe.DownloadS3(ctx, "s3://my-bucket/etl/sales.parquet", "sales.parquet")
//	ok, err := s3pe.ExistsS3(ctx, "s3://my-bucket/etl/manifest.json")
//	meta, err := s3pe.HeadS3(ctx, "s3://my-bucket/etl/manifest.json")
//	err = s3pe.DeleteS3(ctx, "s3://my-bucket/etl/old.parquet")
//	buckets, err := s3pe.BucketsS3(ctx)
//
// The package-level verbs share a client built from the environment. An
// explicit [Client] takes the same settings in a [Config]; a default bucket
// makes keys short:
//
//	s3, err := s3pe.New(s3pe.Config{Endpoint: "https://s3.example.org", Bucket: "my-bucket"})
//	data, err := s3.Read(ctx, "data/manifest.json")
//
// # Configuration
//
// Anything not set in Config comes from the environment, then from the
// defaults:
//
//	endpoint     S3PE_ENDPOINT, AWS_ENDPOINT_URL      required
//	region       AWS_REGION, AWS_DEFAULT_REGION       us-east-1
//	credentials  AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN   required
//	bucket       S3PE_BUCKET                          none: use s3://bucket/key
//	quiet        S3PE_QUIET                           off
//
// URLs are always path-style (https://host/bucket/key); a key may be an
// s3://bucket/key URL, which names the bucket for that call.
//
// # What "dependable" means here
//
// Requests are signed with AWS Signature V4 over the real SHA-256 of the
// payload (no aws-chunked, which some stores, Garage among them, reject). Connection failures and
// HTTP 429/500/502/503/504 are retried twice, with a 1 s then 2 s pause;
// other errors are returned at once, typed: errors.Is(err, [ErrNotFound]),
// [ErrAccessDenied], [ErrConfig], [ErrIntegrity], [ErrTransport], [ErrHTTP],
// with the details in an [*Error]. After an upload the server's ETag is
// compared with the local MD5; after a download the byte count is compared
// with Content-Length and, when the ETag is an MD5, with the MD5 of what
// arrived. Downloads go to a .s3pe-part file renamed when complete. One
// line per transfer goes to the logger (standard error by default):
//
//	↑ s3://my-bucket/etl/sales.parquet (12.3 MB, 0.8 s)
//
// This is the Go edition of s3pe; the same package exists as a Rust crate
// with a command line (cargo install s3pe), for Python (pip install s3pe)
// and for R (install.packages("s3pe")). All four follow one specification
// and reproduce the same signatures.
package s3pe
