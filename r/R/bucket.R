#' List the objects under a prefix
#'
#' Every object whose key starts with `prefix`, across all pages of the
#' listing. Reading a bucket's manifest (`metadata.json`, `manifest.json`)
#' is usually faster than listing it: the manifests of the Pernambuco
#' buckets say what is there and are uploaded last.
#'
#' @param prefix Key prefix, `""` or `NULL` for the whole bucket, or an
#'   `s3://bucket/prefix` URL.
#' @param bucket Bucket; defaults to the config's bucket.
#' @param config An [s3_config()].
#' @returns A tibble with one row per object: `key`, `size` (bytes),
#'   `last_modified` (UTC) and `etag`. Zero rows when nothing matches.
#' @examplesIf interactive()
#' list_s3("dados/", bucket = "iaempe")
#' list_s3("s3://horizonte-fiscal/series/estado/")
#' @export
list_s3 <- function(prefix = NULL, bucket = NULL, config = s3_config()) {
  prefix <- prefix %||% ""
  where <- resolve_key(prefix, bucket, config, allow_empty = TRUE)
  start <- Sys.time()
  objects <- empty_listing()
  token <- NULL
  repeat {
    query <- list(`list-type` = "2")
    if (nzchar(where$key)) {
      query$prefix <- where$key
    }
    if (!is.null(token)) {
      query$`continuation-token` <- token
    }
    req <- s3_request(config, "GET", where$bucket, "", query = query)
    resp <- s3_perform(req, config, "GET", where$bucket, where$key)
    page <- parse_listing(httr2::resp_body_string(resp))
    objects <- rbind(objects, page$objects)
    token <- page$next_token
    if (is.null(token)) {
      break
    }
  }
  s3_inform(c("v" = "{.emph \u2714} {nrow(objects)} object{?s} under {.url {s3_url(where$bucket, where$key)}} ({elapsed_since(start)} s)"))
  objects
}

#' Delete an object
#'
#' Deleting an object that does not exist is not an error.
#'
#' @inheritParams read_s3
#' @returns `TRUE`, invisibly.
#' @examplesIf interactive()
#' delete_s3("nota_tecnica/rascunho.csv", bucket = "iaempe")
#' @export
delete_s3 <- function(key, bucket = NULL, config = s3_config()) {
  where <- resolve_key(key, bucket, config)
  req <- s3_request(config, "DELETE", where$bucket, where$key)
  s3_perform(req, config, "DELETE", where$bucket, where$key)
  s3_inform(c("x" = "{.emph \u2716} {.url {s3_url(where$bucket, where$key)}}"))
  invisible(TRUE)
}

#' Metadata of an object
#'
#' A `HEAD` request: size, content type, modification time and ETag,
#' without downloading the object.
#'
#' @inheritParams read_s3
#' @returns A one-row tibble (`key`, `size`, `content_type`,
#'   `last_modified`, `etag`), or `NULL` when the object does not exist.
#' @examplesIf interactive()
#' head_s3("dados/metadata.json", bucket = "iaempe")
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
  tibble::tibble(
    key = where$key,
    size = resp_content_length(resp),
    content_type = httr2::resp_header(resp, "content-type", default = NA_character_),
    last_modified = parse_http_date(httr2::resp_header(resp, "last-modified", default = NA_character_)),
    etag = resp_etag(resp)
  )
}

#' Does an object exist?
#'
#' @inheritParams read_s3
#' @returns `TRUE` or `FALSE`. Anything but a plain "not found" (no
#'   credentials, access denied, server down) is an error.
#' @examplesIf interactive()
#' exists_s3("dados/metadata.json", bucket = "iaempe")
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
