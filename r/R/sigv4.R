# AWS Signature Version 4 for S3, header style. Signed headers: host,
# x-amz-content-sha256, x-amz-date (and x-amz-security-token when there is
# a session token). The payload hash is always the real SHA-256 of the
# body: never UNSIGNED-PAYLOAD, never aws-chunked (some stores reject it).

EMPTY_SHA256 <- "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

#' Percent-encode the way AWS canonicalises
#'
#' Unreserved characters (`A-Z a-z 0-9 - _ . ~`) are kept, everything else
#' becomes `%XX` with upper-case hex, byte by byte in UTF-8. `/` is kept
#' only when `keep_slash` is `TRUE` (object keys in the path).
#' @noRd
uri_encode <- function(x, keep_slash = FALSE) {
  if (!nzchar(x)) {
    return("")
  }
  bytes <- as.integer(charToRaw(enc2utf8(x)))
  keep <- as.integer(charToRaw(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~"
  ))
  if (keep_slash) {
    keep <- c(keep, 47L)
  }
  out <- sprintf("%%%02X", bytes)
  ok <- bytes %in% keep
  out[ok] <- vapply(bytes[ok], function(b) rawToChar(as.raw(b)), character(1))
  paste(out, collapse = "")
}

#' Canonical query string: encoded pairs sorted by name, then value
#' @noRd
canonical_query <- function(query) {
  if (length(query) == 0) {
    return("")
  }
  keys <- uri_encode_each(names(query))
  values <- uri_encode_each(as.character(unlist(query, use.names = FALSE)))
  o <- order(keys, values, method = "radix")
  paste0(keys[o], "=", values[o], collapse = "&")
}

uri_encode_each <- function(x) {
  vapply(x, uri_encode, character(1), USE.NAMES = FALSE)
}

#' `YYYYMMDDTHHMMSSZ` for a time (default now), in UTC
#' @noRd
amz_date <- function(time = Sys.time()) {
  format(time, "%Y%m%dT%H%M%SZ", tz = "UTC")
}

#' Hex digest as a plain string (openssl keeps a class on its hashes)
#' @noRd
hash_hex <- function(h) {
  as.vector(as.character(h))
}

sha256_hex <- function(x) {
  hash_hex(openssl::sha256(x))
}

hmac_sha256 <- function(key, msg) {
  openssl::sha256(charToRaw(enc2utf8(msg)), key = key)
}

#' Sign a request and return the headers to add to it
#'
#' @param method HTTP method, upper case.
#' @param host Value of the `Host` header (`host[:port]`).
#' @param path Canonical path, already percent-encoded, starting with `/`.
#' @param query Named list of raw (unencoded) query parameters.
#' @param payload_hash Hex SHA-256 of the body.
#' @param amz_date Timestamp `YYYYMMDDTHHMMSSZ`.
#' @param access_key,secret_key,session_token,region Credentials and region.
#' @param extra_headers Named character vector of extra headers to sign
#'   (lower-case names); rarely needed.
#' @returns A named list of headers: `x-amz-date`, `x-amz-content-sha256`,
#'   `x-amz-security-token` (when present), the extra headers and
#'   `Authorization`.
#' @noRd
sigv4_sign <- function(method, host, path, query = list(), payload_hash,
                       amz_date, access_key, secret_key,
                       session_token = NULL, region = "us-east-1",
                       extra_headers = character()) {
  headers <- c(
    host = host,
    `x-amz-content-sha256` = payload_hash,
    `x-amz-date` = amz_date
  )
  if (!is.null(session_token) && nzchar(session_token)) {
    headers <- c(headers, `x-amz-security-token` = session_token)
  }
  if (length(extra_headers) > 0) {
    names(extra_headers) <- tolower(names(extra_headers))
    headers <- c(headers, trimws(extra_headers))
  }
  headers <- headers[order(names(headers), method = "radix")]

  canonical_headers <- paste0(names(headers), ":", headers, "\n", collapse = "")
  signed_headers <- paste(names(headers), collapse = ";")
  canonical_request <- paste(
    method, path, canonical_query(query), canonical_headers,
    signed_headers, payload_hash,
    sep = "\n"
  )

  date <- substr(amz_date, 1, 8)
  scope <- paste(date, region, "s3", "aws4_request", sep = "/")
  string_to_sign <- paste(
    "AWS4-HMAC-SHA256", amz_date, scope,
    sha256_hex(charToRaw(canonical_request)),
    sep = "\n"
  )

  k_date <- hmac_sha256(charToRaw(paste0("AWS4", secret_key)), date)
  k_region <- hmac_sha256(k_date, region)
  k_service <- hmac_sha256(k_region, "s3")
  k_signing <- hmac_sha256(k_service, "aws4_request")
  signature <- hash_hex(hmac_sha256(k_signing, string_to_sign))

  out <- as.list(headers[names(headers) != "host"])
  out$Authorization <- sprintf(
    "AWS4-HMAC-SHA256 Credential=%s/%s, SignedHeaders=%s, Signature=%s",
    access_key, scope, signed_headers, signature
  )
  out
}

#' The query string of a pre-signed URL (without the leading `?`)
#'
#' SigV4 query-string signing: the `X-Amz-*` parameters plus the signature,
#' over `UNSIGNED-PAYLOAD` and the `host` header alone (D16).
#' @param expires Validity in seconds.
#' @noRd
sigv4_presign <- function(method, host, path, query = list(), amz_date, expires,
                          access_key, secret_key, session_token = NULL,
                          region = "us-east-1") {
  date <- substr(amz_date, 1, 8)
  scope <- paste(date, region, "s3", "aws4_request", sep = "/")
  params <- c(query, list(
    `X-Amz-Algorithm` = "AWS4-HMAC-SHA256",
    `X-Amz-Credential` = paste0(access_key, "/", scope),
    `X-Amz-Date` = amz_date,
    `X-Amz-Expires` = as.character(expires),
    `X-Amz-SignedHeaders` = "host"
  ))
  if (!is.null(session_token) && nzchar(session_token)) {
    params$`X-Amz-Security-Token` <- session_token
  }
  cq <- canonical_query(params)
  canonical_request <- paste(
    method, path, cq, paste0("host:", host, "\n"), "host", "UNSIGNED-PAYLOAD",
    sep = "\n"
  )
  string_to_sign <- paste(
    "AWS4-HMAC-SHA256", amz_date, scope,
    sha256_hex(charToRaw(canonical_request)),
    sep = "\n"
  )
  k_date <- hmac_sha256(charToRaw(paste0("AWS4", secret_key)), date)
  k_region <- hmac_sha256(k_date, region)
  k_service <- hmac_sha256(k_region, "s3")
  k_signing <- hmac_sha256(k_service, "aws4_request")
  signature <- hash_hex(hmac_sha256(k_signing, string_to_sign))
  paste0(cq, "&X-Amz-Signature=", signature)
}

#' Raw bytes of a hex string
#' @noRd
hex_to_raw <- function(hex) {
  as.raw(strtoi(substring(hex, seq(1, nchar(hex) - 1, 2), seq(2, nchar(hex), 2)), 16L))
}

#' `md5(concat(part MD5 bytes))-N`: the ETag S3 gives a multipart upload
#' @noRd
multipart_etag <- function(part_md5s) {
  all <- do.call(c, lapply(part_md5s, hex_to_raw))
  paste0(hash_hex(openssl::md5(all)), "-", length(part_md5s))
}

#' `Content-MD5` of a body: base64 of its MD5
#' @noRd
content_md5 <- function(body) {
  openssl::base64_encode(openssl::md5(body))
}
