# Uploading a body: one PUT, or a multipart upload above the threshold (D15).

#' Put a body (raw vector or `s3pe_file`) under a key
#'
#' Below `config$multipart_threshold` a single `PUT`; above it a multipart
#' upload in parts of `config$part_size`, sent one by one, aborted on any
#' failure. Each part's ETag is compared with its MD5 and the final ETag
#' with `md5(concat(part MD5s))-N`.
#'
#' @param metadata Named character vector of user metadata.
#' @param headers Named character vector of extra headers.
#' @returns `list(size =, etag =)`.
#' @noRd
put_body <- function(config, bucket, key, body, content_type, metadata = NULL,
                     headers = NULL, call = rlang::caller_env()) {
  all_headers <- put_headers(content_type, metadata, headers, call = call)
  size <- if (is.raw(body)) length(body) else file.size(unclass(body))
  if (size > config$multipart_threshold) {
    return(multipart_upload(config, bucket, key, body, size, content_type, all_headers, call = call))
  }
  req <- s3_request(config, "PUT", bucket, key, body = body,
                    content_type = content_type, headers = all_headers, call = call)
  hashes <- attr(req, "hashes")
  resp <- s3_perform(req, config, "PUT", bucket, key, call = call)
  etag <- resp_etag(resp)
  check_upload(bucket, key, hashes$md5, etag, call = call)
  list(size = hashes$size, etag = etag)
}

#' `x-amz-meta-*` for the metadata, plus the extra headers, lower-case
#' @noRd
put_headers <- function(content_type, metadata, headers, call = rlang::caller_env()) {
  out <- character()
  if (!is.null(metadata)) {
    if (!is.character(metadata) || is.null(names(metadata)) || any(!nzchar(names(metadata)))) {
      cli::cli_abort("{.arg metadata} must be a named character vector.",
                     class = c("s3pe_error_input", "s3pe_error"), call = call)
    }
    out <- c(out, stats::setNames(unname(metadata), paste0("x-amz-meta-", tolower(names(metadata)))))
  }
  if (!is.null(headers)) {
    if (!is.character(headers) || is.null(names(headers)) || any(!nzchar(names(headers)))) {
      cli::cli_abort("{.arg headers} must be a named character vector.",
                     class = c("s3pe_error_input", "s3pe_error"), call = call)
    }
    out <- c(out, stats::setNames(unname(headers), tolower(names(headers))))
  }
  out
}

multipart_upload <- function(config, bucket, key, body, size, content_type, headers, call) {
  init <- s3_request(config, "POST", bucket, key, query = list(uploads = ""),
                     content_type = content_type %||% content_type_for(key),
                     headers = headers, call = call)
  resp <- s3_perform(init, config, "POST", bucket, key, call = call)
  upload_id <- xml_text(httr2::resp_body_string(resp), "UploadId")
  if (!nzchar(upload_id)) {
    cli::cli_abort("No UploadId in the answer for {s3_url(bucket, key)}.",
                   class = c("s3pe_error_http", "s3pe_error"), call = call)
  }
  n_parts <- ceiling(size / config$part_size)
  md5s <- character(n_parts)
  con <- if (is.raw(body)) NULL else file(unclass(body), "rb")
  abort <- function() {
    tryCatch(
      s3_perform(
        s3_request(config, "DELETE", bucket, key, query = list(uploadId = upload_id), call = call),
        config, "DELETE", bucket, key, call = call
      ),
      error = function(e) NULL
    )
    cli::cli_warn("Multipart upload of {s3_url(bucket, key)} aborted.", class = "s3pe_warning_abort")
  }
  withCallingHandlers(
    {
      for (i in seq_len(n_parts)) {
        offset <- (i - 1) * config$part_size
        this <- min(config$part_size, size - offset)
        part <- if (is.raw(body)) body[(offset + 1):(offset + this)] else readBin(con, "raw", this)
        req <- s3_request(config, "PUT", bucket, key,
                          query = list(partNumber = as.character(i), uploadId = upload_id),
                          body = part, call = call)
        md5 <- attr(req, "hashes")$md5
        resp <- s3_perform(req, config, "PUT", bucket, key, call = call)
        etag <- resp_etag(resp)
        if (looks_like_md5(etag) && tolower(etag) != tolower(md5)) {
          cli::cli_abort(
            c("Part {i} of {s3_url(bucket, key)} failed the integrity check.",
              "x" = "The server's ETag {etag} differs from the MD5 of what was sent ({md5})."),
            class = c("s3pe_error_integrity", "s3pe_error"), call = call
          )
        }
        md5s[i] <- md5
      }
      if (!is.null(con)) {
        close(con)
        con <- NULL
      }
      complete <- paste0(
        "<CompleteMultipartUpload>",
        paste0(sprintf("<Part><PartNumber>%d</PartNumber><ETag>\"%s\"</ETag></Part>", seq_len(n_parts), md5s), collapse = ""),
        "</CompleteMultipartUpload>"
      )
      req <- s3_request(config, "POST", bucket, key, query = list(uploadId = upload_id),
                        body = charToRaw(complete), content_type = "application/xml", call = call)
      resp <- s3_perform(req, config, "POST", bucket, key, call = call)
      text <- httr2::resp_body_string(resp)
      check_body_error(text, "POST", bucket, key, call = call)
    },
    error = function(cnd) {
      if (!is.null(con)) try(close(con), silent = TRUE)
      abort()
    }
  )
  etag <- gsub("\"", "", xml_text(text, "ETag"), fixed = TRUE)
  expected <- multipart_etag(md5s)
  if (nzchar(etag) && tolower(etag) != tolower(expected)) {
    cli::cli_abort(
      c("Multipart upload of {s3_url(bucket, key)} failed the integrity check.",
        "x" = "The server's ETag {etag} differs from the expected {expected}."),
      class = c("s3pe_error_integrity", "s3pe_error"), call = call
    )
  }
  list(size = size, etag = etag)
}

#' The ETag the store would give a local file uploaded by this client:
#' its MD5, or the multipart ETag above the threshold
#' @noRd
expected_etag <- function(path, threshold, part_size) {
  size <- file.size(path)
  if (size <= threshold) {
    return(hash_hex(openssl::md5(file(path, raw = TRUE))))
  }
  con <- file(path, "rb")
  on.exit(close(con), add = TRUE)
  n_parts <- ceiling(size / part_size)
  md5s <- vapply(seq_len(n_parts), function(i) {
    this <- min(part_size, size - (i - 1) * part_size)
    hash_hex(openssl::md5(readBin(con, "raw", this)))
  }, character(1))
  multipart_etag(md5s)
}
