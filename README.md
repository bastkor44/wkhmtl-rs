# wkhmtl-rs

**Drop-in `wkhtmltopdf` replacement for Odoo — pure Rust, no browser, no Qt.**

Built on the [fulgur](https://github.com/fulgur-rs/fulgur) HTML/CSS→PDF engine
(Blitz → Stylo → Taffy → Krilla). Single static binary, ~50 ms cold start,
no Chromium/WebKit dependency.

## Why

Odoo drives `wkhtmltopdf` as a subprocess with a specific CLI dialect (see
`ir_actions_report.py::_build_wkhtmltopdf_args`) and parses `--version` output.
This binary reproduces that contract exactly:

- `--version` → `wkhtmltopdf 0.12.6 (with patched qt)` — Odoo detects "ok"
  state, patched-QT multi-document mode, and dpi zoom ratio (≥ 0.12.2).
- Supports every flag Odoo emits: `--page-size`, `--page-width/--page-height`,
  `--orientation`, `--margin-{top,bottom,left,right}`, `--dpi`, `--zoom`,
  `--header-html`, `--footer-html`, `--header-spacing`, `--header-line`,
  `--disable-smart-shrinking`, `--disable-local-file-access`,
  `--javascript-delay`, `--viewport-size`, `--cookie-jar`, `--quiet`.
- Multiple input HTML files → one merged PDF with per-document **top-level
  outline entries**, which Odoo's `_split_pdf_from_reports` uses to split
  batch reports back into per-record PDFs.
- Header/footer fragments stamped onto every page via Form XObjects.

## Install for Odoo

```bash
cargo build --release
sudo cp target/release/wkhtmltopdf /usr/local/bin/wkhtmltopdf
```

That's it — Odoo finds it on `$PATH` and treats it as a fully-patched Qt
wkhtmltopdf 0.12.6. Remove the real wkhtmltopdf first if it's also installed.

## Usage (standalone)

```bash
wkhtmltopdf --page-size A4 --margin-top 18 invoice.html out.pdf
wkhtmltopdf inv1.html inv2.html inv3.html batch.pdf   # merged, outlined
wkhtmltopdf --header-html hdr.html --footer-html ftr.html body.html out.pdf
```

## Notes & limitations

- Rendering is done by fulgur's own CSS engine — not WebKit. Standard Odoo
  report layouts (Bootstrap-era tables, floats, flexbox basics) render well;
  exotic WebKit-specific CSS may differ slightly. Test your print formats.
- `--cookie-jar` is parsed but unused (no network fetching; pass local files).
- `--javascript-delay` is accepted as a compatibility no-op (no JS engine).

## License

GPL-3.0-or-later — see [COPYING](COPYING).

Note: fulgur is dual MIT/Apache-2.0 and is consumed as a library dependency,
which is compatible with GPLv3 distribution of this binary.
