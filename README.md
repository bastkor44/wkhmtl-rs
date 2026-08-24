# wkhmtl-rs

**Drop-in `wkhtmltopdf` replacement for Odoo — pure Rust, no browser, no Qt.**

Built on the [fulgur](https://github.com/fulgur-rs/fulgur) HTML/CSS→PDF engine
(Blitz → Stylo → Taffy → Krilla). Single static binary, ~50 ms cold start,
no Chromium/WebKit dependency.

## Why

Odoo drives `wkhtmltopdf` as a subprocess with a specific CLI dialect (see
`ir_actions_report.py::_build_wkhtmltopdf_args`) and parses `--version` output.
This binary reproduces that contract:

- `--version` → `wkhtmltopdf 0.12.6 (with patched qt)` — Odoo detects "ok"
  state, patched-QT multi-document mode, and dpi zoom ratio (≥ 0.12.2).
- Parses every flag Odoo emits: `--page-size`, `--page-width/--page-height`,
  `--orientation`, `--margin-{top,bottom,left,right}`, `--dpi`, `--zoom`,
  `--header-html`, `--footer-html`, `--header-spacing`, `--header-line`,
  `--disable-smart-shrinking`, `--disable-local-file-access`,
  `--javascript-delay`, `--viewport-size`, `--cookie-jar`, `--quiet`.
- Multiple input HTML files → one merged PDF with per-document **top-level
  outline entries** in wkhtml `/Dest` + catalog `/Dests` form, which Odoo's
  `_split_pdf_from_reports` uses to split batch reports back into per-record
  PDFs.
- Header/footer fragments stamped onto every page as Form XObjects, clipped
  to the top/bottom margin bands.
- Hard failures print to stderr and exit **2** (Odoo treats exit 1 as
  success-with-warning).

## Install

### One-liner (curl)

```bash
curl -fsSL https://raw.githubusercontent.com/bastkor44/wkhmtl-rs/main/install.sh | bash -s -- --user
```

Drop `--user` (and prefix with `sudo`) for a system-wide install to
`/usr/local/bin`. The script:

- installs a Rust toolchain via rustup if `cargo` is missing,
- verifies the sibling fulgur crate is present (build-time dependency),
- runs `cargo build --release`,
- installs the binary as `wkhtmltopdf` (refusing to clobber a real
  wkhtmltopdf unless you confirm / pass `--force`),
- smoke-tests the installed binary by rendering a tiny PDF.

### Manual

```bash
git clone git@github.com:bastkor44/wkhmtl-rs.git
cd wkhmtl-rs
cargo build --release
sudo cp target/release/wkhtmltopdf /usr/local/bin/wkhtmltopdf
```

That's it — Odoo finds it on `$PATH` and treats it as a fully-patched Qt
wkhtmltopdf 0.12.6. Remove the real wkhtmltopdf first if it's also installed.

### Point Odoo at it

Either start Odoo with:

```bash
odoo --wkhtmltopdf=/usr/local/bin/wkhtmltopdf
```

or in `odoo.conf`:

```ini
[options]
wkhtmltopdf = /usr/local/bin/wkhtmltopdf
```

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
- `--zoom` is injected as `<style>html { zoom: N }</style>` on each body.
  `--dpi` is parsed (Odoo always sends both together for patched Qt).
- `--header-line` strokes a rule under the header band. `--header-spacing`
  is parsed and used for that rule’s y-position; header band placement itself
  stays `page_height − margin_top`.
- `--javascript-delay` is accepted as a compatibility no-op (no JS engine).

### Known gaps

- **No HTTP(S) / cookie-jar asset fetching.** fulgur drops non-`file://` URLs,
  so remote CSS and logos (`/web/assets/…`, `/web/image/…`) will not load.
  `--cookie-jar` is parsed but unused. Inline `<style>` survives.
- **Header/footer `subst()` JS protocol is not implemented.** Odoo’s
  `web.minimal_layout` relies on wkhtmltopdf injecting `?page=&topage=&webpage=`
  and running `subst()` so per-record headers and `.page` / `.topage` resolve.
  The same static fragment is stamped on every page.
- **Zoom is a CSS hint.** If fulgur ignores `html { zoom }`, patched-Qt
  `--zoom 96.0/dpi` scaling will not take effect.

## License

GPL-3.0-or-later — see [COPYING](COPYING).

Note: fulgur is dual MIT/Apache-2.0 and is consumed as a library dependency,
which is compatible with GPLv3 distribution of this binary.
