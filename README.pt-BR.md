# s3pe

Leia e grave dados no **s3.pe.gov.br**, o armazenamento de objetos S3 do
Estado de Pernambuco (um servidor [Garage](https://garagehq.deuxfleurs.fr)),
ou em qualquer S3. Os mesmos nove verbos em R, Python, Rust e Go, com os
mesmos padrões, a mesma assinatura, as mesmas tentativas e as mesmas
verificações de integridade. As credenciais vêm de duas variáveis de
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
meta <- read_s3("s3://iaempe/dados/metadata.json")      # lista, do JSON
icms <- read_s3("series/estado/icms.csv.gz", bucket = "horizonte-fiscal")  # tibble
write_s3(icms, "s3://iaempe/nota/icms.parquet")
download_s3("s3://iaempe/dados/pe_municipios.geojson", "pe.geojson")
list_s3("s3://iaempe/dados/")
```

| Verbo | Faz |
|---|---|
| `read_s3` / `write_s3` | objeto em memória: decodificado pela extensão em R e Python, bytes em Rust e Go |
| `download_s3` / `upload_s3` | arquivo, em fluxo, gravado como `.s3pe-part` e renomeado ao terminar |
| `list_s3` | todos os objetos sob um prefixo, todas as páginas |
| `exists_s3` / `head_s3` | se o objeto existe; tamanho, tipo, data e ETag |
| `delete_s3` | apaga (idempotente) |
| `buckets_s3` | os buckets que a credencial enxerga |

## Credenciais

No `~/.Renviron` (R) ou no ambiente do shell (`.env`, `export`):

```
AWS_ACCESS_KEY_ID=GK...
AWS_SECRET_ACCESS_KEY=...
S3PE_BUCKET=iaempe        # opcional: permite chaves sem s3://
```

O endpoint padrão é `https://s3.pe.gov.br` e a região `us-east-1`. Dentro
do servidor condepe use `S3PE_ENDPOINT=http://127.0.0.1:9003`. Nunca deixe a
chave em script ou repositório.

## Confiável por padrão

Assinatura AWS V4 sobre o SHA-256 real do corpo (sem `aws-chunked`, que o
Garage recusa); três tentativas em falha de conexão e HTTP 429/5xx; erros
tipados (não encontrado, acesso negado, HTTP, transporte, configuração,
integridade); ETag conferido com o MD5 local após upload e `Content-Length`
e MD5 após download. Cada edição é testada contra um S3 falso local que
verifica todas as assinaturas e, com `S3PE_TEST_BUCKET`, contra o servidor
real. Decisões em [DESIGN.md](DESIGN.md); mudanças em [CHANGELOG.md](CHANGELOG.md).

## Autores

André Leite, Marcos Wasiliew, Hugo Vasconcelos, Carlos Amorim, Diogo Bezerra
e Júlia Nascimento Barreto. Licença MIT.
