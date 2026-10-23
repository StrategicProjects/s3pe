test_that("the AWS documentation example signs as documented (D8)", {
  headers <- sigv4_sign(
    method = "GET", host = "examplebucket.s3.amazonaws.com", path = "/test.txt",
    query = list(), payload_hash = EMPTY_SHA256, amz_date = "20130524T000000Z",
    access_key = "AKIAIOSFODNN7EXAMPLE",
    secret_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    region = "us-east-1", extra_headers = c(range = "bytes=0-9")
  )
  expect_equal(
    headers$Authorization,
    paste0(
      "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, ",
      "SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, ",
      "Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
    )
  )
  expect_equal(headers[["x-amz-date"]], "20130524T000000Z")
  expect_equal(headers[["range"]], "bytes=0-9")
  expect_null(headers[["host"]])
})

test_that("session tokens are signed and sent", {
  h <- sigv4_sign("GET", "h", "/", list(), EMPTY_SHA256, "20130524T000000Z",
                  "a", "s", session_token = "tok")
  expect_equal(h[["x-amz-security-token"]], "tok")
  expect_match(h$Authorization, "SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token")
})

test_that("uri_encode keeps unreserved characters and slashes in paths", {
  expect_equal(uri_encode("a b/ç~x.y_z-1", keep_slash = TRUE), "a%20b/%C3%A7~x.y_z-1")
  expect_equal(uri_encode("a/b"), "a%2Fb")
  expect_equal(uri_encode("*+"), "%2A%2B")
  expect_equal(uri_encode(""), "")
  expect_equal(
    uri_encode("pasta/ação & x+y (1).csv", keep_slash = TRUE),
    "pasta/a%C3%A7%C3%A3o%20%26%20x%2By%20%281%29.csv"
  )
})

test_that("canonical_query sorts and encodes", {
  expect_equal(
    canonical_query(list(prefix = "dados/x y", `list-type` = "2")),
    "list-type=2&prefix=dados%2Fx%20y"
  )
  expect_equal(canonical_query(list()), "")
})

test_that("amz_date is UTC", {
  t <- as.POSIXct("2013-05-24 00:00:00", tz = "UTC")
  expect_equal(amz_date(t), "20130524T000000Z")
})
