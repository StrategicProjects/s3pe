//! `s3pe`: the command line of the crate. Credentials come from the
//! environment (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`), never from
//! arguments, so they do not show in process lists or shell history.

use clap::{Parser, Subcommand};
use s3pe::{format_size, Client, Error, PutOptions, SyncOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "s3pe",
    version,
    about = "Read, write, list, copy, delete and sync objects on any S3-compatible store",
    long_about = None,
    after_help = "Endpoint: --endpoint or S3PE_ENDPOINT (https://host[:port]). Credentials: AWS_ACCESS_KEY_ID\n\
                  and AWS_SECRET_ACCESS_KEY in the environment. Keys are s3://bucket/key URLs, or bare\n\
                  keys when --bucket or S3PE_BUCKET is set."
)]
struct Cli {
    /// Endpoint, scheme://host[:port] (default: $S3PE_ENDPOINT)
    #[arg(long, global = true, value_name = "URL")]
    endpoint: Option<String>,
    /// Signing region (default: $AWS_REGION or us-east-1)
    #[arg(long, global = true, value_name = "REGION")]
    region: Option<String>,
    /// Bucket for bare keys (default: $S3PE_BUCKET)
    #[arg(long, short = 'b', global = true, value_name = "BUCKET")]
    bucket: Option<String>,
    /// Time limit of one call, in seconds
    #[arg(long, global = true, value_name = "SECS", default_value_t = 300)]
    timeout: u64,
    /// Print nothing but results and errors
    #[arg(long, short, global = true)]
    quiet: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List objects under a prefix (or the buckets, with no argument and no bucket)
    Ls {
        /// s3://bucket/prefix, or a prefix inside --bucket
        path: Option<String>,
        /// Show date, size and ETag
        #[arg(long, short)]
        long: bool,
        /// One level only: objects and "folders" directly under the prefix
        #[arg(long, short = 'd')]
        dir: bool,
    },
    /// Print an object to standard output
    Cat { key: String },
    /// Download an object to a file (default: its base name in the current directory)
    Get {
        key: String,
        /// File or directory to write to
        dest: Option<PathBuf>,
    },
    /// Upload a file (a key ending in "/" gets the file's name appended)
    Put {
        src: PathBuf,
        key: String,
        /// Content type (default: guessed from the key)
        #[arg(long, value_name = "TYPE")]
        content_type: Option<String>,
        /// User metadata, name=value (repeatable)
        #[arg(long, value_name = "NAME=VALUE")]
        metadata: Vec<String>,
        /// Extra header, name:value (repeatable; x-amz-* headers are signed)
        #[arg(long, value_name = "NAME:VALUE")]
        header: Vec<String>,
    },
    /// Delete objects (several keys go in one request)
    Rm { keys: Vec<String> },
    /// Copy an object inside the store
    Cp { from: String, to: String },
    /// Show size, type, date, ETag and metadata of an object
    Head { key: String },
    /// List the buckets the credentials can see
    Buckets,
    /// Print a pre-signed URL (GET by default)
    Presign {
        key: String,
        /// Validity in seconds (default one hour, at most seven days)
        #[arg(long, default_value_t = 3600)]
        expires: u64,
        /// Sign an upload instead of a download
        #[arg(long)]
        put: bool,
    },
    /// Mirror a directory to a prefix, or a prefix to a directory (by ETag)
    Sync {
        /// Local directory or s3://bucket/prefix/
        from: String,
        /// s3://bucket/prefix/ or local directory
        to: String,
        /// Remove what exists on the destination but not on the source
        #[arg(long)]
        delete: bool,
        /// File to upload last (a manifest), relative to the directory
        #[arg(long, value_name = "FILE")]
        last: Option<String>,
    },
}

struct Stderr;

static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LOGGER: Stderr = Stderr;

impl log::Log for Stderr {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target().starts_with("s3pe")
            && m.level() <= log::Level::Info
            && (m.level() <= log::Level::Warn || !QUIET.load(std::sync::atomic::Ordering::Relaxed))
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("{}", r.args());
        }
    }
    fn flush(&self) {}
}

fn main() {
    let cli = Cli::parse();
    let quiet = cli.quiet
        || std::env::var("S3PE_QUIET")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false);
    QUIET.store(quiet, std::sync::atomic::Ordering::Relaxed);
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
    if let Err(e) = run(cli) {
        eprintln!("s3pe: {e}");
        std::process::exit(match e {
            Error::NotFound { .. } => 2,
            Error::AccessDenied { .. } => 3,
            Error::Config(_) => 4,
            _ => 1,
        });
    }
}

fn run(cli: Cli) -> Result<(), Error> {
    let mut b = Client::builder().timeout(Duration::from_secs(cli.timeout));
    if let Some(e) = cli.endpoint {
        b = b.endpoint(e);
    }
    if let Some(r) = cli.region {
        b = b.region(r);
    }
    if let Some(k) = cli.bucket {
        b = b.bucket(k);
    }
    let s3 = b.build()?;
    let mut out = std::io::stdout().lock();

    match cli.command {
        Command::Ls { path, long, dir } => {
            let path = match path {
                Some(p) => p,
                None if s3.bucket().is_some() => String::new(),
                None => return print_buckets(&s3),
            };
            let listing = if dir {
                s3.list_dir(&path)?
            } else {
                s3pe::Listing {
                    objects: s3.list(&path)?,
                    prefixes: vec![],
                }
            };
            for p in &listing.prefixes {
                if long {
                    writeln!(out, "{:>24}  {:>12}  {:>32}  {}", "", "DIR", "", p)?;
                } else {
                    writeln!(out, "{p}")?;
                }
            }
            for o in &listing.objects {
                if long {
                    writeln!(
                        out,
                        "{}  {:>12}  {}  {}",
                        o.last_modified, o.size, o.etag, o.key
                    )?;
                } else {
                    writeln!(out, "{}", o.key)?;
                }
            }
        }
        Command::Cat { key } => {
            let data = s3.read(&key)?;
            out.write_all(&data)?;
        }
        Command::Get { key, dest } => {
            let (_, k) = s3.resolve(&key)?;
            let name = k.rsplit('/').next().unwrap_or(&k).to_string();
            let dest = match dest {
                Some(d) if d.is_dir() => d.join(&name),
                Some(d) => d,
                None => PathBuf::from(name),
            };
            s3.download(&key, dest)?;
        }
        Command::Put {
            src,
            key,
            content_type,
            metadata,
            header,
        } => {
            let mut opts = PutOptions {
                content_type,
                ..Default::default()
            };
            for m in metadata {
                let (k, v) = m.split_once('=').ok_or_else(|| {
                    Error::Config(format!("--metadata wants NAME=VALUE, got {m}"))
                })?;
                opts.metadata
                    .push((k.trim().to_string(), v.trim().to_string()));
            }
            for h in header {
                let (k, v) = h
                    .split_once(':')
                    .ok_or_else(|| Error::Config(format!("--header wants NAME:VALUE, got {h}")))?;
                opts.headers
                    .push((k.trim().to_string(), v.trim().to_string()));
            }
            s3.upload_with(&src, &key, &opts)?;
        }
        Command::Rm { keys } => {
            if keys.len() == 1 {
                s3.delete(&keys[0])?;
            } else if !keys.is_empty() {
                s3.delete_many(&keys)?;
            }
        }
        Command::Cp { from, to } => {
            s3.copy(&from, &to)?;
        }
        Command::Head { key } => match s3.head(&key)? {
            Some(o) => {
                writeln!(out, "key:           {}", o.key)?;
                writeln!(out, "size:          {} ({})", o.size, format_size(o.size))?;
                writeln!(out, "content-type:  {}", o.content_type.unwrap_or_default())?;
                writeln!(out, "last-modified: {}", o.last_modified)?;
                writeln!(out, "etag:          {}", o.etag)?;
                for (k, v) in &o.metadata {
                    writeln!(out, "metadata:      {k} = {v}")?;
                }
            }
            None => {
                let (b, k) = s3.resolve(&key)?;
                return Err(Error::NotFound { bucket: b, key: k });
            }
        },
        Command::Buckets => print_buckets(&s3)?,
        Command::Presign { key, expires, put } => {
            let url = s3.presign(
                &key,
                if put { "PUT" } else { "GET" },
                Duration::from_secs(expires),
            )?;
            writeln!(out, "{url}")?;
        }
        Command::Sync {
            from,
            to,
            delete,
            last,
        } => {
            let opts = SyncOptions { delete, last };
            let report = match (from.starts_with("s3://"), to.starts_with("s3://")) {
                (false, true) => s3.sync_up(&from, &to, &opts)?,
                (true, false) => s3.sync_down(&from, &to, &opts)?,
                _ => {
                    return Err(Error::Config(
                        "sync wants one local directory and one s3://bucket/prefix/".into(),
                    ))
                }
            };
            for f in &report.transferred {
                writeln!(out, "{f}")?;
            }
        }
    }
    Ok(())
}

fn print_buckets(s3: &Client) -> Result<(), Error> {
    let mut out = std::io::stdout().lock();
    for b in s3.buckets()? {
        writeln!(out, "{}  {}", b.created, b.name)?;
    }
    Ok(())
}
