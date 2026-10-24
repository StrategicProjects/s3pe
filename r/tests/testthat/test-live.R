# Against a real server, only when S3PE_TEST_BUCKET names a bucket the
# credentials in the environment can write to and S3PE_ENDPOINT names the
# store. Everything goes under s3pe-test/<pid>/ and is deleted at the end.

test_that("live round trip", {
  bucket <- Sys.getenv("S3PE_TEST_BUCKET")
  skip_if(bucket == "", "S3PE_TEST_BUCKET not set")
  skip_if(Sys.getenv("S3PE_ENDPOINT") == "", "S3PE_ENDPOINT not set")
  cfg <- s3_config(bucket = bucket, max_tries = 3)
  prefix <- sprintf("s3pe-test/%d/", Sys.getpid())
  withr::defer({
    for (k in list_s3(prefix, config = cfg)$key) delete_s3(k, config = cfg)
  })

  key <- paste0(prefix, "ação e espaço.csv")
  df <- data.frame(a = 1:3, b = c("x", "y", "z"))
  expect_false(exists_s3(key, config = cfg))
  out <- write_s3(df, key, config = cfg)
  expect_equal(out$size, nchar("a,b\n1,x\n2,y\n3,z\n"))
  expect_equal(as.data.frame(read_s3(key, config = cfg)), df)
  h <- head_s3(key, config = cfg)
  expect_equal(h$content_type, "text/csv")
  expect_equal(h$size, out$size)

  write_s3(list(gerado_em = "2026-10-23"), paste0(prefix, "manifest.json"), config = cfg)
  expect_equal(read_s3(paste0(prefix, "manifest.json"), config = cfg)$gerado_em, "2026-10-23")

  dir <- withr::local_tempdir()
  big <- as.raw((0:2499999) %% 253)
  writeBin(big, file.path(dir, "big.bin"))
  upload_s3(file.path(dir, "big.bin"), paste0(prefix, "big.bin"), config = cfg)
  download_s3(paste0(prefix, "big.bin"), file.path(dir, "back.bin"), config = cfg)
  expect_equal(readBin(file.path(dir, "back.bin"), "raw", 3e6), big)

  listed <- list_s3(prefix, config = cfg)
  expect_equal(nrow(listed), 3)
  expect_true(key %in% listed$key)
  expect_true(bucket %in% buckets_s3(config = cfg)$name)

  for (k in listed$key) delete_s3(k, config = cfg)
  expect_equal(nrow(list_s3(prefix, config = cfg)), 0)
})
