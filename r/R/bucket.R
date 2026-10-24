#' List the objects under a prefix
#'
#' Every object whose key starts with `prefix`, across all pages of the
#' listing. Reading a bucket's manifest (`metadata.json`, `manifest.json`)
#' is usually faster than listing it, when the publisher writes one last:
#' it says what is there, and it is there only when everything else is.
#'
#' @param prefix Key prefix, `""` or `NULL` for the whole bucket, or an
#'   `s3://bucket/prefix` URL.
#' @param bucket Bucket; defaults to the config's bucket.
#' @param delimiter `NULL` lists every object under the prefix. `"/"` lists
#'   one level: the objects directly under the prefix and the common
#'   prefixes ("folders") below it, which come back as rows whose `key`
#'   ends with the delimiter and whose `size`, `last_modified` and `etag`
#'   are `NA`.
#' @param config An [s3_config()].
#' @returns A tibble with one row per object: `key`, `size` (bytes),
#'   `last_modified` (UTC) and `etag`. Zero rows when nothing matches.
#' @examplesIf interactive()
#' list_s3("data/", bucket = "my-bucket")
#' list_s3("s3://my-bucket/series/")
#' list_s3("series/", bucket = "my-bucket", delimiter = "/")   # one level
#' @export
list_s3 <- function(prefix = NULL, bucket = NULL, delimiter = NULL,
                    config = s3_config()) {
  prefix <- prefix %||% ""
  rlang::check_string(delimiter, allow_null = TRUE)
  where <- resolve_key(prefix, bucket, config, allow_empty = TRUE)
  start <- Sys.time()
  objects <- empty_listing()
  prefixes <- character()
  token <- NULL
  repeat {
    query <- list(`list-type` = "2")
    if (nzchar(where$key)) {
      query$prefix <- where$key
    }
    if (!is.null(delimiter)) {
      query$delimiter <- delimiter
    }
    if (!is.null(token)) {
      query$`continuation-token` <- token
    }
    req <- s3_request(config, "GET", where$bucket, "", query = query)
    resp <- s3_perform(req, config, "GET", where$bucket, where$key)
    page <- parse_listing(httr2::resp_body_string(resp))
    objects <- rbind(objects, page$objects)
    prefixes <- c(prefixes, page$prefixes)
    token <- page$next_token
    if (is.null(token)) {
      break
    }
  }
  s3_inform(c("v" = "{.emph \u2714} {nrow(objects)} object{?s} under {.url {s3_url(where$bucket, where$key)}} ({elapsed_since(start)} s)"))
  if (length(prefixes) > 0) {
    objects <- rbind(prefix_rows(unique(prefixes)), objects)
  }
  objects
}

#' Delete objects
#'
#' One key is a `DELETE`; several keys go in batches of 1000
#' (`DeleteObjects`). Deleting an object that does not exist is not an
#' error; an object the store refuses to delete is.
#'
#' @param key One or more keys (or `s3://bucket/key` URLs).
#' @inheritParams read_s3
#' @returns The number of keys deleted, invisibly.
#' @examplesIf interactive()
#' delete_s3("report/draft.csv", bucket = "my-bucket")
#' delete_s3(list_s3("tmp/", bucket = "my-bucket")$key, bucket = "my-bucket")
#' @export
delete_s3 <- function(key, bucket = NULL, config = s3_config()) {
  if (!is.character(key)) {
    cli::cli_abort("{.arg key} must be a character vector.",
                   class = c("s3pe_error_input", "s3pe_error"))
  }
  if (length(key) == 0) {
    return(invisible(0L))
  }
  if (length(key) == 1) {
    where <- resolve_key(key, bucket, config)
    req <- s3_request(config, "DELETE", where$bucket, where$key)
    s3_perform(req, config, "DELETE", where$bucket, where$key)
    s3_inform(c("x" = "{.emph \u2716} {.url {s3_url(where$bucket, where$key)}}"))
    return(invisible(1L))
  }
  resolved <- lapply(key, resolve_key, bucket = bucket, config = config)
  buckets <- vapply(resolved, `[[`, character(1), "bucket")
  keys <- vapply(resolved, `[[`, character(1), "key")
  failed <- character()
  for (b in unique(buckets)) {
    ks <- keys[buckets == b]
    for (chunk in split(ks, ceiling(seq_along(ks) / 1000))) {
      body <- charToRaw(enc2utf8(paste0(
        "<Delete><Quiet>true</Quiet>",
        paste0("<Object><Key>", xml_escape(chunk), "</Key></Object>", collapse = ""),
        "</Delete>"
      )))
      req <- s3_request(config, "POST", b, "", query = list(delete = ""), body = body,
                        content_type = "application/xml",
                        headers = c("content-md5" = content_md5(body)))
      resp <- s3_perform(req, config, "POST", b, "")
      text <- httr2::resp_body_string(resp)
      for (block in xml_blocks(text, "Error")) {
        failed <- c(failed, sprintf("s3://%s/%s (%s: %s)", b, xml_text(block, "Key"),
                                    xml_text(block, "Code"), xml_text(block, "Message")))
      }
    }
    s3_inform(c("x" = "{.emph \u2716} {length(ks)} object{?s} in {.url {s3_url(b, '')}}"))
  }
  if (length(failed) > 0) {
    cli::cli_abort(
      c("{length(failed)} object{?s} not deleted.", stats::setNames(failed, rep("x", length(failed)))),
      class = c("s3pe_error_http", "s3pe_error"), code = "DeleteErrors"
    )
  }
  invisible(length(key))
}

#' Copy an object inside the store
#'
#' A server-side `CopyObject`: nothing is downloaded. The copy keeps the
#' source's content type and metadata.
#'
#' @param from,to Keys, or `s3://bucket/key` URLs (the two may name
#'   different buckets).
#' @inheritParams read_s3
#' @returns Invisibly, a one-row tibble: `bucket`, `key`, `size`, `etag`,
#'   `elapsed` of the copy.
#' @examplesIf interactive()
#' copy_s3("etl/sales.parquet", "archive/sales-2026.parquet", bucket = "my-bucket")
#' copy_s3("s3://staging/report.pdf", "s3://public/report.pdf")
#' @export
copy_s3 <- function(from, to, bucket = NULL, config = s3_config()) {
  src <- resolve_key(from, bucket, config)
  dst <- resolve_key(to, bucket, config)
  start <- Sys.time()
  source <- paste0("/", uri_encode(src$bucket), "/", uri_encode(src$key, keep_slash = TRUE))
  req <- s3_request(config, "PUT", dst$bucket, dst$key,
                    headers = c("x-amz-copy-source" = source))
  resp <- s3_perform(req, config, "PUT", dst$bucket, dst$key)
  text <- httr2::resp_body_string(resp)
  check_body_error(text, "PUT", dst$bucket, dst$key, call = rlang::current_env())
  etag <- gsub("\"", "", xml_text(text, "ETag"), fixed = TRUE)
  h <- head_s3(paste0("s3://", dst$bucket, "/", dst$key), config = config)
  size <- if (is.null(h)) NA_real_ else h$size
  s3_inform(c("v" = "{.emph \u29c9} {.url {s3_url(src$bucket, src$key)}} \u2192 {.url {s3_url(dst$bucket, dst$key)}} ({format_size(size)}, {elapsed_since(start)} s)"))
  invisible(tibble::tibble(
    bucket = dst$bucket, key = dst$key, size = size, etag = etag,
    elapsed = elapsed_since(start)
  ))
}

#' A pre-signed URL
#'
#' A URL anyone can use for `expires` seconds, without credentials, to
#' read (`GET`) or write (`PUT`) the object. The signature is in the query
#' string (AWS Signature Version 4).
#'
#' @inheritParams read_s3
#' @param expires Validity in seconds, from 1 to 604800 (seven days).
#' @param method `"GET"` to share a download, `"PUT"` to accept an upload.
#' @returns The URL, a string.
#' @examplesIf interactive()
#' presign_s3("report/report.pdf", bucket = "my-bucket", expires = 3600)
#' @export
presign_s3 <- function(key, bucket = NULL, expires = 3600, method = c("GET", "PUT"),
                       config = s3_config()) {
  where <- resolve_key(key, bucket, config)
  check_config(config)
  method <- rlang::arg_match(method)
  rlang::check_number_decimal(expires, min = 1)
  expires <- min(as.numeric(expires), MAX_PRESIGN_SECS)
  path <- paste0("/", uri_encode(where$bucket), "/", uri_encode(where$key, keep_slash = TRUE))
  qs <- sigv4_presign(
    method = method, host = config$host, path = path, amz_date = amz_date(),
    expires = as.integer(expires), access_key = config$access_key,
    secret_key = config$secret_key, session_token = config$session_token,
    region = config$region
  )
  paste0(config$endpoint, path, "?", qs)
}

#' Metadata of an object
#'
#' A `HEAD` request: size, content type, modification time, ETag and user
#' metadata, without downloading the object.
#'
#' @inheritParams read_s3
#' @returns A one-row tibble (`key`, `size`, `content_type`,
#'   `last_modified`, `etag`, and `metadata`, a list column holding the
#'   named list of user metadata), or `NULL` when the object does not exist.
#' @examplesIf interactive()
#' head_s3("data/manifest.json", bucket = "my-bucket")
#' @export
head_s3 <- function(key, bucket = NULL, config = s3_config()) {
  where <- resolve_key(key, bucket, config)
  req <- s3_request(config, "HEAD", where$bucket, where$key)
  resp <- tryCatch(
    s3_perform(req, config, "HEAD", where$bucket, where$key),
    s3pe_error_not_found = function(cnd) NULL
  )
  if (is.null(resp)) {
    return(NULL)
  }
  all <- httr2::resp_headers(resp)
  names(all) <- tolower(names(all))
  meta <- all[startsWith(names(all), "x-amz-meta-")]
  names(meta) <- sub("^x-amz-meta-", "", names(meta))
  tibble::tibble(
    key = where$key,
    size = resp_content_length(resp),
    content_type = httr2::resp_header(resp, "content-type", default = NA_character_),
    last_modified = parse_http_date(httr2::resp_header(resp, "last-modified", default = NA_character_)),
    etag = resp_etag(resp),
    metadata = list(as.list(unclass(meta)))
  )
}

#' Does an object exist?
#'
#' @inheritParams read_s3
#' @returns `TRUE` or `FALSE`. Anything but a plain "not found" (no
#'   credentials, access denied, server down) is an error.
#' @examplesIf interactive()
#' exists_s3("data/manifest.json", bucket = "my-bucket")
#' @export
exists_s3 <- function(key, bucket = NULL, config = s3_config()) {
  !is.null(head_s3(key, bucket = bucket, config = config))
}

#' List the buckets the credentials can see
#'
#' @inheritParams list_s3
#' @returns A tibble with `name` and `created` (UTC).
#' @examplesIf interactive()
#' buckets_s3()
#' @export
buckets_s3 <- function(config = s3_config()) {
  req <- s3_request(config, "GET", "", "")
  resp <- s3_perform(req, config, "GET", "", "")
  parse_buckets(httr2::resp_body_string(resp))
}
