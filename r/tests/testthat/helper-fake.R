# A fake S3 server for httr2's mocked responses. It verifies every
# signature with the package's own signer, keeps objects in memory and can
# misbehave on demand (503s, 403s, wrong ETags).

FAKE_ACCESS <- "GKtest"
FAKE_SECRET <- "secret-of-the-fake"
FAKE_PAGE <- 2L

fake_config <- function(...) {
  s3_config(
    endpoint = "https://fake.s3.test", region = "us-east-1",
    access_key = FAKE_ACCESS, secret_key = FAKE_SECRET, bucket = "test", ...
  )
}

url_decode <- function(x) {
  utils::URLdecode(x)
}

new_fake_s3 <- function() {
  state <- new.env(parent = emptyenv())
  state$objects <- list()   # key "bucket/key" -> list(data = raw, type = chr)
  state$requests <- character()
  state$fail_first <- 0L

  mock <- function(req) {
    method <- httr2::req_get_method(req)
    url <- req$url
    headers <- httr2::req_get_headers(req, "reveal")
    names(headers) <- tolower(names(headers))
    hdr <- function(n) headers[[n]] %||% ""

    parts <- strsplit(sub("^https://fake.s3.test", "", url), "?", fixed = TRUE)[[1]]
    path <- parts[1]
    query <- list()
    if (length(parts) > 1) {
      for (kv in strsplit(parts[2], "&", fixed = TRUE)[[1]]) {
        p <- strsplit(kv, "=", fixed = TRUE)[[1]]
        query[[url_decode(p[1])]] <- url_decode(if (length(p) > 1) p[2] else "")
      }
    }

    body <- raw()
    content_type <- ""
    if (!is.null(req$body)) {
      content_type <- req$body$content_type %||% ""
      body <- if (req$body$type == "file") {
        readBin(req$body$data, "raw", file.size(req$body$data))
      } else {
        req$body$data
      }
    }
    digest_ok <- identical(hdr("x-amz-content-sha256"), hash_hex(openssl::sha256(body)))
    expected <- sigv4_sign(
      method = method, host = "fake.s3.test", path = path, query = query,
      payload_hash = hdr("x-amz-content-sha256"), amz_date = hdr("x-amz-date"),
      access_key = FAKE_ACCESS, secret_key = FAKE_SECRET, region = "us-east-1"
    )
    ok <- identical(hdr("authorization"), expected$Authorization)

    segs <- strsplit(sub("^/", "", path), "/", fixed = TRUE)[[1]]
    bucket <- url_decode(if (length(segs) >= 1) segs[1] else "")
    key <- url_decode(if (length(segs) >= 2) paste(segs[-1], collapse = "/") else "")
    id <- paste0(bucket, "/", key)
    state$requests <- c(state$requests, paste(method, path))

    respond <- function(status, headers = list(), body = raw()) {
      if (is.character(body)) body <- charToRaw(body)
      if (is.null(headers[["content-length"]]) && method != "HEAD") {
        headers[["content-length"]] <- as.character(length(body))
      }
      httr2::response(status_code = status, url = url, method = method,
                      headers = headers, body = body)
    }
    err <- function(status, code, msg) {
      respond(status, list(`content-type` = "application/xml"),
              sprintf("<Error><Code>%s</Code><Message>%s</Message></Error>", code, msg))
    }

    if (!digest_ok) return(err(400L, "BadDigest", "payload hash does not match"))
    if (!ok) return(err(403L, "SignatureDoesNotMatch", "bad signature"))
    if (bucket == "forbidden") return(err(403L, "AccessDenied", "Access Denied"))
    if (grepl("flaky", key) && state$fail_first > 0) {
      state$fail_first <- state$fail_first - 1L
      return(err(503L, "SlowDown", "try later"))
    }
    if (key == "boom") stop("connection reset")
    if (!nzchar(bucket)) {
      return(respond(200L, list(`content-type` = "application/xml"),
        "<ListAllMyBucketsResult><Buckets><Bucket><Name>test</Name><CreationDate>2026-01-01T00:00:00.000Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>"))
    }
    if (method == "GET" && !nzchar(key)) {
      prefix <- query$prefix %||% ""
      from <- as.integer(query$`continuation-token` %||% "0")
      keys <- sub("^test/", "", names(state$objects))
      keys <- keys[startsWith(names(state$objects), paste0(bucket, "/")) & startsWith(keys, prefix)]
      keys <- sort(keys, method = "radix")
      page <- keys[seq_len(length(keys)) > from & seq_len(length(keys)) <= from + FAKE_PAGE]
      truncated <- from + FAKE_PAGE < length(keys)
      items <- vapply(page, function(k) {
        d <- state$objects[[paste0(bucket, "/", k)]]$data
        sprintf("<Contents><Key>%s</Key><Size>%d</Size><LastModified>2026-10-23T12:00:00.000Z</LastModified><ETag>\"%s\"</ETag></Contents>",
                gsub("&", "&amp;", k, fixed = TRUE), length(d), hash_hex(openssl::md5(d)))
      }, character(1))
      xml <- paste0("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
                    paste(items, collapse = ""),
                    sprintf("<IsTruncated>%s</IsTruncated>", tolower(truncated)),
                    if (truncated) sprintf("<NextContinuationToken>%d</NextContinuationToken>", from + FAKE_PAGE) else "",
                    "</ListBucketResult>")
      return(respond(200L, list(`content-type` = "application/xml"), xml))
    }
    if (method == "PUT") {
      etag <- if (key == "bad-etag") strrep("0", 32) else hash_hex(openssl::md5(body))
      state$objects[[id]] <- list(data = body, type = content_type)
      return(respond(200L, list(etag = sprintf("\"%s\"", etag))))
    }
    if (method == "DELETE") {
      state$objects[[id]] <- NULL
      return(respond(204L))
    }
    obj <- state$objects[[id]]
    if (is.null(obj)) return(err(404L, "NoSuchKey", "The specified key does not exist."))
    h <- list(
      etag = sprintf("\"%s\"", hash_hex(openssl::md5(obj$data))),
      `content-type` = obj$type,
      `last-modified` = "Thu, 23 Oct 2026 12:00:00 GMT",
      `content-length` = as.character(length(obj$data))
    )
    respond(200L, h, if (method == "HEAD") raw() else obj$data)
  }
  list(mock = mock, state = state)
}

local_fake_s3 <- function(env = parent.frame()) {
  fake <- new_fake_s3()
  httr2::local_mocked_responses(fake$mock, env = env)
  fake
}
