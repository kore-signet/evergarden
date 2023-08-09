# evergarden - a scriptable web archiver

**evergarden is a (very experimental as of now) web archiver that supports simple scripting for crawling through new URLs and exports to neatly packaged up .warc.gz files.**

it's meant to be fast and configurable, but does require a bit of technical knowledge still.

you can find example configurations in [configs/](configs/), and example scripts at [scripts/](scripts/)


### usage

```bash
evergarden archive --config configs/html.toml --output example-archive "https://example.com"
evergarden export -i example-archive -o example.wacz
```