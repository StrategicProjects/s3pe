# Building, signing, performing and checking one S3 request.

TRANSIENT_STATUS <- c(429L, 500L, 502L, 503L, 504L)

#' A signed request
#'
#' @param config An `s3pe_config`.
#' @param method `"GET"`, `"PUT"`, `"HEAD"` or `"DELETE"`.
#' @param bucket,key Bucket and key (`""` for bucket- and account-level calls).
#' @param query Named list of raw query parameters.
#' @param body `NULL`, a raw vector, or a file path (class `s3pe_file`).
#' @param content_type Content type of the body, if any.
#' @param headers Named character vector of extra headers. `x-amz-*`
#'   headers are signed (D17); the others travel unsigned.
#' @returns An `httr2_request`, with the payload hashes attached as the
#'   attribute `hashes` (`sha256`, `md5`, `size`).
#' @noRd
s3_request <- function(config, method, bucket = "", key = "", query = list(),
                       body = NULL, content_type = NULL, headers = NULL,
                       call = rlang::caller_env()) {
  check_config(config, call = call)
  path <- if (!nzchar(bucket)) {
    "/"
  } else if (!nzchar(key)) {
    paste0("/", uri_encode(bucket))
  } else {
    paste0("/", uri_encode(bucket), "/", uri_encode(key, keep_slash = TRUE))
  }
  url <- paste0(config$endpoint, path)
  qs <- canonical_query(query)
  if (nzchar(qs)) {
    url <- paste0(url, "?", qs)
  }

  hashes <- payload_hashes(body)
  extra <- as.character(headers %||% character())
  names(extra) <- tolower(names(headers %||% character()))
  signed_extra <- extra[startsWith(names(extra), "x-amz-")]
  unsigned <- extra[!startsWith(names(extra), "x-amz-")]
  if (is.null(body) && !is.null(content_type)) {
    unsigned[["content-type"]] <- content_type
  }
  all_headers <- sigv4_sign(
    method = method, host = config$host, path = path, query = query,
    payload_hash = hashes$sha256, amz_date = amz_date(),
    access_key = config$access_key, secret_key = config$secret_key,
    session_token = config$session_token, region = config$region,
    extra_headers = signed_extra
  )
  all_headers <- c(all_headers, as.list(unsigned))

  req <- httr2::request(url) |>
    httr2::req_method(method) |>
    httr2::req_user_agent("s3pe R package (https://github.com/StrategicProjects/s3pe)") |>
    httr2::req_timeout(config$timeout) |>
    httr2::req_error(is_error = function(resp) FALSE)
  req <- rlang::inject(httr2::req_headers(req, !!!all_headers, .redact = "Authorization"))

  if (is.raw(body)) {
    req <- httr2::req_body_raw(req, body, type = content_type %||% "application/octet-stream")
  } else if (inherits(body, "s3pe_file")) {
    req <- httr2::req_body_file(req, unclass(body), type = content_type %||% "application/octet-stream")
  }
  attr(req, "hashes") <- hashes
  req
}

#' Mark a path as a body to stream from disk
#' @noRd
s3_file <- function(path) {
  structure(path, class = "s3pe_file")
}

payload_hashes <- function(body) {
  if (is.null(body)) {
    list(sha256 = EMPTY_SHA256, md5 = NA_character_, size = 0)
  } else if (is.raw(body)) {
    list(
      sha256 = hash_hex(openssl::sha256(body)),
      md5 = hash_hex(openssl::md5(body)),
      size = length(body)
    )
  } else if (inherits(body, "s3pe_file")) {
    path <- unclass(body)
    list(
      sha256 = hash_hex(openssl::sha256(file(path, raw = TRUE))),
      md5 = hash_hex(openssl::md5(file(path, raw = TRUE))),
      size = file.size(path)
    )
  } else {
    cli::cli_abort("Internal: unsupported body of class {.cls {class(body)}}.")
  }
}

#' Perform with retries and turn HTTP failures into classed errors
#'
#' Transient statuses (429, 500, 502, 503, 504) and connection failures are
#' retried up to `config$max_tries` times with a 1 s, 2 s, 4 s... pause.
#' Other failures are errors at once.
#'
#' @param path When given, the body is streamed to this file.
#' @returns The `httr2_response` (2xx).
#' @noRd
s3_perform <- function(req, config, method, bucket, key, path = NULL,
                       call = rlang::caller_env()) {
  max_tries <- config$max_tries
  for (i in seq_len(max_tries)) {
    resp <- tryCatch(
      httr2::req_perform(req, path = path),
      error = function(cnd) cnd
    )
    if (inherits(resp, "httr2_response")) {
      status <- httr2::resp_status(resp)
      if (status %in% TRANSIENT_STATUS && i < max_tries) {
        wait <- 2^(i - 1)
        cli::cli_warn(
          "HTTP {status} from {method} {s3_url(bucket, key)}; retrying in {wait} s ({i}/{max_tries}).",
          class = "s3pe_warning_retry"
        )
        Sys.sleep(wait)
        next
      }
      return(check_response(resp, method, bucket, key, call = call))
    }
    if (i < max_tries) {
      wait <- 2^(i - 1)
      cli::cli_warn(
        "{conditionMessage(resp)}; retrying in {wait} s ({i}/{max_tries}).",
        class = "s3pe_warning_retry"
      )
      Sys.sleep(wait)
      next
    }
    cli::cli_abort(
      c("Could not {method} {s3_url(bucket, key)} at {.url {config$endpoint}}.",
        "x" = conditionMessage(resp)),
      class = c("s3pe_error_transport", "s3pe_error"),
      call = call
    )
  }
}

s3_url <- function(bucket, key) {
  if (!nzchar(bucket)) "the account" else paste0("s3://", bucket, "/", key)
}

check_response <- function(resp, method, bucket, key, call) {
  status <- httr2::resp_status(resp)
  if (status < 300L) {
    return(resp)
  }
  code <- ""
  message <- ""
  if (method != "HEAD") {
    body <- tryCatch(httr2::resp_body_string(resp), error = function(e) "")
    code <- xml_text(body, "Code")
    message <- xml_text(body, "Message")
  }
  url <- s3_url(bucket, key)
  if (status == 404L) {
    cli::cli_abort(
      "Not found: {url}.",
      class = c("s3pe_error_not_found", "s3pe_error"),
      bucket = bucket, key = key, status = status, call = call
    )
  }
  if (status == 403L) {
    cli::cli_abort(
      c("Access denied: {url}.",
        "i" = if (nzchar(message)) message else "Check the credentials and the bucket's permissions."),
      class = c("s3pe_error_access", "s3pe_error"),
      bucket = bucket, key = key, status = status, call = call
    )
  }
  cli::cli_abort(
    c("HTTP {status}{if (nzchar(code)) paste0(' ', code)} for {method} {url}.",
      "x" = if (nzchar(message)) message else httr2::resp_status_desc(resp)),
    class = c("s3pe_error_http", "s3pe_error"),
    bucket = bucket, key = key, status = status, code = code, call = call
  )
}

resp_etag <- function(resp) {
  etag <- httr2::resp_header(resp, "etag", default = "")
  gsub("\"", "", etag, fixed = TRUE)
}

looks_like_md5 <- function(etag) {
  nchar(etag) == 32 && grepl("^[0-9a-fA-F]{32}$", etag)
}

check_upload <- function(bucket, key, md5, etag, call) {
  if (looks_like_md5(etag) && tolower(etag) != tolower(md5)) {
    cli::cli_abort(
      c("Upload of {s3_url(bucket, key)} failed the integrity check.",
        "x" = "The server's ETag {etag} differs from the MD5 of what was sent ({md5})."),
      class = c("s3pe_error_integrity", "s3pe_error"), call = call
    )
  }
  invisible(TRUE)
}

check_download <- function(bucket, key, size, expected, etag, md5, call) {
  if (!is.na(expected) && expected != size) {
    cli::cli_abort(
      c("Download of {s3_url(bucket, key)} failed the integrity check.",
        "x" = "Received {size} bytes, Content-Length said {expected}."),
      class = c("s3pe_error_integrity", "s3pe_error"), call = call
    )
  }
  if (looks_like_md5(etag) && !is.na(md5) && tolower(etag) != tolower(md5)) {
    cli::cli_abort(
      c("Download of {s3_url(bucket, key)} failed the integrity check.",
        "x" = "MD5 of what arrived ({md5}) differs from the ETag {etag}."),
      class = c("s3pe_error_integrity", "s3pe_error"), call = call
    )
  }
  invisible(TRUE)
}

resp_content_length <- function(resp) {
  v <- httr2::resp_header(resp, "content-length", default = NA_character_)
  suppressWarnings(as.numeric(v))
}

#' Objects, common prefixes and continuation token of a ListObjectsV2 document
#' @noRd
parse_listing <- function(xml) {
  blocks <- xml_blocks(xml, "Contents")
  objects <- tibble::tibble(
    key = vapply(blocks, xml_text, character(1), tag = "Key", USE.NAMES = FALSE),
    size = as.numeric(vapply(blocks, xml_text, character(1), tag = "Size", USE.NAMES = FALSE)),
    last_modified = parse_iso(vapply(blocks, xml_text, character(1), tag = "LastModified", USE.NAMES = FALSE)),
    etag = gsub("\"", "", vapply(blocks, xml_text, character(1), tag = "ETag", USE.NAMES = FALSE), fixed = TRUE)
  )
  prefixes <- vapply(xml_blocks(xml, "CommonPrefixes"), xml_text, character(1),
                     tag = "Prefix", USE.NAMES = FALSE)
  truncated <- xml_text(xml, "IsTruncated") == "true"
  token <- xml_text(xml, "NextContinuationToken")
  list(
    objects = objects, prefixes = prefixes[nzchar(prefixes)],
    next_token = if (truncated && nzchar(token)) token else NULL
  )
}

#' Rows of a listing for common prefixes: key ends with the delimiter,
#' the rest is missing
#' @noRd
prefix_rows <- function(prefixes) {
  tibble::tibble(
    key = prefixes, size = rep(NA_real_, length(prefixes)),
    last_modified = as.POSIXct(rep(NA_real_, length(prefixes)), origin = "1970-01-01", tz = "UTC"),
    etag = rep(NA_character_, length(prefixes))
  )
}

#' An `<Error>` document in a 2xx body (copy, complete) is still an error
#' @noRd
check_body_error <- function(body, method, bucket, key, call) {
  if (grepl("^\\s*(<\\?xml[^>]*>)?\\s*<Error[ >]", body)) {
    cli::cli_abort(
      c("{xml_text(body, 'Code')} for {method} {s3_url(bucket, key)}.",
        "x" = xml_text(body, "Message")),
      class = c("s3pe_error_http", "s3pe_error"),
      bucket = bucket, key = key, code = xml_text(body, "Code"), call = call
    )
  }
  invisible(body)
}

empty_listing <- function() {
  tibble::tibble(
    key = character(), size = numeric(),
    last_modified = as.POSIXct(character(), tz = "UTC"), etag = character()
  )
}

#' Buckets of a ListBuckets document
#' @noRd
parse_buckets <- function(xml) {
  blocks <- xml_blocks(xml, "Bucket")
  tibble::tibble(
    name = vapply(blocks, xml_text, character(1), tag = "Name", USE.NAMES = FALSE),
    created = parse_iso(vapply(blocks, xml_text, character(1), tag = "CreationDate", USE.NAMES = FALSE))
  )
}
