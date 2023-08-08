# evergarden - a scriptable web archiver

very experimental. don't use in prod, probably

### usage

```bash
cargo run --bin archive -- --config config.toml --output example-archive --start-point "https://example.com"
cargo run --bin evergarden-export -- --input example-archive --output example.wacz
```