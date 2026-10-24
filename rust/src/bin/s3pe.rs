//! `s3pe`: the command line of the crate. Credentials come from the
//! environment (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`), never from
//! arguments, so they do not show in process lists or shell history.

use clap::{Parser, Subcommand};
use s3pe::{format_size, Client, Error};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "s3pe",
    version,
    about = "Read, write, list and delete objects on any S3-compatible store",
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
    },
    /// Delete objects
    Rm { keys: Vec<String> },
    /// Show size, type, date and ETag of an object
    Head { key: String },
    /// List the buckets the credentials can see
    Buckets,
}

struct Stderr;

static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LOGGER: Stderr = Stderr;

impl log::Log for Stderr {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target().starts_with("s3pe")
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
        Command::Ls { path, long } => {
            let path = match path {
                Some(p) => p,
                None if s3.bucket().is_some() => String::new(),
                None => return print_buckets(&s3),
            };
            for o in s3.list(&path)? {
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
        } => {
            s3.upload(&src, &key, content_type.as_deref())?;
        }
        Command::Rm { keys } => {
            for k in keys {
                s3.delete(&k)?;
            }
        }
        Command::Head { key } => match s3.head(&key)? {
            Some(o) => {
                writeln!(out, "key:           {}", o.key)?;
                writeln!(out, "size:          {} ({})", o.size, format_size(o.size))?;
                writeln!(out, "content-type:  {}", o.content_type.unwrap_or_default())?;
                writeln!(out, "last-modified: {}", o.last_modified)?;
                writeln!(out, "etag:          {}", o.etag)?;
            }
            None => {
                let (b, k) = s3.resolve(&key)?;
                return Err(Error::NotFound { bucket: b, key: k });
            }
        },
        Command::Buckets => print_buckets(&s3)?,
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
