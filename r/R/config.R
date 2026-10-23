#' Configure the connection to the store
#'
#' Collects endpoint, region, credentials and default bucket. Anything not
#' given as an argument comes from an option, then from the environment,
#' then from the default:
#'
#' | Setting | Option | Environment | Default |
#' |---|---|---|---|
#' | `endpoint` | `s3pe.endpoint` | `S3PE_ENDPOINT`, `AWS_ENDPOINT_URL` | `https://s3.pe.gov.br` |
#' | `region` | `s3pe.region` | `AWS_REGION`, `AWS_DEFAULT_REGION` | `us-east-1` |
#' | `access_key` | | `AWS_ACCESS_KEY_ID` | required |
#' | `secret_key` | | `AWS_SECRET_ACCESS_KEY` | required |
#' | `session_token` | | `AWS_SESSION_TOKEN` | none |
#' | `bucket` | `s3pe.bucket` | `S3PE_BUCKET` | none |
#' | `timeout` | `s3pe.timeout` | | 300 s |
#' | `max_tries` | `s3pe.max_tries` | | 3 |
#'
#' Keep credentials in `~/.Renviron` (R reads it at start-up), never in
#' scripts. URLs are always path-style (`endpoint/bucket/key`), which is
#' what the Garage server of Pernambuco expects.
#'
#' @param endpoint `scheme://host[:port]`, no path.
#' @param region Signing region. The Garage of Pernambuco uses `us-east-1`.
#' @param access_key,secret_key Credentials. Missing credentials are an
#'   error when a request is made, not here.
#' @param session_token Session token of temporary credentials.
#' @param bucket Bucket used when a key is not an `s3://bucket/key` URL.
#' @param timeout Time limit of one call, in seconds.
#' @param max_tries Attempts per call (the first one included).
#' @returns An object of class `s3pe_config`. Printing it never shows the
#'   secret key.
#' @examplesIf interactive()
#' s3_config()
#' cfg <- s3_config(bucket = "iaempe")
#' list_s3("dados/", config = cfg)
#' @export
s3_config <- function(endpoint = NULL, region = NULL, access_key = NULL,
                      secret_key = NULL, session_token = NULL, bucket = NULL,
                      timeout = NULL, max_tries = NULL) {
  endpoint <- endpoint %||% getOption("s3pe.endpoint") %||%
    env_or_null("S3PE_ENDPOINT") %||% env_or_null("AWS_ENDPOINT_URL") %||%
    "https://s3.pe.gov.br"
  region <- region %||% getOption("s3pe.region") %||%
    env_or_null("AWS_REGION") %||% env_or_null("AWS_DEFAULT_REGION") %||%
    "us-east-1"
  access_key <- access_key %||% env_or_null("AWS_ACCESS_KEY_ID")
  secret_key <- secret_key %||% env_or_null("AWS_SECRET_ACCESS_KEY")
  session_token <- session_token %||% env_or_null("AWS_SESSION_TOKEN")
  bucket <- bucket %||% getOption("s3pe.bucket") %||% env_or_null("S3PE_BUCKET")
  timeout <- timeout %||% getOption("s3pe.timeout") %||% 300
  max_tries <- max_tries %||% getOption("s3pe.max_tries") %||% 3

  rlang::check_number_decimal(timeout, min = 0)
  rlang::check_number_whole(max_tries, min = 1)
  parsed <- parse_endpoint(endpoint)

  structure(
    list(
      endpoint = parsed$endpoint,
      host = parsed$host,
      region = region,
      access_key = access_key,
      secret_key = secret_key,
      session_token = session_token,
      bucket = bucket,
      timeout = as.numeric(timeout),
      max_tries = as.integer(max_tries)
    ),
    class = "s3pe_config"
  )
}

env_or_null <- function(name) {
  v <- Sys.getenv(name, unset = "")
  if (nzchar(trimws(v))) v else NULL
}

#' Split `scheme://host[:port]`, dropping a default port
#' @noRd
parse_endpoint <- function(endpoint, call = rlang::caller_env()) {
  rlang::check_string(endpoint, call = call)
  raw <- sub("/+$", "", trimws(endpoint))
  m <- regmatches(raw, regexec("^(https?)://([^/?#]+)$", raw))[[1]]
  if (length(m) != 3) {
    cli::cli_abort(
      c("{.arg endpoint} must be {.code scheme://host[:port]}, with no path.",
        "x" = "Got {.val {endpoint}}."),
      class = c("s3pe_error_config", "s3pe_error"),
      call = call
    )
  }
  scheme <- m[2]
  host <- m[3]
  default_port <- if (scheme == "https") ":443" else ":80"
  if (endsWith(host, default_port)) {
    host <- sub(paste0(default_port, "$"), "", host)
  }
  list(endpoint = paste0(scheme, "://", host), host = host)
}

#' @export
print.s3pe_config <- function(x, ...) {
  key <- x$access_key
  key <- if (is.null(key)) "<unset>" else paste0(substr(key, 1, 4), "\u2026")
  cat("<s3pe_config>\n")
  cat(sprintf("  %-11s %s\n", c(
    "endpoint:", "region:", "bucket:", "access_key:", "secret_key:", "timeout:", "max_tries:"
  ), c(
    x$endpoint, x$region, x$bucket %||% "<none>", key,
    if (is.null(x$secret_key)) "<unset>" else "<set>",
    paste(x$timeout, "s"), as.character(x$max_tries)
  )), sep = "")
  invisible(x)
}

#' Make sure a config is usable for a request
#' @noRd
check_config <- function(config, call = rlang::caller_env()) {
  if (!inherits(config, "s3pe_config")) {
    cli::cli_abort(
      "{.arg config} must come from {.fn s3_config}.",
      class = c("s3pe_error_config", "s3pe_error"), call = call
    )
  }
  if (is.null(config$access_key) || is.null(config$secret_key)) {
    cli::cli_abort(
      c("No credentials for {.url {config$endpoint}}.",
        "i" = "Set {.envvar AWS_ACCESS_KEY_ID} and {.envvar AWS_SECRET_ACCESS_KEY}
               (for example in {.file ~/.Renviron}) or pass {.arg access_key}
               and {.arg secret_key} to {.fn s3_config}."),
      class = c("s3pe_error_config", "s3pe_error"), call = call
    )
  }
  invisible(config)
}

#' Split `s3://bucket/key` or apply the bucket argument / default bucket
#'
#' @returns `list(bucket =, key =)`.
#' @noRd
resolve_key <- function(key, bucket, config, allow_empty = FALSE,
                        call = rlang::caller_env()) {
  rlang::check_string(key, call = call)
  rlang::check_string(bucket, allow_null = TRUE, call = call)
  if (startsWith(key, "s3://")) {
    rest <- substr(key, 6, nchar(key))
    slash <- regexpr("/", rest, fixed = TRUE)
    if (slash == -1) {
      b <- rest
      k <- ""
    } else {
      b <- substr(rest, 1, slash - 1)
      k <- substr(rest, slash + 1, nchar(rest))
    }
    if (!nzchar(b)) {
      cli::cli_abort(
        "No bucket in URL {.val {key}}.",
        class = c("s3pe_error_config", "s3pe_error"), call = call
      )
    }
  } else {
    k <- sub("^/", "", key)
    b <- bucket %||% config$bucket
    if (is.null(b) || !nzchar(b)) {
      cli::cli_abort(
        c("No bucket for key {.val {key}}.",
          "i" = "Write the key as {.code s3://bucket/{key}}, pass {.arg bucket},
                 set {.envvar S3PE_BUCKET} or {.code options(s3pe.bucket = )}."),
        class = c("s3pe_error_config", "s3pe_error"), call = call
      )
    }
  }
  if (!allow_empty && !nzchar(k)) {
    cli::cli_abort(
      "No key in {.val {key}}: give the object's key inside bucket {.val {b}}.",
      class = c("s3pe_error_config", "s3pe_error"), call = call
    )
  }
  list(bucket = b, key = k)
}

#' Set the default bucket or endpoint for the session
#'
#' Shortcuts for `options(s3pe.bucket = )` and `options(s3pe.endpoint = )`:
#' after `use_bucket("iaempe")` every verb accepts bare keys
#' (`read_s3("dados/metadata.json")`); after
#' `use_endpoint("http://127.0.0.1:9003")` requests go to a local server,
#' as inside the server that hosts the store. `NULL` clears the setting.
#'
#' @param bucket Bucket name, or `NULL` to clear.
#' @param endpoint `scheme://host[:port]`, or `NULL` to go back to the
#'   environment's `S3PE_ENDPOINT` and the default `https://s3.pe.gov.br`.
#' @returns The previous value, invisibly.
#' @examplesIf interactive()
#' use_bucket("iaempe")
#' read_s3("dados/metadata.json")
#' use_endpoint("http://127.0.0.1:9003")
#' @export
use_bucket <- function(bucket) {
  rlang::check_string(bucket, allow_null = TRUE)
  old <- getOption("s3pe.bucket")
  options(s3pe.bucket = bucket)
  if (is.null(bucket)) {
    s3_inform(c("i" = "Default bucket cleared."))
  } else {
    s3_inform(c("v" = "Default bucket: {.url s3://{bucket}/}"))
  }
  invisible(old)
}

#' @rdname use_bucket
#' @export
use_endpoint <- function(endpoint) {
  if (!is.null(endpoint)) {
    endpoint <- parse_endpoint(endpoint)$endpoint
  }
  old <- getOption("s3pe.endpoint")
  options(s3pe.endpoint = endpoint)
  if (is.null(endpoint)) {
    s3_inform(c("i" = "Endpoint back to the default ({.url {s3_config()$endpoint}})."))
  } else {
    s3_inform(c("v" = "Endpoint: {.url {endpoint}}"))
  }
  invisible(old)
}
