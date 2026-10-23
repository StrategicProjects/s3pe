op <- options(
  s3pe.quiet = TRUE,
  s3pe.max_tries = 1,
  s3pe.endpoint = NULL,
  s3pe.bucket = NULL
)
withr::defer(options(op), teardown_env())
