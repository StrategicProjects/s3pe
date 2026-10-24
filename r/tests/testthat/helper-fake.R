# A fake S3 server for httr2's mocked responses. It verifies every
# signature with the package's own signer (header style, and query style
# for pre-signed URLs), keeps objects and metadata in memory, supports
# multipart uploads, copies, batch deletes and one-level listings, and can
# misbehave on demand (503s, 403s, wrong ETags, a refused part).

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

md5_hex <- function(x) hash_hex(openssl::md5(x))

new_fake_s3 <- function() {
  state <- new.env(parent = emptyenv())
  state$objects <- list()   # "bucket/key" -> list(data, type, meta, etag)
  state$uploads <- list()   # upload id -> list of parts (raw)
  state$requests <- character()
  state$fail_first <- 0L
  state$fail_part <- NA_integer_
  state$next_upload <- 0L

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
    q <- function(n) query[[n]]

    body <- raw()
    content_type <- hdr("content-type")
    if (!is.null(req$body)) {
      content_type <- req$body$content_type %||% content_type
      body <- if (req$body$type == "file") {
        readBin(req$body$data, "raw", file.size(req$body$data))
      } else {
        req$body$data
      }
    }

    if (!is.null(q("X-Amz-Signature"))) {
      plain <- query[!startsWith(names(query), "X-Amz-")]
      expected <- sigv4_presign(
        method = method, host = "fake.s3.test", path = path, query = plain,
        amz_date = q("X-Amz-Date"), expires = as.integer(q("X-Amz-Expires")),
        access_key = FAKE_ACCESS, secret_key = FAKE_SECRET, region = "us-east-1"
      )
      ok <- endsWith(expected, paste0("X-Amz-Signature=", q("X-Amz-Signature")))
      digest_ok <- TRUE
    } else {
      digest_ok <- identical(hdr("x-amz-content-sha256"), hash_hex(openssl::sha256(body)))
      extra <- unlist(headers[startsWith(names(headers), "x-amz-") &
                                !names(headers) %in% c("x-amz-date", "x-amz-content-sha256")])
      expected <- sigv4_sign(
        method = method, host = "fake.s3.test", path = path, query = query,
        payload_hash = hdr("x-amz-content-sha256"), amz_date = hdr("x-amz-date"),
        access_key = FAKE_ACCESS, secret_key = FAKE_SECRET, region = "us-east-1",
        extra_headers = extra %||% character()
      )
      ok <- identical(hdr("authorization"), expected$Authorization)
    }

    segs <- strsplit(sub("^/", "", path), "/", fixed = TRUE)[[1]]
    bucket <- url_decode(if (length(segs) >= 1) segs[1] else "")
    key <- url_decode(if (length(segs) >= 2) paste(segs[-1], collapse = "/") else "")
    id <- paste0(bucket, "/", key)
    state$requests <- c(state$requests, paste(method, sub("^https://fake.s3.test", "", url)))

    respond <- function(status, headers = list(), body = raw()) {
      if (is.character(body)) body <- charToRaw(body)
      if (is.null(headers[["content-length"]]) && method != "HEAD") {
        headers[["content-length"]] <- as.character(length(body))
      }
      httr2::response(status_code = status, url = url, method = method,
                      headers = headers, body = body)
    }
    xml <- function(status, body) respond(status, list(`content-type` = "application/xml"), body)
    err <- function(status, code, msg) {
      xml(status, sprintf("<Error><Code>%s</Code><Message>%s</Message></Error>", code, msg))
    }
    meta_of <- function() {
      m <- headers[startsWith(names(headers), "x-amz-meta-")]
      names(m) <- sub("^x-amz-meta-", "", names(m))
      m
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
      return(xml(200L, "<ListAllMyBucketsResult><Buckets><Bucket><Name>test</Name><CreationDate>2026-01-01T00:00:00.000Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>"))
    }
    # batch delete
    if (method == "POST" && !nzchar(key) && !is.null(q("delete"))) {
      if (!identical(hdr("content-md5"), as.character(openssl::base64_encode(openssl::md5(body))))) {
        return(err(400L, "InvalidDigest", "Content-MD5 does not match"))
      }
      text <- rawToChar(body)
      keys <- regmatches(text, gregexpr("<Key>[^<]*</Key>", text))[[1]]
      keys <- xml_unescape(gsub("</?Key>", "", keys))
      out <- "<DeleteResult>"
      for (k in keys) {
        if (k == "undeletable") {
          out <- paste0(out, "<Error><Key>undeletable</Key><Code>AccessDenied</Code><Message>nope</Message></Error>")
        } else {
          state$objects[[paste0(bucket, "/", k)]] <- NULL
        }
      }
      return(xml(200L, paste0(out, "</DeleteResult>")))
    }
    if (method == "GET" && !nzchar(key)) {
      prefix <- q("prefix") %||% ""
      delimiter <- q("delimiter")
      from <- as.integer(q("continuation-token") %||% "0")
      all_names <- names(state$objects) %||% character()
      all_keys <- sub(paste0("^", bucket, "/"), "", all_names)
      all_keys <- all_keys[startsWith(all_names, paste0(bucket, "/")) & startsWith(all_keys, prefix)]
      all_keys <- all_keys[!startsWith(all_keys, "__init__/")]
      all_keys <- sort(all_keys, method = "radix")
      prefixes <- character()
      if (!is.null(delimiter)) {
        rest <- substring(all_keys, nchar(prefix) + 1)
        has <- grepl(delimiter, rest, fixed = TRUE)
        prefixes <- unique(paste0(prefix, sub(paste0("\\Q", delimiter, "\\E.*$"), "", rest[has]), delimiter))
        all_keys <- all_keys[!has]
      }
      page <- all_keys[seq_along(all_keys) > from & seq_along(all_keys) <= from + FAKE_PAGE]
      truncated <- from + FAKE_PAGE < length(all_keys)
      items <- vapply(page, function(k) {
        o <- state$objects[[paste0(bucket, "/", k)]]
        sprintf("<Contents><Key>%s</Key><Size>%d</Size><LastModified>2026-10-23T12:00:00.000Z</LastModified><ETag>\"%s\"</ETag></Contents>",
                xml_escape(k), length(o$data), o$etag)
      }, character(1))
      cps <- if (from == 0) paste0(sprintf("<CommonPrefixes><Prefix>%s</Prefix></CommonPrefixes>", prefixes), collapse = "") else ""
      out <- paste0("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
                    paste(items, collapse = ""), cps,
                    sprintf("<IsTruncated>%s</IsTruncated>", tolower(truncated)),
                    if (truncated) sprintf("<NextContinuationToken>%d</NextContinuationToken>", from + FAKE_PAGE) else "",
                    "</ListBucketResult>")
      return(xml(200L, out))
    }
    # multipart: initiate
    if (method == "POST" && !is.null(q("uploads"))) {
      state$next_upload <- state$next_upload + 1L
      uid <- paste0("upload-", state$next_upload)
      state$uploads[[uid]] <- list()
      state$objects[[paste0(bucket, "/__init__/", uid)]] <- list(data = raw(), type = content_type, meta = meta_of(), etag = "")
      return(xml(200L, sprintf("<InitiateMultipartUploadResult><UploadId>%s</UploadId></InitiateMultipartUploadResult>", uid)))
    }
    # multipart: part
    if (method == "PUT" && !is.null(q("partNumber"))) {
      n <- as.integer(q("partNumber"))
      uid <- q("uploadId")
      if (!is.na(state$fail_part) && state$fail_part == n) return(err(500L, "InternalError", "part refused"))
      if (is.null(state$uploads[[uid]])) return(err(404L, "NoSuchUpload", "no such upload"))
      state$uploads[[uid]][[as.character(n)]] <- body
      return(respond(200L, list(etag = sprintf("\"%s\"", md5_hex(body)))))
    }
    # multipart: complete
    if (method == "POST" && !is.null(q("uploadId"))) {
      uid <- q("uploadId")
      parts <- state$uploads[[uid]]
      if (is.null(parts)) return(err(404L, "NoSuchUpload", "no such upload"))
      parts <- parts[order(as.integer(names(parts)))]
      data <- do.call(c, unname(parts))
      etag <- multipart_etag(vapply(parts, md5_hex, character(1)))
      init <- state$objects[[paste0(bucket, "/__init__/", uid)]]
      state$objects[[paste0(bucket, "/__init__/", uid)]] <- NULL
      state$uploads[[uid]] <- NULL
      state$objects[[id]] <- list(data = data, type = init$type, meta = init$meta, etag = etag)
      return(xml(200L, sprintf("<CompleteMultipartUploadResult><ETag>\"%s\"</ETag></CompleteMultipartUploadResult>", etag)))
    }
    # multipart: abort
    if (method == "DELETE" && !is.null(q("uploadId"))) {
      uid <- q("uploadId")
      state$uploads[[uid]] <- NULL
      state$objects[[paste0(bucket, "/__init__/", uid)]] <- NULL
      return(respond(204L))
    }
    # copy
    if (method == "PUT" && nzchar(hdr("x-amz-copy-source"))) {
      src <- strsplit(sub("^/", "", hdr("x-amz-copy-source")), "/", fixed = TRUE)[[1]]
      sid <- paste0(url_decode(src[1]), "/", url_decode(paste(src[-1], collapse = "/")))
      o <- state$objects[[sid]]
      if (is.null(o)) return(err(404L, "NoSuchKey", "no source"))
      state$objects[[id]] <- o
      return(xml(200L, sprintf("<CopyObjectResult><ETag>\"%s\"</ETag></CopyObjectResult>", o$etag)))
    }
    if (method == "PUT") {
      etag <- if (key == "bad-etag") strrep("0", 32) else md5_hex(body)
      state$objects[[id]] <- list(data = body, type = content_type, meta = meta_of(), etag = etag)
      return(respond(200L, list(etag = sprintf("\"%s\"", etag))))
    }
    if (method == "DELETE") {
      state$objects[[id]] <- NULL
      return(respond(204L))
    }
    obj <- state$objects[[id]]
    if (is.null(obj)) return(err(404L, "NoSuchKey", "The specified key does not exist."))
    h <- list(
      etag = sprintf("\"%s\"", obj$etag),
      `content-type` = obj$type,
      `last-modified` = "Thu, 23 Oct 2026 12:00:00 GMT",
      `content-length` = as.character(length(obj$data))
    )
    for (m in names(obj$meta)) h[[paste0("x-amz-meta-", m)]] <- obj$meta[[m]]
    respond(200L, h, if (method == "HEAD") raw() else obj$data)
  }
  list(mock = mock, state = state)
}

local_fake_s3 <- function(env = parent.frame()) {
  fake <- new_fake_s3()
  httr2::local_mocked_responses(fake$mock, env = env)
  fake
}
