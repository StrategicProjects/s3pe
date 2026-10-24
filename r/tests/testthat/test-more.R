# 0.3.0: multipart, copy, batch delete, one-level listing, metadata,
# pre-signed URLs and sync, against the fake server.

MiB <- 1024^2

test_that("large bodies go up in parts, from memory and from a file", {
  fake <- local_fake_s3()
  cfg <- fake_config(multipart_threshold = 1 * MiB, part_size = 5 * MiB)
  big <- as.raw((0:(11 * MiB + 6)) %% 253)

  out <- write_s3(big, "big/mem.bin", config = cfg,
                  metadata = c(origin = "test"), content_type = "application/x-test")
  expect_equal(out$size, length(big))
  expect_match(out$etag, "-3$")
  o <- fake$state$objects[["test/big/mem.bin"]]
  expect_identical(o$data, big)
  expect_equal(o$type, "application/x-test")
  expect_equal(o$meta$origin, "test")
  expect_length(fake$state$uploads, 0)
  expect_equal(sum(grepl("partNumber", fake$state$requests)), 3)
  expect_identical(read_s3("big/mem.bin", config = cfg, format = "raw"), big)
  expect_equal(head_s3("big/mem.bin", config = cfg)$metadata[[1]]$origin, "test")

  dir <- withr::local_tempdir()
  f <- file.path(dir, "big.bin")
  writeBin(big, f)
  out <- upload_s3(f, "big/file.bin", config = cfg)
  expect_match(out$etag, "-3$")
  expect_identical(fake$state$objects[["test/big/file.bin"]]$data, big)
  expect_equal(expected_etag(f, 1 * MiB, 5 * MiB), out$etag)
  expect_equal(expected_etag(f, 20 * MiB, 5 * MiB), hash_hex(openssl::md5(big)))

  # a refused part aborts the upload and leaves nothing behind
  fake$state$fail_part <- 2L
  expect_warning(
    expect_error(write_s3(big, "big/fails.bin", config = cfg), class = "s3pe_error_http"),
    class = "s3pe_warning_abort"
  )
  expect_length(fake$state$uploads, 0)
  expect_null(fake$state$objects[["test/big/fails.bin"]])
  expect_true(any(grepl("^DELETE .*uploadId", fake$state$requests)))
})

test_that("metadata and headers travel with a plain PUT", {
  fake <- local_fake_s3()
  cfg <- fake_config()
  write_s3("1", "a/one.txt", config = cfg,
           metadata = c(Author = "ana"),
           headers = c("Cache-Control" = "max-age=60", "x-amz-storage-class" = "STANDARD"))
  h <- head_s3("a/one.txt", config = cfg)
  expect_equal(h$metadata[[1]], list(author = "ana"))
  expect_error(write_s3("1", "a/two.txt", config = cfg, metadata = c("x")), class = "s3pe_error_input")
})

test_that("copy_s3 copies on the server and keeps metadata", {
  fake <- local_fake_s3()
  cfg <- fake_config()
  write_s3("1", "a/one.txt", config = cfg, metadata = c(author = "ana"))
  out <- copy_s3("a/one.txt", "s3://test/b/one-copy.txt", config = cfg)
  expect_equal(out$key, "b/one-copy.txt")
  expect_equal(out$size, 1)
  expect_equal(read_s3("b/one-copy.txt", config = cfg), "1")
  expect_equal(head_s3("b/one-copy.txt", config = cfg)$metadata[[1]]$author, "ana")
  expect_error(copy_s3("a/missing", "b/x", config = cfg), class = "s3pe_error_not_found")
})

test_that("delete_s3 deletes many keys in one request and reports refusals", {
  fake <- local_fake_s3()
  cfg <- fake_config()
  for (i in 0:4) write_s3("x", sprintf("d/%d.txt", i), config = cfg)
  write_s3("x", "d/a&b.txt", config = cfg)
  keys <- list_s3("d/", config = cfg)$key
  expect_length(keys, 6)
  expect_equal(delete_s3(keys, config = cfg), 6L)
  expect_equal(nrow(list_s3("d/", config = cfg)), 0)
  expect_equal(sum(grepl("\\?delete", fake$state$requests)), 1)
  expect_equal(delete_s3(character(), config = cfg), 0L)

  write_s3("x", "undeletable", config = cfg)
  write_s3("x", "a/one.txt", config = cfg)
  expect_error(delete_s3(c("undeletable", "a/one.txt"), config = cfg), class = "s3pe_error_http")
  expect_false(exists_s3("a/one.txt", config = cfg))
})

test_that("list_s3 with a delimiter lists one level", {
  fake <- local_fake_s3()
  cfg <- fake_config()
  for (i in 0:4) write_s3("x", sprintf("p/%d.txt", i), config = cfg)
  write_s3("a", "p/sub/a.txt", config = cfg)
  write_s3("b", "p/sub/deeper/b.txt", config = cfg)
  write_s3("x", "q/other.txt", config = cfg)
  expect_equal(nrow(list_s3("p/", config = cfg)), 7)
  one <- list_s3("p/", config = cfg, delimiter = "/")
  expect_equal(nrow(one), 6)
  expect_equal(one$key[1], "p/sub/")
  expect_true(is.na(one$size[1]) && is.na(one$etag[1]) && is.na(one$last_modified[1]))
  expect_equal(one$key[-1], sprintf("p/%d.txt", 0:4))
  root <- list_s3(config = cfg, delimiter = "/")
  expect_equal(root$key, c("p/", "q/"))
})

test_that("pre-signed URLs are accepted without credentials", {
  fake <- local_fake_s3()
  cfg <- fake_config()
  write_s3(charToRaw("%PDF"), "pub/report.pdf", config = cfg)
  url <- presign_s3("pub/report.pdf", config = cfg, expires = 600)
  expect_true(startsWith(url, "https://fake.s3.test/test/pub/report.pdf?X-Amz-Algorithm=AWS4-HMAC-SHA256&"))
  expect_match(url, "X-Amz-Expires=600")
  resp <- httr2::request(url) |> httr2::req_perform()
  expect_equal(httr2::resp_status(resp), 200L)
  expect_equal(httr2::resp_body_raw(resp), charToRaw("%PDF"))
  tampered <- sub("report.pdf", "other.pdf", url, fixed = TRUE)
  resp <- httr2::request(tampered) |> httr2::req_error(is_error = function(r) FALSE) |> httr2::req_perform()
  expect_equal(httr2::resp_status(resp), 403L)
  expect_match(presign_s3("k", config = cfg, expires = 1e9), "X-Amz-Expires=604800")
  expect_error(presign_s3("k", config = cfg, method = "DELETE"))
})

test_that("sync_up_s3 and sync_down_s3 move only what differs", {
  fake <- local_fake_s3()
  cfg <- fake_config()
  dir <- withr::local_tempdir()
  dir.create(file.path(dir, "sub"))
  writeLines("a", file.path(dir, "a.csv"))
  writeLines("b", file.path(dir, "sub", "b.csv"))
  writeLines("{}", file.path(dir, "manifest.json"))
  writeBin(raw(1), file.path(dir, "x.s3pe-part"))

  r <- sync_up_s3(dir, "s3://test/out", config = cfg, delete = TRUE, last = "manifest.json")
  expect_s3_class(r, "s3pe_sync")
  expect_equal(r$transferred, c("a.csv", "sub/b.csv", "manifest.json"))
  expect_length(r$skipped, 0)
  expect_equal(nrow(list_s3("out/", config = cfg)), 3)
  expect_output(print(r), "3 transferred")

  r <- sync_up_s3(dir, "out/", config = cfg, delete = TRUE)
  expect_length(r$transferred, 0)
  expect_length(r$skipped, 3)
  writeLines("a2", file.path(dir, "a.csv"))
  write_s3("old", "out/stale.txt", config = cfg)
  r <- sync_up_s3(dir, "out/", config = cfg, delete = TRUE)
  expect_equal(r$transferred, "a.csv")
  expect_equal(r$deleted, "stale.txt")
  expect_false(exists_s3("out/stale.txt", config = cfg))
  expect_error(sync_up_s3(dir, "out/", config = cfg, last = "nope"), class = "s3pe_error_input")

  down <- withr::local_tempdir()
  writeLines("x", file.path(down, "extra.txt"))
  r <- sync_down_s3("out/", down, config = cfg, delete = TRUE)
  expect_length(r$transferred, 3)
  expect_equal(r$deleted, "extra.txt")
  expect_equal(readLines(file.path(down, "sub", "b.csv")), "b")
  expect_equal(readLines(file.path(down, "a.csv")), "a2")
  r <- sync_down_s3("out/", down, config = cfg)
  expect_length(r$transferred, 0)
  expect_length(r$skipped, 3)
})
