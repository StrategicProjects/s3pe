#' Mirror a directory to a prefix, or a prefix to a directory
#'
#' `sync_up_s3()` uploads every file under `dir` that is missing from the
#' prefix or differs from it; `sync_down_s3()` downloads every object under
#' the prefix that is missing locally or differs. "Differs" is decided by
#' size and ETag: the local file's MD5, or its multipart ETag when it is
#' larger than the config's `multipart_threshold` (computed with the same
#' `part_size`, so files this package uploaded compare equal).
#'
#' With `delete = TRUE`, what exists on the destination but not on the
#' source is removed (remote objects in one batch; local files with
#' [unlink()]). `last` names the file to upload after every other, so a
#' manifest is there only when the batch is complete.
#'
#' @param dir Local directory (created by `sync_down_s3()` if needed).
#' @param prefix Key prefix, or an `s3://bucket/prefix` URL. A trailing `/`
#'   is added if missing.
#' @param bucket Bucket; defaults to the config's bucket.
#' @param delete Remove what the source does not have?
#' @param last Relative path of the file to upload last (`sync_up_s3()` only).
#' @param config An [s3_config()].
#' @returns Invisibly, a list of class `s3pe_sync` with `transferred`,
#'   `skipped` and `deleted` (relative paths) and `elapsed` (seconds).
#' @examplesIf interactive()
#' sync_up_s3("output/", "reports/2026/", bucket = "my-bucket", last = "manifest.json")
#' sync_down_s3("s3://my-bucket/reports/2026/", "reports-2026/")
#' @export
sync_up_s3 <- function(dir, prefix, bucket = NULL, delete = FALSE, last = NULL,
                       config = s3_config()) {
  rlang::check_string(dir)
  rlang::check_bool(delete)
  rlang::check_string(last, allow_null = TRUE)
  if (!dir.exists(dir)) {
    cli::cli_abort("{.file {dir}} is not a directory.", class = c("s3pe_error_io", "s3pe_error"))
  }
  where <- resolve_key(prefix, bucket, config, allow_empty = TRUE)
  pre <- normalize_prefix(where$key)
  start <- Sys.time()
  local <- sort(local_files(dir), method = "radix")
  if (!is.null(last) && !last %in% local) {
    cli::cli_abort("{.arg last} = {.val {last}}: no such file under {.file {dir}}.",
                   class = c("s3pe_error_input", "s3pe_error"))
  }
  remote <- list_s3(paste0("s3://", where$bucket, "/", pre), config = config)
  remote <- remote[!is.na(remote$size), ]
  rel <- substring(remote$key, nchar(pre) + 1)
  remote_size <- stats::setNames(remote$size, rel)
  remote_etag <- stats::setNames(remote$etag, rel)

  queue <- c(setdiff(local, last), last)
  transferred <- character()
  skipped <- character()
  for (r in queue) {
    path <- file.path(dir, r)
    same <- r %in% rel &&
      file.size(path) == remote_size[[r]] &&
      tolower(expected_etag(path, config$multipart_threshold, config$part_size)) == tolower(remote_etag[[r]])
    if (same) {
      skipped <- c(skipped, r)
    } else {
      upload_s3(path, paste0("s3://", where$bucket, "/", pre, r), config = config)
      transferred <- c(transferred, r)
    }
  }
  deleted <- character()
  if (delete) {
    deleted <- setdiff(rel, local)
    if (length(deleted) > 0) {
      delete_s3(paste0("s3://", where$bucket, "/", pre, deleted), config = config)
    }
  }
  report <- new_sync_report(transferred, skipped, deleted, start)
  s3_inform(c("v" = "{.emph \u2714} sync {.file {dir}} \u2192 {.url {s3_url(where$bucket, pre)}}: {length(transferred)} uploaded, {length(skipped)} unchanged, {length(deleted)} deleted ({report$elapsed} s)"))
  invisible(report)
}

#' @rdname sync_up_s3
#' @export
sync_down_s3 <- function(prefix, dir, bucket = NULL, delete = FALSE,
                         config = s3_config()) {
  rlang::check_string(dir)
  rlang::check_bool(delete)
  where <- resolve_key(prefix, bucket, config, allow_empty = TRUE)
  pre <- normalize_prefix(where$key)
  start <- Sys.time()
  if (!dir.exists(dir)) {
    dir.create(dir, recursive = TRUE)
  }
  remote <- list_s3(paste0("s3://", where$bucket, "/", pre), config = config)
  remote <- remote[!is.na(remote$size), ]
  rel <- substring(remote$key, nchar(pre) + 1)
  keep <- nzchar(rel) & !endsWith(rel, "/")
  remote <- remote[keep, ]
  rel <- rel[keep]

  transferred <- character()
  skipped <- character()
  for (i in seq_along(rel)) {
    path <- file.path(dir, rel[i])
    same <- file.exists(path) && !dir.exists(path) &&
      file.size(path) == remote$size[i] &&
      tolower(expected_etag(path, config$multipart_threshold, config$part_size)) == tolower(remote$etag[i])
    if (same) {
      skipped <- c(skipped, rel[i])
    } else {
      download_s3(paste0("s3://", where$bucket, "/", remote$key[i]), path,
                  overwrite = TRUE, config = config)
      transferred <- c(transferred, rel[i])
    }
  }
  deleted <- character()
  if (delete) {
    deleted <- setdiff(local_files(dir), rel)
    if (length(deleted) > 0) {
      unlink(file.path(dir, deleted))
    }
  }
  report <- new_sync_report(transferred, skipped, deleted, start)
  s3_inform(c("v" = "{.emph \u2714} sync {.url {s3_url(where$bucket, pre)}} \u2192 {.file {dir}}: {length(transferred)} downloaded, {length(skipped)} unchanged, {length(deleted)} deleted ({report$elapsed} s)"))
  invisible(report)
}

new_sync_report <- function(transferred, skipped, deleted, start) {
  structure(
    list(
      transferred = transferred, skipped = skipped, deleted = deleted,
      elapsed = elapsed_since(start)
    ),
    class = "s3pe_sync"
  )
}

#' @export
print.s3pe_sync <- function(x, ...) {
  cat(sprintf(
    "<s3pe_sync> %d transferred, %d unchanged, %d deleted (%s s)\n",
    length(x$transferred), length(x$skipped), length(x$deleted), format(x$elapsed)
  ))
  for (f in x$transferred) cat("  \u2191 ", f, "\n", sep = "")
  for (f in x$deleted) cat("  \u2716 ", f, "\n", sep = "")
  invisible(x)
}

#' Relative paths (with `/`) of every file under `dir`
#' @noRd
local_files <- function(dir) {
  files <- list.files(dir, recursive = TRUE, all.files = TRUE, no.. = TRUE)
  files <- files[!endsWith(files, ".s3pe-part") & basename(files) != ".DS_Store"]
  files[!dir.exists(file.path(dir, files))]
}

#' A prefix that is empty or ends with `/`
#' @noRd
normalize_prefix <- function(p) {
  if (!nzchar(p) || endsWith(p, "/")) p else paste0(p, "/")
}
