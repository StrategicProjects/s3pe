test_that("configuration resolves argument > option > environment > default", {
  withr::local_envvar(
    S3PE_ENDPOINT = "", AWS_ENDPOINT_URL = "", AWS_REGION = "", AWS_DEFAULT_REGION = "",
    S3PE_BUCKET = "", AWS_ACCESS_KEY_ID = "", AWS_SECRET_ACCESS_KEY = "", AWS_SESSION_TOKEN = ""
  )
  withr::local_options(s3pe.endpoint = NULL)
  cfg <- s3_config()
  expect_s3_class(cfg, "s3pe_config")
  expect_null(cfg$endpoint)
  expect_null(cfg$host)
  expect_equal(cfg$region, "us-east-1")
  expect_null(cfg$bucket)
  expect_null(cfg$access_key)
  expect_equal(cfg$timeout, 300)
  expect_equal(cfg$max_tries, 1L) # setup.R option

  withr::local_envvar(
    AWS_ENDPOINT_URL = "http://127.0.0.1:9000/", AWS_DEFAULT_REGION = "eu-west-1",
    S3PE_BUCKET = "my-bucket", AWS_ACCESS_KEY_ID = "GKenv", AWS_SECRET_ACCESS_KEY = "senv"
  )
  cfg <- s3_config()
  expect_equal(cfg$endpoint, "http://127.0.0.1:9000")
  expect_equal(cfg$host, "127.0.0.1:9000")
  expect_equal(cfg$region, "eu-west-1")
  expect_equal(cfg$bucket, "my-bucket")
  expect_equal(cfg$access_key, "GKenv")

  withr::local_envvar(S3PE_ENDPOINT = "https://other.example:443")
  expect_equal(s3_config()$host, "other.example")

  withr::local_options(s3pe.endpoint = "https://opt.example", s3pe.bucket = "b",
                       s3pe.region = "r", s3pe.timeout = 10, s3pe.max_tries = 5)
  cfg <- s3_config()
  expect_equal(cfg$endpoint, "https://opt.example")
  expect_equal(cfg$bucket, "b")
  expect_equal(cfg$region, "r")
  expect_equal(cfg$timeout, 10)
  expect_equal(cfg$max_tries, 5L)

  cfg <- s3_config(endpoint = "https://arg.example/", bucket = "c", max_tries = 2)
  expect_equal(cfg$endpoint, "https://arg.example")
  expect_equal(cfg$bucket, "c")
  expect_equal(cfg$max_tries, 2L)
})

test_that("bad endpoints are refused", {
  expect_error(s3_config(endpoint = "s3.example.org"), class = "s3pe_error_config")
  expect_error(s3_config(endpoint = "https://s3.example.org/bucket"), class = "s3pe_error_config")
  expect_error(s3_config(endpoint = "ftp://x"), class = "s3pe_error_config")
})

test_that("printing never shows the secret", {
  cfg <- s3_config(access_key = "GK1234567890", secret_key = "very-secret", bucket = "b")
  out <- paste(capture.output(print(cfg)), collapse = "\n")
  expect_match(out, "GK12")
  expect_false(grepl("very-secret", out, fixed = TRUE))
  expect_match(out, "<set>")
  withr::local_options(s3pe.endpoint = NULL)
  withr::local_envvar(S3PE_ENDPOINT = "", AWS_ENDPOINT_URL = "")
  out <- paste(capture.output(print(s3_config())), collapse = "\n")
  expect_match(out, "<unset>")
})

test_that("missing credentials are an error at request time", {
  withr::local_envvar(AWS_ACCESS_KEY_ID = "", AWS_SECRET_ACCESS_KEY = "")
  cfg <- s3_config(endpoint = "https://s3.example.org", bucket = "b")
  expect_error(read_s3("x", config = cfg), class = "s3pe_error_config")
  expect_error(list_s3(config = cfg), class = "s3pe_error_config")
})

test_that("a missing endpoint is an error at request time, not at s3_config()", {
  withr::local_options(s3pe.endpoint = NULL)
  withr::local_envvar(S3PE_ENDPOINT = "", AWS_ENDPOINT_URL = "")
  cfg <- s3_config(access_key = "a", secret_key = "b", bucket = "b")
  expect_null(cfg$endpoint)
  err <- expect_error(read_s3("x", config = cfg), class = "s3pe_error_config")
  expect_match(conditionMessage(err), "S3PE_ENDPOINT")
  expect_error(exists_s3("x", config = cfg), class = "s3pe_error_config")
  expect_error(buckets_s3(config = cfg), class = "s3pe_error_config")
})

test_that("keys resolve against s3:// URLs, the bucket argument and the config", {
  cfg <- fake_config()
  expect_equal(resolve_key("dados/x.json", NULL, cfg), list(bucket = "test", key = "dados/x.json"))
  expect_equal(resolve_key("/dados/x.json", NULL, cfg), list(bucket = "test", key = "dados/x.json"))
  expect_equal(resolve_key("x", "other", cfg), list(bucket = "other", key = "x"))
  expect_equal(resolve_key("s3://b/k/l", "other", cfg), list(bucket = "b", key = "k/l"))
  expect_equal(resolve_key("s3://b", NULL, cfg, allow_empty = TRUE), list(bucket = "b", key = ""))
  expect_error(resolve_key("s3://b", NULL, cfg), class = "s3pe_error_config")
  expect_error(resolve_key("s3:///k", NULL, cfg), class = "s3pe_error_config")
  no_bucket <- s3_config(endpoint = "https://s3.example.org", access_key = "a", secret_key = "b", bucket = NULL)
  withr::local_envvar(S3PE_BUCKET = "")
  withr::local_options(s3pe.bucket = NULL)
  no_bucket$bucket <- NULL
  expect_error(resolve_key("k", NULL, no_bucket), class = "s3pe_error_config")
})

test_that("format_size and content types match the other editions", {
  expect_equal(format_size(0), "0 B")
  expect_equal(format_size(999), "999 B")
  expect_equal(format_size(1234), "1.2 kB")
  expect_equal(format_size(12345678), "12.3 MB")
  expect_equal(content_type_for("dados/series.csv.gz"), "application/gzip")
  expect_equal(content_type_for("a/b.PARQUET"), "application/vnd.apache.parquet")
  expect_equal(content_type_for("noext"), "application/octet-stream")
  expect_equal(content_type_for("x.geojson"), "application/geo+json")
  expect_equal(content_type_for("x.json"), "application/json")
})

test_that("HTTP dates parse without depending on the locale", {
  d <- parse_http_date("Thu, 23 Oct 2026 12:00:00 GMT")
  expect_equal(d, as.POSIXct("2026-10-23 12:00:00", tz = "UTC"))
  expect_true(is.na(parse_http_date("")))
})

test_that("quietness follows the option and S3PE_QUIET", {
  withr::local_options(s3pe.quiet = FALSE)
  withr::local_envvar(S3PE_QUIET = "")
  expect_false(s3_is_quiet())
  withr::local_envvar(S3PE_QUIET = "1")
  expect_true(s3_is_quiet())
  withr::local_envvar(S3PE_QUIET = "0")
  expect_false(s3_is_quiet())
  expect_message(s3_inform("hello"), "hello")
})

test_that("use_bucket() and use_endpoint() set and clear session options", {
  withr::local_options(s3pe.bucket = NULL, s3pe.endpoint = NULL)
  withr::local_envvar(S3PE_BUCKET = "", S3PE_ENDPOINT = "", AWS_ENDPOINT_URL = "")
  expect_null(use_bucket("my-bucket"))
  expect_equal(s3_config()$bucket, "my-bucket")
  expect_equal(use_bucket(NULL), "my-bucket")
  expect_null(s3_config()$bucket)
  expect_null(use_endpoint("https://s3.example.org/"))
  expect_equal(s3_config()$endpoint, "https://s3.example.org")
  expect_equal(use_endpoint(NULL), "https://s3.example.org")
  expect_null(s3_config()$endpoint)
  expect_error(use_endpoint("s3.example.org"), class = "s3pe_error_config")
})
