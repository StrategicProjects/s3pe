#' Emit a message unless the package is quiet
#'
#' Quiet when `options(s3pe.quiet = TRUE)`, when the environment variable
#' `S3PE_QUIET` is set to anything but `""` or `"0"`, or, by default, in
#' non-interactive sessions.
#' @noRd
s3_inform <- function(...) {
  if (!s3_is_quiet()) {
    cli::cli_inform(...)
  }
  invisible(NULL)
}

s3_is_quiet <- function() {
  env <- Sys.getenv("S3PE_QUIET", unset = "")
  if (nzchar(env) && env != "0") {
    return(TRUE)
  }
  isTRUE(getOption("s3pe.quiet", default = !rlang::is_interactive()))
}

#' `12.3 MB`, `1.2 kB`, `512 B`: SI units, one decimal
#' @noRd
format_size <- function(bytes) {
  units <- c("B", "kB", "MB", "GB", "TB")
  v <- as.numeric(bytes)
  i <- 1L
  while (v >= 1000 && i < length(units)) {
    v <- v / 1000
    i <- i + 1L
  }
  if (i == 1L) {
    sprintf("%d B", as.integer(bytes))
  } else {
    sprintf("%.1f %s", v, units[i])
  }
}

#' Seconds since `start`, rounded for a log line
#' @noRd
elapsed_since <- function(start) {
  round(as.numeric(difftime(Sys.time(), start, units = "secs")), 3)
}

#' The content type a key's extension suggests
#'
#' The same table as the other editions of s3pe. Unknown extensions get
#' `application/octet-stream`.
#' @noRd
content_type_for <- function(key) {
  name <- tolower(basename(key))
  if (grepl("\\.(csv|tsv|json)\\.gz$", name)) {
    return("application/gzip")
  }
  ext <- if (grepl(".", name, fixed = TRUE)) sub(".*\\.", "", name) else ""
  switch(ext,
    csv = "text/csv",
    tsv = "text/tab-separated-values",
    json = "application/json",
    geojson = "application/geo+json",
    parquet = "application/vnd.apache.parquet",
    gz = "application/gzip",
    zip = "application/zip",
    txt = "text/plain",
    md = "text/markdown",
    html = ,
    htm = "text/html",
    xml = "application/xml",
    svg = "image/svg+xml",
    png = "image/png",
    jpg = ,
    jpeg = "image/jpeg",
    gif = "image/gif",
    webp = "image/webp",
    pdf = "application/pdf",
    xlsx = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    docx = "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    js = "text/javascript",
    css = "text/css",
    yaml = ,
    yml = "application/yaml",
    toml = "application/toml",
    rds = ,
    "application/octet-stream"
  )
}

#' Decode the five XML entities and numeric references
#' @noRd
xml_unescape <- function(x) {
  x <- gsub("&lt;", "<", x, fixed = TRUE)
  x <- gsub("&gt;", ">", x, fixed = TRUE)
  x <- gsub("&quot;", "\"", x, fixed = TRUE)
  x <- gsub("&apos;", "'", x, fixed = TRUE)
  x <- gsub("&#39;", "'", x, fixed = TRUE)
  x <- gsub("&amp;", "&", x, fixed = TRUE)
  x
}

#' Text of the first `<tag>` inside `xml`, or `""`
#' @noRd
xml_text <- function(xml, tag) {
  m <- regmatches(xml, regexpr(sprintf("<%s>[^<]*</%s>", tag, tag), xml))
  if (length(m) == 0) {
    return("")
  }
  xml_unescape(gsub(sprintf("^<%s>|</%s>$", tag, tag), "", m))
}

#' Every `<tag>...</tag>` block of `xml`
#' @noRd
xml_blocks <- function(xml, tag) {
  regmatches(xml, gregexpr(sprintf("<%s>.*?</%s>", tag, tag), xml, perl = TRUE))[[1]]
}

#' Parse an ISO 8601 instant (`2026-10-20T04:31:02.000Z`) as UTC
#' @noRd
parse_iso <- function(x) {
  out <- as.POSIXct(x, format = "%Y-%m-%dT%H:%M:%OS", tz = "UTC")
  out
}

#' Parse an HTTP date (`Thu, 23 Oct 2026 12:00:00 GMT`) as UTC,
#' independent of the session's locale
#' @noRd
parse_http_date <- function(x) {
  if (length(x) == 0 || is.na(x) || !nzchar(x)) {
    return(as.POSIXct(NA_real_, origin = "1970-01-01", tz = "UTC"))
  }
  parts <- strsplit(trimws(x), "[ ,:]+")[[1]]
  if (length(parts) < 7) {
    return(as.POSIXct(NA_real_, origin = "1970-01-01", tz = "UTC"))
  }
  months <- c("jan", "feb", "mar", "apr", "may", "jun",
              "jul", "aug", "sep", "oct", "nov", "dec")
  month <- match(tolower(substr(parts[3], 1, 3)), months)
  iso <- sprintf("%s-%02d-%02d %s:%s:%s", parts[4], month,
                 as.integer(parts[2]), parts[5], parts[6], parts[7])
  as.POSIXct(iso, format = "%Y-%m-%d %H:%M:%S", tz = "UTC")
}
