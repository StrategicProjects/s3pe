#!/usr/bin/env python3
"""Generates the landing pages of the Rust, Go and Python editions
(site/{rust,go,python}/index.html) from one template, in the style of the
foresight family. Run from the repository root after editing EDITIONS."""

from pathlib import Path

CSS = """
  :root { --bg: #0b1020; --bg-soft: #121a33; --card: #16203f; --border: #263457; --fg: #e8edf7; --muted: #9fb0d0; --brand: BRAND; --brand-2: #29c7ac; --accent: #ffb14e; --code-bg: #0d1428; --shadow: 0 12px 40px rgba(0,0,0,.35); --radius: 16px; --maxw: 1080px; }
  @media (prefers-color-scheme: light) { :root { --bg: #f6f8fe; --bg-soft: #eef2fb; --card: #ffffff; --border: #dbe3f4; --fg: #10192e; --muted: #56648a; --code-bg: #0d1428; --shadow: 0 12px 36px rgba(30,45,90,.12); } }
  * { box-sizing: border-box; } html { scroll-behavior: smooth; }
  body { margin: 0; font: 16px/1.65 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif; color: var(--fg); background: radial-gradient(1100px 600px at 12% -8%, color-mix(in srgb, var(--brand) 22%, transparent), transparent 60%), radial-gradient(900px 500px at 96% 4%, rgba(41,199,172,.16), transparent 55%), var(--bg); -webkit-font-smoothing: antialiased; }
  a { color: var(--brand-2); text-decoration: none; } a:hover { text-decoration: underline; }
  .wrap { max-width: var(--maxw); margin: 0 auto; padding: 0 22px; }
  header.nav { position: sticky; top: 0; z-index: 10; backdrop-filter: blur(10px); background: color-mix(in srgb, var(--bg) 78%, transparent); border-bottom: 1px solid var(--border); }
  .nav-inner { display: flex; align-items: center; justify-content: space-between; height: 62px; }
  .brand { display: flex; align-items: center; gap: 10px; font-weight: 800; letter-spacing: -.02em; font-size: 1.1rem; } .brand .logo { font-size: 1.35rem; }
  .nav-links { display: flex; gap: 22px; align-items: center; } .nav-links a { color: var(--muted); font-weight: 600; font-size: .95rem; } .nav-links a:hover { color: var(--fg); text-decoration: none; }
  .btn { display: inline-flex; align-items: center; gap: 8px; padding: 10px 16px; border-radius: 12px; font-weight: 700; font-size: .95rem; border: 1px solid var(--border); color: var(--fg); background: var(--card); transition: transform .12s ease, box-shadow .12s ease; }
  .btn:hover { text-decoration: none; transform: translateY(-1px); box-shadow: var(--shadow); } .btn.primary { background: linear-gradient(135deg, var(--brand), #5b73ff); color: #fff; border: none; }
  @media (max-width: 720px) { .nav-links a.hide-sm { display: none; } }
  .hero { padding: 74px 0 40px; }
  .eyebrow { display: inline-flex; align-items: center; gap: 8px; font-size: .8rem; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; color: var(--brand-2); background: color-mix(in srgb, var(--brand-2) 14%, transparent); border: 1px solid color-mix(in srgb, var(--brand-2) 30%, transparent); padding: 6px 12px; border-radius: 999px; }
  h1 { font-size: clamp(2.2rem, 5vw, 3.4rem); line-height: 1.05; letter-spacing: -.03em; margin: 18px 0 14px; } h1 .grad { background: linear-gradient(120deg, var(--brand), var(--brand-2)); -webkit-background-clip: text; background-clip: text; color: transparent; }
  .lead { font-size: 1.18rem; color: var(--muted); max-width: 46ch; } .badges { display: flex; flex-wrap: wrap; gap: 8px; margin: 24px 0 26px; } .badges img { height: 22px; } .cta { display: flex; flex-wrap: wrap; gap: 12px; }
  .hero-grid { display: grid; grid-template-columns: 1.05fr .95fr; gap: 40px; align-items: center; } @media (max-width: 900px) { .hero-grid { grid-template-columns: 1fr; } }
  .term { background: var(--code-bg); border: 1px solid var(--border); border-radius: var(--radius); box-shadow: var(--shadow); overflow: hidden; }
  .term-bar { display: flex; align-items: center; gap: 8px; padding: 12px 14px; border-bottom: 1px solid rgba(255,255,255,.07); } .dot { width: 12px; height: 12px; border-radius: 50%; } .dot.r { background: #ff5f57; } .dot.y { background: #febc2e; } .dot.g { background: #28c840; }
  .term-title { margin-left: 8px; color: #7f8bb0; font: 600 .82rem ui-monospace, monospace; }
  pre { margin: 0; padding: 18px 18px 22px; overflow-x: auto; } code, pre { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: .88rem; } pre code { color: #d7e0f5; }
  .c-prompt { color: #6ee7c8; } .c-flag { color: #ffb14e; } .c-cmd { color: #8ea2ff; } .c-cmt { color: #6b779b; } .c-ok { color: #59d99a; } .c-str { color: #ffb14e; } .c-ty { color: #6ee7c8; } .c-kw { color: #c9a5ff; }
  section { padding: 56px 0; } section.alt { background: var(--bg-soft); border-top: 1px solid var(--border); border-bottom: 1px solid var(--border); }
  h2 { font-size: clamp(1.6rem, 3.4vw, 2.2rem); letter-spacing: -.02em; margin: 0 0 8px; } .sub { color: var(--muted); margin: 0 0 30px; max-width: 62ch; }
  .grid { display: grid; gap: 18px; } .grid.f3 { grid-template-columns: repeat(3, 1fr); } .grid.f2 { grid-template-columns: repeat(2, 1fr); } @media (max-width: 860px) { .grid.f3, .grid.f2 { grid-template-columns: 1fr; } }
  .card { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: 22px; transition: transform .14s ease, box-shadow .14s ease, border-color .14s ease; }
  .card:hover { transform: translateY(-3px); box-shadow: var(--shadow); border-color: color-mix(in srgb, var(--brand) 40%, var(--border)); }
  .card .ic { font-size: 1.5rem; } .card h3 { margin: 12px 0 6px; font-size: 1.1rem; letter-spacing: -.01em; } .card p { margin: 0; color: var(--muted); font-size: .96rem; }
  .pills { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 6px; } .pill { font: 600 .82rem ui-monospace, monospace; padding: 6px 11px; border-radius: 999px; background: color-mix(in srgb, var(--brand) 12%, transparent); border: 1px solid color-mix(in srgb, var(--brand) 26%, transparent); color: var(--fg); }
  .steps { display: grid; grid-template-columns: repeat(4, 1fr); gap: 14px; } @media (max-width: 860px) { .steps { grid-template-columns: 1fr 1fr; } }
  .step { background: var(--card); border: 1px solid var(--border); border-radius: 14px; padding: 16px; } .step .n { font: 800 .8rem ui-monospace, monospace; color: var(--brand-2); } .step h4 { margin: 6px 0 4px; font-size: 1rem; } .step p { margin: 0; color: var(--muted); font-size: .9rem; }
  .panel { background: var(--code-bg); border: 1px solid var(--border); border-radius: var(--radius); padding: 6px; } .note { color: var(--muted); font-size: .9rem; margin-top: 10px; }
  .diagram { background: #ffffff; border: 1px solid var(--border); border-radius: var(--radius); padding: 22px; box-shadow: var(--shadow); overflow-x: auto; } .diagram img { display: block; width: 100%; max-width: 960px; height: auto; margin: 0 auto; }
  table.cmp { width: 100%; border-collapse: collapse; font-size: .94rem; } table.cmp th, table.cmp td { text-align: left; padding: 10px 12px; border-bottom: 1px solid var(--border); vertical-align: top; } table.cmp th { color: var(--muted); font-weight: 700; }
  table.cmp code { background: color-mix(in srgb, var(--brand) 12%, transparent); padding: 2px 6px; border-radius: 6px; font-size: .86rem; color: var(--fg); } .scroll { overflow-x: auto; }
  footer { padding: 40px 0 60px; color: var(--muted); border-top: 1px solid var(--border); } .foot-inner { display: flex; flex-wrap: wrap; gap: 16px; justify-content: space-between; align-items: center; } .foot-links { display: flex; gap: 18px; flex-wrap: wrap; }
"""

GH = "https://github.com/StrategicProjects/s3pe"

EDITIONS = {
    "rust": dict(
        brand="#f0763b", logo="🦀", name="s3pe", lang="Rust", eyebrow="◆ Rust · S3 client + CLI",
        title="s3pe — S3 client and command line in Rust for s3.pe.gov.br",
        desc="A small, dependable S3 client in Rust: SigV4 signing over the real payload hash, retries, integrity checks, typed errors, and the s3pe command line. Made for s3.pe.gov.br, the Garage store of Pernambuco, usable with any S3-style store.",
        h1='Objects in and out of <span class="grad">s3.pe.gov.br</span>, from Rust and the shell.',
        lead="The core of the s3pe family: nine verbs, a builder, typed errors, and the <code>s3pe</code> command for scripts and servers. ureq and rustls, no system OpenSSL.",
        badges=[("https://crates.io/crates/s3pe", "crates.io", "https://img.shields.io/crates/v/s3pe.svg"),
                ("https://docs.rs/s3pe", "docs.rs", "https://img.shields.io/docsrs/s3pe"),
                (GH + "/actions/workflows/ci.yml", "CI", GH + "/actions/workflows/ci.yml/badge.svg"),
                (GH + "/blob/main/LICENSE", "License MIT", "https://img.shields.io/badge/license-MIT-blue.svg"),
                ("https://doi.org/10.5281/zenodo.23606939", "DOI", "https://zenodo.org/badge/DOI/10.5281/zenodo.23606939.svg")],
        api="api/s3pe/", api_label="API reference", api_ext="https://docs.rs/s3pe",
        term_title="main.rs", term="""<span class="c-kw">use</span> s3pe::{<span class="c-cmd">read_s3</span>, <span class="c-cmd">upload_s3</span>, <span class="c-cmd">list_s3</span>};

<span class="c-cmt">// credentials and endpoint from the environment</span>
<span class="c-kw">let</span> meta = <span class="c-cmd">read_s3</span>(<span class="c-str">"s3://iaempe/dados/metadata.json"</span>)?;
<span class="c-cmd">upload_s3</span>(<span class="c-str">"painel.parquet"</span>, <span class="c-str">"s3://iaempe/etl/"</span>, <span class="c-ty">None</span>)?;
<span class="c-ok">↑ painel.parquet → s3://iaempe/etl/painel.parquet (12.3 MB, 0.8 s)</span>

<span class="c-kw">for</span> o <span class="c-kw">in</span> <span class="c-cmd">list_s3</span>(<span class="c-str">"s3://iaempe/dados/"</span>)? {
    println!(<span class="c-str">"{:>10} {}"</span>, o.size, o.key);
}""",
        features=[
            ("🔏", "Signed the way Garage wants", "AWS Signature V4 over the real SHA-256 of the body. No unsigned payloads, no <code>aws-chunked</code> trailers, path-style URLs."),
            ("🔁", "Retries that know when to stop", "Connection failures and HTTP 429/500/502/503/504: three tries, 1 s then 2 s, signed again with a fresh date. Never on other 4xx."),
            ("✅", "Every byte accounted for", "ETag against the local MD5 after a PUT; Content-Length and MD5 after a GET. Downloads land in a <code>.s3pe-part</code> file and are renamed when complete."),
            ("🧭", "Typed errors", "<code>Error::NotFound</code>, <code>AccessDenied</code>, <code>Http { status, code }</code>, <code>Transport</code>, <code>Config</code>, <code>Integrity</code>. Match on them, do not parse messages."),
            ("🦀", "Small and portable", "ureq with rustls and bundled roots: no OpenSSL to find, builds on Linux, macOS and Windows, Rust 1.85 or later. <code>#![forbid(unsafe_code)]</code>."),
            ("⌨️", "A command line too", "<code>cargo install s3pe</code> gives <code>s3pe ls | cat | get | put | rm | head | buckets</code>, with exit codes for scripts and credentials read only from the environment."),
        ],
        verbs_note="Free functions over a client built from the environment, and the same verbs as methods of <code>Client</code>. Bytes in, bytes out: the crate does not pick formats, the Python and R editions do.",
        verbs=[("read_s3", "GET into a <code>Vec&lt;u8&gt;</code>"), ("write_s3", "PUT bytes, content type guessed from the key"),
               ("download_s3", "GET to a file, streaming"), ("upload_s3", "PUT from a file, streaming, two passes"),
               ("list_s3", "every <code>Object</code> under a prefix, all pages"), ("exists_s3", "<code>bool</code>"),
               ("head_s3", "<code>Option&lt;Object&gt;</code>: size, type, date, ETag"), ("delete_s3", "idempotent"), ("buckets_s3", "<code>Vec&lt;Bucket&gt;</code>")],
        install_cmd="""<span class="c-prompt">$</span> <span class="c-cmd">cargo</span> add s3pe                    <span class="c-cmt"># library (+ CLI target)</span>
<span class="c-prompt">$</span> <span class="c-cmd">cargo</span> add s3pe --no-default-features   <span class="c-cmt"># library only, no clap</span>
<span class="c-prompt">$</span> <span class="c-cmd">cargo</span> install s3pe                <span class="c-cmt"># the s3pe command</span>""",
        panels=[("// an explicit client", """<span class="c-kw">let</span> s3 = s3pe::<span class="c-ty">Client</span>::<span class="c-cmd">builder</span>()
    .<span class="c-cmd">endpoint</span>(<span class="c-str">"http://127.0.0.1:9003"</span>)
    .<span class="c-cmd">credentials</span>(<span class="c-str">"GK…"</span>, <span class="c-str">"…"</span>)
    .<span class="c-cmd">bucket</span>(<span class="c-str">"iaempe"</span>)
    .<span class="c-cmd">build</span>()?;

<span class="c-kw">let</span> bytes = s3.<span class="c-cmd">read</span>(<span class="c-str">"dados/metadata.json"</span>)?;
s3.<span class="c-cmd">download</span>(<span class="c-str">"dados/pe.geojson"</span>, <span class="c-str">"pe.geojson"</span>)?;
<span class="c-kw">if let</span> <span class="c-ty">Some</span>(m) = s3.<span class="c-cmd">head</span>(<span class="c-str">"etl/painel.parquet"</span>)? {
    println!(<span class="c-str">"{} bytes"</span>, m.size);
}
<span class="c-kw">match</span> s3.<span class="c-cmd">read</span>(<span class="c-str">"missing"</span>) {
    <span class="c-ty">Err</span>(s3pe::<span class="c-ty">Error</span>::NotFound { .. }) => {}
    other => { other?; }
}"""),
                ("# the command line", """<span class="c-prompt">$</span> <span class="c-cmd">s3pe</span> buckets
<span class="c-prompt">$</span> <span class="c-cmd">s3pe</span> ls -l s3://iaempe/dados/
<span class="c-prompt">$</span> <span class="c-cmd">s3pe</span> cat s3://iaempe/dados/metadata.json | jq .ultimo_mes
<span class="c-prompt">$</span> <span class="c-cmd">s3pe</span> get s3://iaempe/dados/pe_municipios.geojson
<span class="c-prompt">$</span> <span class="c-cmd">s3pe</span> put painel.parquet s3://iaempe/etl/
<span class="c-ok">↑ painel.parquet → s3://iaempe/etl/painel.parquet (12.3 MB, 0.8 s)</span>
<span class="c-prompt">$</span> <span class="c-cmd">s3pe</span> rm s3://iaempe/etl/old.parquet
<span class="c-prompt">$</span> <span class="c-cmd">s3pe</span> --endpoint http://127.0.0.1:9003 -b iaempe ls dados/

<span class="c-cmt"># exit codes: 0 ok · 2 not found · 3 access denied · 4 config</span>""")],
        install_note="Rust 1.85 or later. Changes between versions in the <a href=\"" + GH + "/blob/main/CHANGELOG.md\">changelog</a>.",
        arch_text="The crate is the core of the family: the Python package is a PyO3 binding on it, while R and Go reimplement the same specification in their own idiom. One repository, one version, one <a href=\"" + GH + "/blob/main/DESIGN.md\">DESIGN.md</a>.",
        family=[("R", "../r/", "install.packages(\"s3pe\")"), ("Python", "../python/", "pip install s3pe"), ("Go", "../go/", "go get github.com/StrategicProjects/s3pe")],
    ),
    "go": dict(
        brand="#00add8", logo="🐹", name="s3pe", lang="Go", eyebrow="◆ Go · S3 client",
        title="s3pe — S3 client in Go for s3.pe.gov.br, standard library only",
        desc="A small, dependable S3 client in Go: SigV4 signing over the real payload hash, retries, integrity checks, typed errors for errors.Is, context on every call, no dependencies. Made for s3.pe.gov.br, the Garage store of Pernambuco, usable with any S3-style store.",
        h1='Objects in and out of <span class="grad">s3.pe.gov.br</span>, in Go.',
        lead="Nine verbs over net/http and crypto/hmac, context on every call, errors you can <code>errors.Is</code>. No cgo, no <code>require</code> in go.mod.",
        badges=[("https://pkg.go.dev/github.com/StrategicProjects/s3pe", "Go Reference", "https://pkg.go.dev/badge/github.com/StrategicProjects/s3pe.svg"),
                (GH + "/actions/workflows/ci.yml", "CI", GH + "/actions/workflows/ci.yml/badge.svg"),
                (GH + "/blob/main/LICENSE", "License MIT", "https://img.shields.io/badge/license-MIT-blue.svg"),
                ("https://doi.org/10.5281/zenodo.23606939", "DOI", "https://zenodo.org/badge/DOI/10.5281/zenodo.23606939.svg"),
                (None, "No dependencies", "https://img.shields.io/badge/dependencies-none-brightgreen.svg")],
        api="https://pkg.go.dev/github.com/StrategicProjects/s3pe", api_label="Reference", api_ext=None,
        term_title="main.go", term="""<span class="c-kw">import</span> <span class="c-str">"github.com/StrategicProjects/s3pe"</span>

ctx := context.<span class="c-cmd">Background</span>()
<span class="c-cmt">// credentials and endpoint from the environment</span>
meta, err := s3pe.<span class="c-cmd">ReadS3</span>(ctx, <span class="c-str">"s3://iaempe/dados/metadata.json"</span>)
_, err = s3pe.<span class="c-cmd">UploadS3</span>(ctx, <span class="c-str">"painel.parquet"</span>, <span class="c-str">"s3://iaempe/etl/"</span>, <span class="c-str">""</span>)
<span class="c-ok">↑ painel.parquet → s3://iaempe/etl/painel.parquet (12.3 MB, 0.8 s)</span>

objects, err := s3pe.<span class="c-cmd">ListS3</span>(ctx, <span class="c-str">"s3://iaempe/dados/"</span>)
<span class="c-kw">for</span> _, o := <span class="c-kw">range</span> objects {
    fmt.<span class="c-cmd">Println</span>(o.Size, o.Key)
}""",
        features=[
            ("🔏", "Signed the way Garage wants", "AWS Signature V4 over the real SHA-256 of the body. No unsigned payloads, no <code>aws-chunked</code> trailers, path-style URLs."),
            ("🔁", "Retries that know when to stop", "Connection failures and HTTP 429/500/502/503/504: three tries, 1 s then 2 s, signed again with a fresh date. Never on other 4xx."),
            ("✅", "Every byte accounted for", "ETag against the local MD5 after a PUT; Content-Length and MD5 after a GET. Downloads land in a <code>.s3pe-part</code> file and are renamed when complete."),
            ("🧭", "Errors for errors.Is", "<code>ErrNotFound</code>, <code>ErrAccessDenied</code>, <code>ErrHTTP</code>, <code>ErrTransport</code>, <code>ErrConfig</code>, <code>ErrIntegrity</code>, with the status, code and key in a <code>*s3pe.Error</code> for <code>errors.As</code>."),
            ("🐹", "Standard library only", "net/http, crypto/hmac, crypto/sha256, encoding/xml. No cgo, nothing to vet but the package itself. Go 1.22 or later."),
            ("⏱️", "Context everywhere", "Every call takes a <code>context.Context</code>; cancel it and the transfer stops. A per-call timeout applies when the context has no deadline."),
        ],
        verbs_note="Package-level verbs over a client built from the environment on first use, and the same verbs as methods of <code>*Client</code>. Bytes in, bytes out: the module does not pick formats, the Python and R editions do.",
        verbs=[("ReadS3", "GET into a <code>[]byte</code>"), ("WriteS3", "PUT bytes, content type guessed from the key"),
               ("DownloadS3", "GET to a file, streaming"), ("UploadS3", "PUT from a file, streaming, two passes"),
               ("ListS3", "every <code>Object</code> under a prefix, all pages"), ("ExistsS3", "<code>bool</code>"),
               ("HeadS3", "<code>*Object</code> or nil: size, type, date, ETag"), ("DeleteS3", "idempotent"), ("BucketsS3", "<code>[]Bucket</code>")],
        install_cmd="""<span class="c-prompt">$</span> <span class="c-cmd">go</span> get github.com/StrategicProjects/s3pe""",
        panels=[("// an explicit client", """s3, err := s3pe.<span class="c-cmd">New</span>(s3pe.<span class="c-ty">Config</span>{
    Endpoint:  <span class="c-str">"http://127.0.0.1:9003"</span>,
    AccessKey: key, SecretKey: secret,
    Bucket:    <span class="c-str">"iaempe"</span>,
})
data, err := s3.<span class="c-cmd">Read</span>(ctx, <span class="c-str">"dados/metadata.json"</span>)
_, err = s3.<span class="c-cmd">Download</span>(ctx, <span class="c-str">"dados/pe.geojson"</span>, <span class="c-str">"pe.geojson"</span>)
meta, err := s3.<span class="c-cmd">Head</span>(ctx, <span class="c-str">"etl/painel.parquet"</span>)
<span class="c-kw">if</span> meta != <span class="c-kw">nil</span> {
    fmt.<span class="c-cmd">Println</span>(meta.Size, meta.LastModified)
}"""),
                ("// errors you can test", """_, err = s3.<span class="c-cmd">Read</span>(ctx, <span class="c-str">"missing"</span>)
<span class="c-kw">if</span> errors.<span class="c-cmd">Is</span>(err, s3pe.ErrNotFound) {
    <span class="c-cmt">// 404: nothing there</span>
}
<span class="c-kw">var</span> e *s3pe.<span class="c-ty">Error</span>
<span class="c-kw">if</span> errors.<span class="c-cmd">As</span>(err, &amp;e) {
    log.<span class="c-cmd">Println</span>(e.Status, e.Code, e.Key)
}

<span class="c-cmt">// quiet, or your own logger</span>
s3, _ = s3pe.<span class="c-cmd">New</span>(s3pe.<span class="c-ty">Config</span>{Quiet: <span class="c-kw">true</span>})
s3, _ = s3pe.<span class="c-cmd">New</span>(s3pe.<span class="c-ty">Config</span>{Logger: log.<span class="c-cmd">Default</span>()})""")],
        install_note="Go 1.22 or later. pkg.go.dev indexes each tag; changes between versions in the <a href=\"" + GH + "/blob/main/CHANGELOG.md\">changelog</a>.",
        arch_text="The Go module lives at the root of the family's repository, so its import path is the repository URL. It reimplements the shared specification on the standard library; the Rust crate is the reference, and the four signers reproduce the AWS documentation vector. One repository, one version, one <a href=\"" + GH + "/blob/main/DESIGN.md\">DESIGN.md</a>.",
        family=[("R", "../r/", "install.packages(\"s3pe\")"), ("Python", "../python/", "pip install s3pe"), ("Rust", "../rust/", "cargo add s3pe")],
    ),
    "python": dict(
        brand="#3776ab", logo="🐍", name="s3pe", lang="Python", eyebrow="◆ Python · S3 client",
        title="s3pe — read and write data on s3.pe.gov.br from Python",
        desc="read_s3 and write_s3 for Python: DataFrames, dicts and files in and out of s3.pe.gov.br, the Garage store of Pernambuco, or any S3-style store. A Rust core signs, retries and checks every transfer; wheels for Linux, macOS and Windows.",
        h1='Data in and out of <span class="grad">s3.pe.gov.br</span>, in Python.',
        lead="Nine verbs that read and write DataFrames, dicts, text and files by extension, on a Rust core that signs, retries and verifies every transfer. <code>pip install s3pe</code>, two environment variables, done.",
        badges=[("https://pypi.org/project/s3pe/", "PyPI", "https://img.shields.io/pypi/v/s3pe.svg"),
                (GH + "/actions/workflows/ci.yml", "CI", GH + "/actions/workflows/ci.yml/badge.svg"),
                (GH + "/blob/main/LICENSE", "License MIT", "https://img.shields.io/badge/license-MIT-blue.svg"),
                ("https://doi.org/10.5281/zenodo.23606939", "DOI", "https://zenodo.org/badge/DOI/10.5281/zenodo.23606939.svg"),
                (None, "Python 3.9+", "https://img.shields.io/badge/python-3.9%2B-informational.svg")],
        api="api/", api_label="API reference", api_ext=None,
        term_title="analysis.py", term="""<span class="c-kw">import</span> s3pe

<span class="c-cmt"># credentials and endpoint from the environment</span>
meta = s3pe.<span class="c-cmd">read_s3</span>(<span class="c-str">"s3://iaempe/dados/metadata.json"</span>)   <span class="c-cmt"># dict</span>
<span class="c-ok">↓ s3://iaempe/dados/metadata.json (1.2 kB, 0.2 s)</span>

df = s3pe.<span class="c-cmd">read_s3</span>(<span class="c-str">"series/estado/icms.csv.gz"</span>,
                  bucket=<span class="c-str">"horizonte-fiscal"</span>)        <span class="c-cmt"># DataFrame</span>
s3pe.<span class="c-cmd">write_s3</span>(df, <span class="c-str">"s3://iaempe/nota/icms.parquet"</span>)
<span class="c-ok">↑ s3://iaempe/nota/icms.parquet (84.1 kB, 0.3 s)</span>

<span class="c-kw">for</span> o <span class="c-kw">in</span> s3pe.<span class="c-cmd">list_s3</span>(<span class="c-str">"s3://iaempe/dados/"</span>):
    print(o.size, o.key)""",
        features=[
            ("🗂️", "Formats by extension", "<code>.json</code> → dict, <code>.csv</code>/<code>.csv.gz</code>/<code>.tsv</code>/<code>.parquet</code>/<code>.xlsx</code> → DataFrame, text files → str, anything else → bytes. <code>format=</code> overrides; extra arguments go to pandas."),
            ("🦀", "A Rust core", "Signing, retries and checks happen in the s3pe crate; the GIL is released during transfers. Wheels for Linux (x86_64, aarch64), macOS and Windows, Python 3.9 or later."),
            ("🔏", "Signed the way Garage wants", "AWS Signature V4 over the real SHA-256 of the body. No unsigned payloads, no <code>aws-chunked</code> trailers, path-style URLs."),
            ("🔁", "Retries that know when to stop", "Connection failures and HTTP 429/500/502/503/504: three tries, 1 s then 2 s. Never on other 4xx."),
            ("✅", "Every byte accounted for", "ETag against the local MD5 after a PUT; Content-Length and MD5 after a GET. Downloads land in a <code>.s3pe-part</code> file and are renamed when complete."),
            ("🧭", "Typed exceptions", "<code>NotFoundError</code>, <code>AccessDeniedError</code>, <code>HttpError</code> (with <code>.status</code>, <code>.code</code>), <code>TransportError</code>, <code>ConfigError</code>, <code>IntegrityError</code>, all under <code>S3Error</code>."),
        ],
        verbs_note="Module-level verbs over a default client (<code>s3pe.configure(bucket=\"iaempe\")</code> sets it), and the same verbs as methods of <code>s3pe.Client</code>.",
        verbs=[("read_s3", "GET, decoded by extension or <code>format=</code>"), ("write_s3", "PUT a DataFrame, dict, list, str, bytes, file-like or Path"),
               ("download_s3", "GET to a file, streaming; refuses to overwrite unless asked"), ("upload_s3", "PUT from a file, streaming"),
               ("list_s3", "every <code>S3Object</code> under a prefix, all pages"), ("exists_s3", "<code>bool</code>"),
               ("head_s3", "<code>S3Object</code> or <code>None</code>: size, type, date, ETag"), ("delete_s3", "idempotent"), ("buckets_s3", "list of <code>Bucket</code>")],
        install_cmd="""<span class="c-prompt">$</span> <span class="c-cmd">pip</span> install s3pe              <span class="c-cmt"># bytes, str, dict/list and files</span>
<span class="c-prompt">$</span> <span class="c-cmd">pip</span> install <span class="c-str">"s3pe[pandas]"</span>     <span class="c-cmt"># also CSV, Parquet and Excel as DataFrames</span>""",
        panels=[("# an explicit client", """s3 = s3pe.<span class="c-ty">Client</span>(endpoint=<span class="c-str">"http://127.0.0.1:9003"</span>,
                 bucket=<span class="c-str">"iaempe"</span>,
                 access_key=<span class="c-str">"GK…"</span>, secret_key=<span class="c-str">"…"</span>)
s3.<span class="c-cmd">read</span>(<span class="c-str">"dados/metadata.json"</span>)
s3.<span class="c-cmd">write</span>(df, <span class="c-str">"nota/icms.csv"</span>, index=<span class="c-kw">False</span>)
s3.<span class="c-cmd">download</span>(<span class="c-str">"dados/pe_municipios.geojson"</span>, <span class="c-str">"pe.geojson"</span>)
s3.<span class="c-cmd">upload</span>(<span class="c-str">"figura1.png"</span>, <span class="c-str">"nota/"</span>)
s3.<span class="c-cmd">head</span>(<span class="c-str">"etl/painel.parquet"</span>)      <span class="c-cmt"># S3Object or None</span>

s3pe.<span class="c-cmd">quiet</span>()                        <span class="c-cmt"># or S3PE_QUIET=1</span>"""),
                ("# errors and formats", """<span class="c-kw">try</span>:
    s3pe.<span class="c-cmd">read_s3</span>(<span class="c-str">"s3://iaempe/missing.json"</span>)
<span class="c-kw">except</span> s3pe.<span class="c-ty">NotFoundError</span>:
    ...
<span class="c-kw">except</span> s3pe.<span class="c-ty">HttpError</span> <span class="c-kw">as</span> e:
    print(e.status, e.code)

raw = s3pe.<span class="c-cmd">read_s3</span>(<span class="c-str">"x.csv"</span>, bucket=<span class="c-str">"b"</span>, format=<span class="c-str">"bytes"</span>)
df  = s3pe.<span class="c-cmd">read_s3</span>(<span class="c-str">"x.csv"</span>, bucket=<span class="c-str">"b"</span>, sep=<span class="c-str">";"</span>, dtype=str)
s3pe.<span class="c-cmd">write_s3</span>({<span class="c-str">"ok"</span>: <span class="c-kw">True</span>}, <span class="c-str">"s3://b/manifest.json"</span>)
s3pe.<span class="c-cmd">write_s3</span>(df, <span class="c-str">"s3://b/series.csv.gz"</span>)   <span class="c-cmt"># gzipped on the way</span>""")],
        install_note="Python 3.9 or later; pandas and pyarrow only for tables. Changes between versions in the <a href=\"" + GH + "/blob/main/CHANGELOG.md\">changelog</a>.",
        arch_text="The Python package is a thin layer: formats and logging in Python, everything on the wire in the Rust crate through PyO3. R and Go reimplement the same specification; the four signers reproduce the AWS documentation vector. One repository, one version, one <a href=\"" + GH + "/blob/main/DESIGN.md\">DESIGN.md</a>.",
        family=[("R", "../r/", "install.packages(\"s3pe\")"), ("Rust", "../rust/", "cargo add s3pe"), ("Go", "../go/", "go get github.com/StrategicProjects/s3pe")],
    ),
}


def page(key, e):
    badges = "\n".join(
        (f'          <a href="{href}"><img alt="{alt}" src="{src}"></a>' if href else f'          <img alt="{alt}" src="{src}">')
        for href, alt, src in e["badges"])
    features = "\n".join(f'        <div class="card"><div class="ic">{ic}</div><h3>{h}</h3><p>{p}</p></div>' for ic, h, p in e["features"])
    verbs = "\n".join(f'          <tr><td><code>{v}</code></td><td>{d}</td></tr>' for v, d in e["verbs"])
    panels = "\n".join(f'''        <div><div class="panel">
<pre><code><span class="c-cmt">{t}</span>
{c}</code></pre>
        </div></div>''' for t, c in e["panels"])
    family = "\n".join(f'        <div class="card"><h3>{n}</h3><pre><code>{cmd}</code></pre><div class="note"><a href="{href}">{n} edition →</a></div></div>' for n, href, cmd in e["family"])
    api_nav = f'<a class="hide-sm" href="{e["api"]}">{e["api_label"]}</a>'
    api_btn = f'<a class="btn" href="{e["api"]}">{e["api_label"]}</a>'
    css = CSS.replace("BRAND", e["brand"])
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>{e["title"]}</title>
<meta name="description" content="{e["desc"]}" />
<meta property="og:title" content="{e["title"]}" />
<meta property="og:description" content="{e["desc"]}" />
<meta property="og:type" content="website" />
<meta property="og:url" content="https://strategicprojects.github.io/s3pe/{key}/" />
<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'%3E%3Ctext y='.9em' font-size='90'%3E%F0%9F%AA%A3%3C/text%3E%3C/svg%3E" />
<style>{css}</style>
</head>
<body>
<header class="nav">
  <div class="wrap nav-inner">
    <div class="brand"><span class="logo">{e["logo"]}</span> s3pe <span style="color:var(--muted);font-weight:600">· {e["lang"]}</span></div>
    <nav class="nav-links">
      <a class="hide-sm" href="#features">Features</a>
      <a class="hide-sm" href="#verbs">Verbs</a>
      <a class="hide-sm" href="#how">How it works</a>
      <a class="hide-sm" href="#install">Install</a>
      <a class="hide-sm" href="#architecture">Architecture</a>
      {api_nav}
      <a class="hide-sm" href="../">All editions</a>
      <a class="btn" href="{GH}">GitHub ↗</a>
    </nav>
  </div>
</header>

<main>
  <section class="hero">
    <div class="wrap hero-grid">
      <div>
        <span class="eyebrow">{e["eyebrow"]}</span>
        <h1>{e["h1"]}</h1>
        <p class="lead">{e["lead"]}</p>
        <div class="badges">
{badges}
        </div>
        <div class="cta">
          <a class="btn primary" href="#install">Get started →</a>
          {api_btn}
          <a class="btn" href="{GH}">GitHub ★</a>
        </div>
      </div>
      <div class="term" aria-hidden="true">
        <div class="term-bar"><span class="dot r"></span><span class="dot y"></span><span class="dot g"></span><span class="term-title">{e["term_title"]}</span></div>
<pre><code>{e["term"]}</code></pre>
      </div>
    </div>
  </section>

  <section id="features" class="alt">
    <div class="wrap">
      <h2>Built to be trusted with the data</h2>
      <p class="sub">A client that moves the files a pipeline depends on has one job: either the bytes arrived, or you hear about it. Everything here serves that.</p>
      <div class="grid f3">
{features}
      </div>
    </div>
  </section>

  <section id="verbs">
    <div class="wrap">
      <h2>The nine verbs</h2>
      <p class="sub">{e["verbs_note"]}</p>
      <div class="scroll"><table class="cmp">
        <thead><tr><th>Verb</th><th>Does</th></tr></thead>
        <tbody>
{verbs}
        </tbody>
      </table></div>
      <p class="note">A key is <code>s3://bucket/key</code>, or a bare key when the bucket is configured (<code>S3PE_BUCKET</code> or the client). Credentials: <code>AWS_ACCESS_KEY_ID</code> and <code>AWS_SECRET_ACCESS_KEY</code>; endpoint <code>S3PE_ENDPOINT</code>, default <code>https://s3.pe.gov.br</code>.</p>
    </div>
  </section>

  <section id="how" class="alt">
    <div class="wrap">
      <h2>What happens in every call</h2>
      <p class="sub">The same six steps in the four editions.</p>
      <div class="diagram"><img src="../assets/request-flow.svg" alt="Six steps: resolve the key, hash the payload, sign with SigV4, send with Content-Length, retry on 429 and 5xx after 1 and 2 seconds, verify the ETag and Content-Length. Success prints one log line; errors are typed." loading="lazy" /></div>
      <div class="steps" style="margin-top:18px">
        <div class="step"><div class="n">01</div><h4>Resolve and hash</h4><p><code>s3://bucket/key</code> or bucket plus key; SHA-256 for the signature, MD5 for the check.</p></div>
        <div class="step"><div class="n">02</div><h4>Sign and send</h4><p>SigV4 over host, date and payload hash; path-style, Content-Length, no chunked trailers.</p></div>
        <div class="step"><div class="n">03</div><h4>Retry</h4><p>Connection failures and 429/5xx, three tries, 1 s then 2 s. Other 4xx come straight back.</p></div>
        <div class="step"><div class="n">04</div><h4>Verify and report</h4><p>ETag and Content-Length checked; one line on stderr; typed errors otherwise.</p></div>
      </div>
    </div>
  </section>

  <section id="install">
    <div class="wrap">
      <h2>Install &amp; run</h2>
      <p class="sub">{e["install_note"]}</p>
      <div class="panel" style="margin-bottom:18px">
<pre><code>{e["install_cmd"]}</code></pre>
      </div>
      <div class="grid f2">
{panels}
      </div>
    </div>
  </section>

  <section id="architecture" class="alt">
    <div class="wrap">
      <h2>Architecture</h2>
      <p class="sub">{e["arch_text"]}</p>
      <div class="diagram"><img src="../assets/architecture.svg" alt="Four editions (R on httr2 and openssl; Python as a PyO3 binding on the Rust crate; the Rust crate on ureq and rustls with the s3pe command line; Go on net/http) share one specification and talk to s3.pe.gov.br, a Garage server, or any S3 store." loading="lazy" /></div>
      <div class="grid f3" style="margin-top:18px">
{family}
      </div>
    </div>
  </section>
</main>

<footer>
  <div class="wrap foot-inner">
    <div>s3pe · MIT · André Leite, Marcos Wasiliew, Hugo Vasconcelos, Carlos Amorim, Diogo Bezerra, Júlia Nascimento Barreto</div>
    <div class="foot-links">
      <a href="{GH}">GitHub</a>
      <a href="{GH}/blob/main/CHANGELOG.md">Changelog</a>
      <a href="{GH}/blob/main/DESIGN.md">Design</a>
      <a href="../">All editions</a>
    </div>
  </div>
</footer>
</body>
</html>
"""


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent
    for key, e in EDITIONS.items():
        out = root / "site" / key / "index.html"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(page(key, e))
        print(out.relative_to(root), len(out.read_text()))
