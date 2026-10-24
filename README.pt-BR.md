# s3pe

[![DOI](https://zenodo.org/badge/DOI/10.5281/zenodo.23606939.svg)](https://doi.org/10.5281/zenodo.23606939)

Leia e grave dados em qualquer armazenamento de objetos compatível com S3
(AWS S3, Garage, MinIO, Ceph, …) com os mesmos nove verbos em R, Python,
Rust e Go: os mesmos padrões, a mesma assinatura, as mesmas tentativas e as
mesmas verificações de integridade. O endpoint e as credenciais vêm do
ambiente; o formato vem da extensão do arquivo; cada transferência imprime
uma linha.

**Site:** <https://strategicprojects.github.io/s3pe/> · [English](README.md)

| Edição | Instalação | Documentação |
|---|---|---|
| R | `install.packages("s3pe")` · `pak::pak("StrategicProjects/s3pe/r")` | [pkgdown](https://strategicprojects.github.io/s3pe/r/) |
| Python | `pip install s3pe` (`"s3pe[pandas]"` para tabelas) | [site](https://strategicprojects.github.io/s3pe/python/) · [API](https://strategicprojects.github.io/s3pe/python/api/) |
| Rust | `cargo add s3pe` · `cargo install s3pe` (o comando `s3pe`) | [site](https://strategicprojects.github.io/s3pe/rust/) · [docs.rs](https://docs.rs/s3pe) |
| Go | `go get github.com/StrategicProjects/s3pe` | [site](https://strategicprojects.github.io/s3pe/go/) · [pkg.go.dev](https://pkg.go.dev/github.com/StrategicProjects/s3pe) |

## Os nove verbos

```r
library(s3pe)
use_endpoint("https://s3.example.org")
use_bucket("meu-bucket")
meta   <- read_s3("dados/manifesto.json")        # lista, do JSON
vendas <- read_s3("series/vendas.csv.gz")        # tibble
write_s3(vendas, "relatorios/vendas.parquet")
download_s3("dados/regioes.geojson", "regioes.geojson")
list_s3("dados/")
```

| Verbo | Faz |
|---|---|
| `read_s3` / `write_s3` | objeto em memória: decodificado pela extensão em R e Python, bytes em Rust e Go |
| `download_s3` / `upload_s3` | arquivo, em fluxo, gravado como `.s3pe-part` e renomeado ao terminar |
| `list_s3` | todos os objetos sob um prefixo, todas as páginas |
| `exists_s3` / `head_s3` | se o objeto existe; tamanho, tipo, data e ETag |
| `delete_s3` | apaga (idempotente) |
| `buckets_s3` | os buckets que a credencial enxerga |
| `copy_s3` | cópia no servidor |
| `presign_s3` | URL que funciona sem credencial por um tempo (GET ou PUT) |
| `sync_up_s3` / `sync_down_s3` | espelha diretório e prefixo por ETag, manifesto por último |

Arquivos acima de 64 MiB sobem em partes (multipart), em paralelo, com
abort limpo em caso de falha; `write_s3`/`upload_s3` aceitam `metadata` e
`headers`; `delete_s3` apaga vários de uma vez; `list_s3(delimiter = "/")`
lista um nível. Sem ACLs e sem versionamento, por decisão (DESIGN.md D19).

## Configuração

No `~/.Renviron` (R) ou no ambiente do shell (`.env`, `export`):

```
S3PE_ENDPOINT=https://s3.example.org
AWS_ACCESS_KEY_ID=...
AWS_SECRET_ACCESS_KEY=...
S3PE_BUCKET=meu-bucket        # opcional: permite chaves sem s3://
```

O endpoint é obrigatório (`https://host[:porta]`); a região padrão é
`us-east-1`. Nunca deixe a chave em script ou repositório.

## Confiável por padrão

Assinatura AWS V4 sobre o SHA-256 real do corpo (sem `aws-chunked`, que
alguns servidores recusam); três tentativas em falha de conexão e HTTP
429/5xx; erros tipados (não encontrado, acesso negado, HTTP, transporte,
configuração, integridade); ETag conferido com o MD5 local após upload e
`Content-Length` e MD5 após download. Cada edição é testada contra um S3
falso local que verifica todas as assinaturas e, com `S3PE_ENDPOINT` e
`S3PE_TEST_BUCKET`, contra um servidor real. Decisões em
[DESIGN.md](DESIGN.md); mudanças em [CHANGELOG.md](CHANGELOG.md).

## Autores

André Leite, Marcos Wasiliew, Hugo Vasconcelos, Carlos Amorim, Diogo Bezerra
e Júlia Nascimento Barreto. Licença MIT.

## Feito em Pernambuco 🌵

O s3pe é desenvolvido no Recife.
