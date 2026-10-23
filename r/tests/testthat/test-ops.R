test_that("a full round trip with verified signatures", {
  fake <- local_fake_s3()
  cfg <- fake_config()

  expect_false(exists_s3("dados/metadata.json", config = cfg))
  expect_null(head_s3("dados/metadata.json", config = cfg))
  expect_error(read_s3("dados/metadata.json", config = cfg), class = "s3pe_error_not_found")

  out <- write_s3(list(ultimo_mes = 202608), "dados/metadata.json", config = cfg)
  expect_s3_class(out, "tbl_df")
  expect_equal(out$bucket, "test")
  expect_equal(out$key, "dados/metadata.json")
  expect_equal(out$size, nchar('{"ultimo_mes":202608}'))
  expect_equal(fake$state$objects[["test/dados/metadata.json"]]$type, "application/json")

  meta <- read_s3("dados/metadata.json", config = cfg)
  expect_equal(meta$ultimo_mes, 202608)
  expect_equal(read_s3("s3://test/dados/metadata.json", config = cfg)$ultimo_mes, 202608)
  expect_equal(rawToChar(read_s3("dados/metadata.json", config = cfg, format = "raw")), '{"ultimo_mes":202608}')

  h <- head_s3("dados/metadata.json", config = cfg)
  expect_equal(h$size, 21)
  expect_equal(h$content_type, "application/json")
  expect_equal(nchar(h$etag), 32)
  expect_equal(h$last_modified, as.POSIXct("2026-10-23 12:00:00", tz = "UTC"))
  expect_true(exists_s3("dados/metadata.json", config = cfg))

  expect_true(delete_s3("dados/metadata.json", config = cfg))
  expect_false(exists_s3("dados/metadata.json", config = cfg))
  expect_true(delete_s3("dados/metadata.json", config = cfg))
})

test_that("keys with special characters round-trip", {
  fake <- local_fake_s3()
  cfg <- fake_config()
  key <- "pasta/ação & x+y (1).csv"
  write_s3(data.frame(a = 1, b = 2), key, config = cfg)
  expect_true(grepl("pasta/a%C3%A7%C3%A3o%20%26%20x%2By%20%281%29.csv", fake$state$requests[1], fixed = TRUE))
  expect_equal(names(fake$state$objects), paste0("test/", key))
  expect_equal(fake$state$objects[[1]]$type, "text/csv")
  df <- read_s3(key, config = cfg)
  expect_equal(df$a, 1)
  listed <- list_s3("pasta/", config = cfg)
  expect_equal(listed$key, key)
  expect_equal(listed$size, 8)
})

test_that("listing follows continuation tokens and is typed when empty", {
  fake <- local_fake_s3()
  cfg <- fake_config()
  for (i in 0:4) write_s3(as.character(i), sprintf("p/%d.txt", i), config = cfg)
  write_s3("x", "q/other.txt", config = cfg)
  all <- list_s3("p/", config = cfg)
  expect_equal(all$key, sprintf("p/%d.txt", 0:4))
  expect_s3_class(all$last_modified, "POSIXct")
  expect_equal(nrow(list_s3(config = cfg)), 6)
  expect_equal(nrow(list_s3("s3://test", config = cfg)), 6)
  empty <- list_s3("nothing/", config = cfg)
  expect_equal(nrow(empty), 0)
  expect_named(empty, c("key", "size", "last_modified", "etag"))
  b <- buckets_s3(config = cfg)
  expect_equal(b$name, "test")
  expect_s3_class(b$created, "POSIXct")
})

test_that("transient errors are retried, client errors are not", {
  fake <- local_fake_s3()
  cfg <- fake_config(max_tries = 2)
  write_s3("ok", "flaky.txt", config = cfg)
  fake$state$fail_first <- 1L
  n <- length(fake$state$requests)
  expect_warning(out <- read_s3("flaky.txt", config = cfg), class = "s3pe_warning_retry")
  expect_equal(out, "ok")
  expect_equal(length(fake$state$requests), n + 2)

  fake$state$fail_first <- 5L
  expect_warning(
    expect_error(read_s3("flaky.txt", config = cfg), class = "s3pe_error_http"),
    class = "s3pe_warning_retry"
  )
  cnd <- tryCatch(
    suppressWarnings(read_s3("flaky.txt", config = cfg)),
    s3pe_error_http = function(e) e
  )
  expect_equal(cnd$status, 503L)
  expect_equal(cnd$code, "SlowDown")

  n <- length(fake$state$requests)
  expect_error(read_s3("s3://forbidden/x", config = cfg), class = "s3pe_error_access")
  expect_equal(length(fake$state$requests), n + 1)
  expect_error(read_s3("s3://forbidden/x", config = cfg), class = "s3pe_error")
})

test_that("connection failures become transport errors after the retries", {
  fake <- local_fake_s3()
  cfg <- fake_config(max_tries = 2)
  expect_warning(
    expect_error(read_s3("boom", config = cfg), class = "s3pe_error_transport"),
    class = "s3pe_warning_retry"
  )
  cfg1 <- fake_config(max_tries = 1)
  expect_error(read_s3("boom", config = cfg1), class = "s3pe_error_transport")
})

test_that("integrity is checked on upload", {
  fake <- local_fake_s3()
  cfg <- fake_config()
  expect_error(write_s3("data", "bad-etag", config = cfg), class = "s3pe_error_integrity")
})

test_that("files go up and down", {
  fake <- local_fake_s3()
  cfg <- fake_config()
  dir <- withr::local_tempdir()
  src <- file.path(dir, "painel.parquet")
  big <- as.raw((0:299999) %% 251)
  writeBin(big, src)

  out <- upload_s3(src, "etl/", config = cfg)
  expect_equal(out$key, "etl/painel.parquet")
  expect_equal(out$size, length(big))
  expect_equal(fake$state$objects[["test/etl/painel.parquet"]]$type, "application/vnd.apache.parquet")
  expect_equal(upload_s3(src, config = cfg)$key, "painel.parquet")
  expect_equal(upload_s3(src, "s3://test/x/y.bin", content_type = "text/plain", config = cfg)$key, "x/y.bin")
  expect_equal(fake$state$objects[["test/x/y.bin"]]$type, "text/plain")

  dest <- file.path(dir, "sub", "copy.parquet")
  out <- download_s3("etl/painel.parquet", dest, config = cfg)
  expect_equal(out$size, length(big))
  expect_equal(readBin(dest, "raw", 1e6), big)
  expect_false(file.exists(paste0(dest, ".s3pe-part")))

  expect_error(download_s3("etl/painel.parquet", dest, config = cfg), class = "s3pe_error_exists")
  expect_no_error(download_s3("etl/painel.parquet", dest, overwrite = TRUE, config = cfg))

  missing <- file.path(dir, "missing")
  expect_error(download_s3("etl/missing.parquet", missing, config = cfg), class = "s3pe_error_not_found")
  expect_false(file.exists(missing))
  expect_false(file.exists(paste0(missing, ".s3pe-part")))
  expect_error(upload_s3(file.path(dir, "nope"), "k", config = cfg), class = "s3pe_error_io")
})
