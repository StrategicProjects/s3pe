FORMATS <- c("csv", "tsv", "json", "geojson", "parquet", "rds", "xlsx", "text", "raw")
TEXT_EXTENSIONS <- c("txt", "md", "html", "htm", "xml", "svg", "yaml", "yml",
                     "toml", "css", "js", "r", "sql", "qmd", "rmd")

#' The format a key's extension suggests, or `NA`
#' @noRd
detect_format <- function(key) {
  name <- tolower(basename(key))
  if (grepl("\\.csv(\\.gz)?$", name)) return("csv")
  if (grepl("\\.tsv(\\.gz)?$", name)) return("tsv")
  ext <- if (grepl(".", name, fixed = TRUE)) sub(".*\\.", "", name) else ""
  if (ext %in% c("json", "geojson", "parquet", "rds", "xlsx")) return(ext)
  if (ext %in% TEXT_EXTENSIONS) return("text")
  NA_character_
}

check_format <- function(format, call = rlang::caller_env()) {
  if (is.null(format)) {
    return(NULL)
  }
  rlang::arg_match(format, FORMATS, error_call = call)
}

#' Read an object into R
#'
#' Downloads an object and parses it according to its extension (or
#' `format`):
#'
#' | Extension | Reader | Result |
#' |---|---|---|
#' | `.csv`, `.csv.gz` | [readr::read_csv()] | tibble |
#' | `.tsv`, `.tsv.gz` | [readr::read_tsv()] | tibble |
#' | `.json` | [jsonlite::fromJSON()] | list or data frame |
#' | `.geojson` | `sf::st_read()` if sf is installed, else jsonlite | sf or list |
#' | `.parquet` | `nanoparquet::read_parquet()` | data frame |
#' | `.rds` | [readRDS()] | the saved object |
#' | `.xlsx` | `readxl::read_excel()` | tibble |
#' | `.txt`, `.md`, `.html`, `.xml`, `.svg`, ... | | character (one string) |
#' | anything else | | raw vector |
#'
#' The byte count is checked against the server's `Content-Length` and,
#' when the ETag is an MD5, against the MD5 of what arrived.
#'
#' @param key Object key inside `bucket`, or an `s3://bucket/key` URL.
#' @param bucket Bucket; defaults to the config's bucket (`S3PE_BUCKET`).
#' @param ... Passed to the reader (for example `col_types` for CSV,
#'   `simplifyVector` for JSON).
#' @param format One of `"csv"`, `"tsv"`, `"json"`, `"geojson"`,
#'   `"parquet"`, `"rds"`, `"xlsx"`, `"text"`, `"raw"`; overrides the
#'   extension.
#' @param config An [s3_config()].
#' @returns The parsed object, as in the table.
#' @examplesIf interactive()
#' meta <- read_s3("dados/metadata.json", bucket = "iaempe")
#' meta$ultimo_mes
#'
#' icms <- read_s3("s3://horizonte-fiscal/series/estado/icms.csv.gz")
#'
#' # the bytes, whatever the extension
#' png <- read_s3("nota_tecnica/figura1.png", bucket = "iaempe", format = "raw")
#' @export
read_s3 <- function(key, bucket = NULL, ..., format = NULL, config = s3_config()) {
  where <- resolve_key(key, bucket, config)
  format <- check_format(format) %||% detect_format(where$key) %||% "raw"
  if (is.na(format)) {
    format <- "raw"
  }
  start <- Sys.time()
  req <- s3_request(config, "GET", where$bucket, where$key)
  resp <- s3_perform(req, config, "GET", where$bucket, where$key)
  body <- httr2::resp_body_raw(resp)
  check_download(
    where$bucket, where$key,
    size = length(body), expected = resp_content_length(resp),
    etag = resp_etag(resp), md5 = hash_hex(openssl::md5(body))
  )
  s3_inform(c("v" = "{.emph \u2193} {.url {s3_url(where$bucket, where$key)}} ({format_size(length(body))}, {elapsed_since(start)} s)"))
  parse_body(body, format, where$key, ...)
}

parse_body <- function(body, format, key, ...) {
  switch(format,
    raw = body,
    text = raw_to_text(body),
    json = jsonlite::fromJSON(raw_to_text(body), ...),
    csv = with_tempfile(body, key, function(f) readr::read_csv(f, show_col_types = FALSE, ...)),
    tsv = with_tempfile(body, key, function(f) readr::read_tsv(f, show_col_types = FALSE, ...)),
    parquet = {
      rlang::check_installed("nanoparquet", "to read Parquet files.")
      with_tempfile(body, key, function(f) nanoparquet::read_parquet(f, ...))
    },
    rds = with_tempfile(body, key, function(f) readRDS(f, ...)),
    xlsx = {
      rlang::check_installed("readxl", "to read Excel files.")
      with_tempfile(body, key, function(f) readxl::read_excel(f, ...))
    },
    geojson = if (rlang::is_installed("sf")) {
      with_tempfile(body, key, function(f) sf::st_read(f, quiet = TRUE, ...))
    } else {
      jsonlite::fromJSON(raw_to_text(body), ...)
    }
  )
}

raw_to_text <- function(body) {
  x <- rawToChar(body)
  Encoding(x) <- "UTF-8"
  x
}

#' Write `body` to a temporary file with the key's extension, call `fun`
#' on it, remove it
#' @noRd
with_tempfile <- function(body, key, fun) {
  f <- tempfile(fileext = file_extension(key))
  on.exit(unlink(f), add = TRUE)
  writeBin(body, f)
  fun(f)
}

#' `.csv.gz` or `.json` or `""`: the extension with its dot, double for gz
#' @noRd
file_extension <- function(key) {
  name <- basename(key)
  m <- regmatches(name, regexpr("\\.[A-Za-z0-9]+(\\.gz)?$", name))
  if (length(m) == 0) "" else tolower(m)
}

#' Write an R object to the store
#'
#' Serialises `x` according to the key's extension (or `format`) and
#' uploads it:
#'
#' | Extension | Writer | Accepts |
#' |---|---|---|
#' | `.csv`, `.csv.gz` | [readr::write_csv()] | data frame |
#' | `.tsv`, `.tsv.gz` | [readr::write_tsv()] | data frame |
#' | `.json` | [jsonlite::toJSON()] (`auto_unbox = TRUE`, `digits = NA`) | anything jsonlite can encode |
#' | `.geojson` | `sf::st_write()` for sf objects, else jsonlite | sf or list |
#' | `.parquet` | `nanoparquet::write_parquet()` | data frame |
#' | `.rds` | [saveRDS()] | anything |
#' | `.txt`, `.md`, `.html`, ... | | character (joined by newlines) |
#' | anything else | | raw vector, or character as text |
#'
#' The content type is set from the extension unless `content_type` is
#' given. After the upload the server's ETag is compared with the MD5 of
#' what was sent.
#'
#' @param x The object to write.
#' @inheritParams read_s3
#' @param ... Passed to the writer (for example `pretty = TRUE` for JSON,
#'   `na = ""` for CSV).
#' @param content_type Content type to store with the object.
#' @returns Invisibly, a one-row tibble: `bucket`, `key`, `size` (bytes),
#'   `etag`, `elapsed` (seconds).
#' @examplesIf interactive()
#' write_s3(mtcars, "nota_tecnica/mtcars.csv", bucket = "iaempe")
#' write_s3(list(gerado_em = Sys.time()), "nota_tecnica/manifest.json", bucket = "iaempe")
#' write_s3(readBin("figura1.png", "raw", 1e7), "s3://iaempe/nota_tecnica/figura1.png")
#' @export
write_s3 <- function(x, key, bucket = NULL, ..., format = NULL,
                     content_type = NULL, config = s3_config()) {
  where <- resolve_key(key, bucket, config)
  rlang::check_string(content_type, allow_null = TRUE)
  format <- check_format(format) %||% detect_format(where$key)
  if (is.na(format)) {
    format <- if (is.raw(x)) "raw" else if (is.character(x)) "text" else NA_character_
  }
  if (is.na(format)) {
    cli::cli_abort(
      c("Cannot tell how to write {.cls {class(x)[1]}} to {.val {where$key}}.",
        "i" = "Give the key an extension ({.code .csv}, {.code .parquet}, {.code .rds}, ...) or set {.arg format}."),
      class = c("s3pe_error_input", "s3pe_error")
    )
  }
  body <- serialise(x, format, where$key, ...)
  content_type <- content_type %||% content_type_for(where$key)
  start <- Sys.time()
  req <- s3_request(config, "PUT", where$bucket, where$key, body = body,
                    content_type = content_type)
  hashes <- attr(req, "hashes")
  resp <- s3_perform(req, config, "PUT", where$bucket, where$key)
  etag <- resp_etag(resp)
  check_upload(where$bucket, where$key, hashes$md5, etag)
  if (inherits(body, "s3pe_file")) {
    unlink(unclass(body))
  }
  s3_inform(c("v" = "{.emph \u2191} {.url {s3_url(where$bucket, where$key)}} ({format_size(hashes$size)}, {elapsed_since(start)} s)"))
  invisible(tibble::tibble(
    bucket = where$bucket, key = where$key, size = hashes$size,
    etag = etag, elapsed = elapsed_since(start)
  ))
}

#' A raw vector, or an `s3pe_file` pointing to a temporary file
#' @noRd
serialise <- function(x, format, key, ...) {
  to_file <- function(ext, fun) {
    f <- tempfile(fileext = ext)
    fun(f)
    s3_file(f)
  }
  gz <- if (endsWith(tolower(key), ".gz")) ".gz" else ""
  switch(format,
    raw = {
      if (!is.raw(x)) cli::cli_abort("{.arg x} must be a raw vector for {.code format = \"raw\"}.", class = c("s3pe_error_input", "s3pe_error"))
      x
    },
    text = {
      if (!is.character(x)) cli::cli_abort("{.arg x} must be character for {.code format = \"text\"}.", class = c("s3pe_error_input", "s3pe_error"))
      charToRaw(enc2utf8(paste(x, collapse = "\n")))
    },
    csv = to_file(paste0(".csv", gz), function(f) readr::write_csv(x, f, ...)),
    tsv = to_file(paste0(".tsv", gz), function(f) readr::write_tsv(x, f, ...)),
    parquet = {
      rlang::check_installed("nanoparquet", "to write Parquet files.")
      to_file(".parquet", function(f) nanoparquet::write_parquet(x, f, ...))
    },
    rds = to_file(".rds", function(f) saveRDS(x, f, ...)),
    xlsx = cli::cli_abort(
      c("Writing {.code .xlsx} is not supported.",
        "i" = "Write the file with another package and use {.fn upload_s3}."),
      class = c("s3pe_error_input", "s3pe_error")
    ),
    geojson = if (inherits(x, "sf")) {
      rlang::check_installed("sf", "to write GeoJSON.")
      to_file(".geojson", function(f) sf::st_write(x, f, driver = "GeoJSON", quiet = TRUE, ...))
    } else {
      to_json(x, ...)
    },
    json = to_json(x, ...)
  )
}

to_json <- function(x, ...) {
  args <- list(...)
  if (is.null(args$auto_unbox)) args$auto_unbox <- TRUE
  if (is.null(args$digits)) args$digits <- NA
  json <- rlang::exec(jsonlite::toJSON, x, !!!args)
  charToRaw(enc2utf8(as.character(json)))
}
