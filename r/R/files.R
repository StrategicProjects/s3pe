#' Download an object to a file
#'
#' Streams the object to disk. It is written next to its final name with
#' the suffix `.s3pe-part` and renamed when complete and verified, so an
#' interrupted download leaves nothing at `path`.
#'
#' @inheritParams read_s3
#' @param path Where to write. Its directory is created if needed.
#' @param overwrite Replace `path` if it exists?
#' @returns Invisibly, a one-row tibble: `bucket`, `key`, `size`, `etag`,
#'   `elapsed`.
#' @examplesIf interactive()
#' download_s3("data/regions.geojson", "regions.geojson", bucket = "my-bucket")
#' @export
download_s3 <- function(key, path, bucket = NULL, overwrite = FALSE,
                        config = s3_config()) {
  where <- resolve_key(key, bucket, config)
  rlang::check_string(path)
  rlang::check_bool(overwrite)
  if (file.exists(path) && !overwrite) {
    cli::cli_abort(
      c("{.file {path}} already exists.", "i" = "Use {.code overwrite = TRUE} to replace it."),
      class = c("s3pe_error_exists", "s3pe_error")
    )
  }
  dir <- dirname(path)
  if (!dir.exists(dir)) {
    dir.create(dir, recursive = TRUE)
  }
  part <- paste0(path, ".s3pe-part")
  on.exit(unlink(part), add = TRUE)

  start <- Sys.time()
  req <- s3_request(config, "GET", where$bucket, where$key)
  resp <- s3_perform(req, config, "GET", where$bucket, where$key, path = part)
  if (!inherits(resp$body, "httr2_path")) {
    # a mocked response: the body is in memory
    writeBin(httr2::resp_body_raw(resp), part)
  }
  size <- file.size(part)
  check_download(
    where$bucket, where$key,
    size = size, expected = resp_content_length(resp),
    etag = resp_etag(resp), md5 = hash_hex(openssl::md5(file(part, raw = TRUE)))
  )
  if (!file.rename(part, path)) {
    cli::cli_abort("Could not move the downloaded file to {.file {path}}.",
                   class = c("s3pe_error_io", "s3pe_error"))
  }
  s3_inform(c("v" = "{.emph \u2193} {.url {s3_url(where$bucket, where$key)}} \u2192 {.file {path}} ({format_size(size)}, {elapsed_since(start)} s)"))
  invisible(tibble::tibble(
    bucket = where$bucket, key = where$key, size = size,
    etag = resp_etag(resp), elapsed = elapsed_since(start)
  ))
}

#' Upload a file
#'
#' Streams a file to the store (two passes over the file: one to hash it,
#' one to send it). The server's ETag is compared with the file's MD5.
#'
#' @param path File to upload.
#' @param key Object key, or an `s3://bucket/key` URL. `NULL`, `""` or a key
#'   ending in `/` gets the file's name appended.
#' @inheritParams read_s3
#' @inheritParams write_s3
#' @returns Invisibly, a one-row tibble: `bucket`, `key`, `size`, `etag`,
#'   `elapsed`.
#' @examplesIf interactive()
#' upload_s3("output/panel.parquet", "etl/", bucket = "my-bucket")
#' upload_s3("figure1.png", "s3://my-bucket/report/figure1.png")
#' @export
upload_s3 <- function(path, key = NULL, bucket = NULL, content_type = NULL,
                      config = s3_config()) {
  rlang::check_string(path)
  rlang::check_string(content_type, allow_null = TRUE)
  if (!file.exists(path) || dir.exists(path)) {
    cli::cli_abort("{.file {path}} is not a file.", class = c("s3pe_error_io", "s3pe_error"))
  }
  key <- key %||% ""
  where <- resolve_key(key, bucket, config, allow_empty = TRUE)
  if (!nzchar(where$key) || endsWith(where$key, "/")) {
    where$key <- paste0(where$key, basename(path))
  }
  content_type <- content_type %||% content_type_for(where$key)
  start <- Sys.time()
  req <- s3_request(config, "PUT", where$bucket, where$key, body = s3_file(path),
                    content_type = content_type)
  hashes <- attr(req, "hashes")
  resp <- s3_perform(req, config, "PUT", where$bucket, where$key)
  etag <- resp_etag(resp)
  check_upload(where$bucket, where$key, hashes$md5, etag)
  s3_inform(c("v" = "{.emph \u2191} {.file {path}} \u2192 {.url {s3_url(where$bucket, where$key)}} ({format_size(hashes$size)}, {elapsed_since(start)} s)"))
  invisible(tibble::tibble(
    bucket = where$bucket, key = where$key, size = hashes$size,
    etag = etag, elapsed = elapsed_since(start)
  ))
}
