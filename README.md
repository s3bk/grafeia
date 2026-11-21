# Γραφείο 

Γραφείο (greek for writing table) is an experiment in typography and editing.
It is intended to be eventually useful to authors who want to escape the dull look of modern books and to give them the tools to create beautiful art – content and layout.

## DEMO
You can [try it in your browser](https://grafeia.github.io/grafeia-wasm), if it supports WebGL2 (+EXT_color_buffer_float) and WebAssembly.

## Set an article as a PDF

The converter turns an article written in markdown, and a style file that says
how each kind of block looks, into a PDF. `data/glowpunk.toml` and
`data/glowpunk.md` are the article this was written for.

```sh
cargo run -p grafeia_convert --features pdf --bin grafeia -- \
    data/glowpunk.md data/glowpunk.toml glowpunk.pdf
```

The markdown is a small subset — `%title` and the other front matter, `#` and
`##` headings, `+` list items, `*lead.*` italic openings, `\pagebreak`,
`\vfill` — and the style is TOML: the paper, the fonts, and one entry per kind
of block, each inheriting from `[types.body]`. `--dump` prints the laid out
columns as text, which is how a heading at the foot of a page is found without
rendering anything.

The PDF carries the markdown and the style it was made from, so the file on its
own is enough to edit the article later:

```sh
cargo run -p grafeia_convert --features pdf --bin grafeia -- --extract glowpunk.pdf out/
```

That writes `out/glowpunk.md` and `out/glowpunk.toml` back out; the font files
they name are the ones in `data/`.

## Run it yourself

1. Fetch the code
```sh
git clone https://github.com/grafeia/grafeia/
cd grafeia
```

2. Then generate the demo file (`demo.graf` in the current directory).
```sh
cargo run --release --bin demo
```

3. Run grafeia and load the demo file
```sh
cargo run --release --bin grafeia_gui demo.graf
```
