# Against a real server, only when S3PE_TEST_BUCKET names a bucket the
# credentials in the environment can write to and S3PE_ENDPOINT names the
# store. Everything goes under s3pe-test/<pid>/ and is deleted at the end.

test_that("live round trip", {
  bucket <- Sys.getenv("S3PE_TEST_BUCKET")
  skip_if(bucket == "", "S3PE_TEST_BUCKET not set")
  skip_if(Sys.getenv("S3PE_ENDPOINT") == "", "S3PE_ENDPOINT not set")
  cfg <- s3_config(bucket = bucket, max_tries = 3,
                   multipart_threshold = 6 * 1024^2, part_size = 5 * 1024^2)
  prefix <- sprintf("s3pe-test/%d/", Sys.getpid())
  withr::defer({
    keys <- list_s3(prefix, config = cfg)$key
    if (length(keys) > 0) delete_s3(keys, config = cfg)
  })

  key <- paste0(prefix, "ação e espaço.csv")
  df <- data.frame(a = 1:3, b = c("x", "y", "z"))
  expect_false(exists_s3(key, config = cfg))
  out <- write_s3(df, key, config = cfg, metadata = c(origin = "s3pe-test"))
  expect_equal(out$size, nchar("a,b\n1,x\n2,y\n3,z\n"))
  expect_equal(as.data.frame(read_s3(key, config = cfg)), df)
  h <- head_s3(key, config = cfg)
  expect_equal(h$content_type, "text/csv")
  expect_equal(h$size, out$size)
  expect_equal(h$metadata[[1]]$origin, "s3pe-test")

  write_s3(list(gerado_em = "2026-10-23"), paste0(prefix, "manifest.json"), config = cfg)
  expect_equal(read_s3(paste0(prefix, "manifest.json"), config = cfg)$gerado_em, "2026-10-23")

  # multipart: 12 MiB above a 6 MiB threshold, parts of 5 MiB
  dir <- withr::local_tempdir()
  big <- as.raw((0:(12 * 1024^2 - 1)) %% 253)
  writeBin(big, file.path(dir, "big.bin"))
  up <- upload_s3(file.path(dir, "big.bin"), paste0(prefix, "big.bin"), config = cfg)
  expect_match(up$etag, "-3$")
  download_s3(paste0(prefix, "big.bin"), file.path(dir, "back.bin"), config = cfg)
  expect_equal(readBin(file.path(dir, "back.bin"), "raw", 2e7), big)

  # copy, one-level listing
  cp <- copy_s3(key, paste0(prefix, "copy/again.csv"), config = cfg)
  expect_equal(cp$size, out$size)
  expect_equal(head_s3(paste0(prefix, "copy/again.csv"), config = cfg)$metadata[[1]]$origin, "s3pe-test")
  one <- list_s3(prefix, config = cfg, delimiter = "/")
  expect_true(paste0(prefix, "copy/") %in% one$key)
  expect_false(paste0(prefix, "copy/again.csv") %in% one$key)

  # pre-signed GET
  url <- presign_s3(paste0(prefix, "manifest.json"), config = cfg, expires = 120)
  got <- httr2::request(url) |> httr2::req_perform() |> httr2::resp_body_string()
  expect_match(got, "2026-10-23")

  # sync up and down
  src <- withr::local_tempdir()
  dir.create(file.path(src, "sub"))
  writeLines("a", file.path(src, "a.txt"))
  writeLines("b", file.path(src, "sub", "b.txt"))
  r <- sync_up_s3(src, paste0(prefix, "sync/"), config = cfg)
  expect_equal(sort(r$transferred), c("a.txt", "sub/b.txt"))
  r <- sync_up_s3(src, paste0(prefix, "sync/"), config = cfg)
  expect_length(r$transferred, 0)
  dst <- withr::local_tempdir()
  r <- sync_down_s3(paste0(prefix, "sync/"), dst, config = cfg)
  expect_length(r$transferred, 2)
  expect_equal(readLines(file.path(dst, "sub", "b.txt")), "b")

  listed <- list_s3(prefix, config = cfg)
  expect_equal(nrow(listed), 6)
  expect_true(bucket %in% buckets_s3(config = cfg)$name)

  expect_equal(delete_s3(listed$key, config = cfg), 6L)
  expect_equal(nrow(list_s3(prefix, config = cfg)), 0)
})
