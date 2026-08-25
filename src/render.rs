//! Rendering pipeline: convert each input HTML body with fulgur, stamp
//! wkhtml-style header/footer HTML onto every page, then merge all bodies
//! into one PDF (Odoo passes one body file per record and expects a single
//! merged PDF whose top-level outline entries delimit each record).

use crate::args::{PageSizeSpec, WkArgs};
use crate::datauri::DataUriCache;
use anyhow::Context;
use fulgur::config::{Margin, PageSize};
use fulgur::engine::{Engine, EngineBuilder};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::BTreeMap;
use std::path::Path;

/// Odoo `PAPER_SIZES` (report_paperformat.py) plus Qt extras A10 / A3+.
/// Values are millimetres (width, height).
pub(crate) const PAPER_SIZES_MM: &[(&str, f32, f32)] = &[
    ("A0", 841.0, 1189.0),
    ("A1", 594.0, 841.0),
    ("A2", 420.0, 594.0),
    ("A3", 297.0, 420.0),
    ("A4", 210.0, 297.0),
    ("A5", 148.0, 210.0),
    ("A6", 105.0, 148.0),
    ("A7", 74.0, 105.0),
    ("A8", 52.0, 74.0),
    ("A9", 37.0, 52.0),
    ("A10", 26.0, 37.0),
    ("A3+", 329.0, 483.0),
    ("B0", 1000.0, 1414.0),
    ("B1", 707.0, 1000.0),
    ("B2", 500.0, 707.0),
    ("B3", 353.0, 500.0),
    ("B4", 250.0, 353.0),
    ("B5", 176.0, 250.0),
    ("B6", 125.0, 176.0),
    ("B7", 88.0, 125.0),
    ("B8", 62.0, 88.0),
    ("B9", 33.0, 62.0),
    ("B10", 31.0, 44.0),
    ("C5E", 163.0, 229.0),
    ("COMM10E", 105.0, 241.0),
    ("DLE", 110.0, 220.0),
    ("EXECUTIVE", 190.5, 254.0),
    ("FOLIO", 210.0, 330.0),
    ("LEDGER", 431.8, 279.4),
    ("LEGAL", 215.9, 355.6),
    ("LETTER", 215.9, 279.4),
    ("TABLOID", 279.4, 431.8),
];

pub(crate) fn paper_size_mm(name: &str) -> Option<(f32, f32)> {
    PAPER_SIZES_MM
        .iter()
        .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, w, h)| (*w, *h))
}

fn page_size(a: &WkArgs) -> PageSize {
    let mut size = match &a.page_size {
        Some(PageSizeSpec::Named(name)) => named_size(name, a.quiet),
        Some(PageSizeSpec::CustomMm(w, h)) if *w > 0.0 && *h > 0.0 => PageSize::custom(*w, *h),
        _ => PageSize::A4,
    };
    let landscape = a
        .orientation
        .as_deref()
        .map(|o| o.eq_ignore_ascii_case("landscape"))
        .unwrap_or(false);
    if landscape {
        size = size.landscape();
    }
    size
}

fn named_size(name: &str, quiet: bool) -> PageSize {
    if let Some((w, h)) = paper_size_mm(name) {
        return PageSize::custom(w, h);
    }
    if !quiet {
        eprintln!("warning: unknown page size '{name}', using A4");
    }
    PageSize::custom(210.0, 297.0)
}

fn margin_pt(a: &WkArgs, which: &str) -> f32 {
    // wkhtmltopdf margins are millimetres.
    let mm = match which {
        "top" => a.margin_top,
        "bottom" => a.margin_bottom,
        "left" => a.margin_left,
        "right" => a.margin_right,
        _ => None,
    };
    mm.unwrap_or(10.0) * 72.0 / 25.4
}

fn mm_to_pt(mm: f32) -> f32 {
    mm * 72.0 / 25.4
}

/// Wrap a header/footer HTML fragment into a full document. Odoo’s
/// `--header-html` / `--footer-html` are already complete `web.minimal_layout`
/// documents; nesting those inside another `<html>` confuses CSS, so they
/// pass through verbatim.
pub(crate) fn wrap_fragment(html: &str, title: Option<&str>) -> String {
    if html.to_ascii_lowercase().contains("<html") {
        return html.to_string();
    }
    format!(
        "<!doctype html><html><head><meta charset='utf-8'>{}<style>\
         @page {{ margin: 0 }} html,body {{ margin:0; padding:0 }}\
         </style></head><body>{}</body></html>",
        title
            .map(|t| format!("<title>{t}</title>"))
            .unwrap_or_default(),
        html
    )
}

/// Apply `--zoom` as a CSS hint on the body HTML.
///
/// fulgur may ignore CSS `zoom`; this matches Odoo’s patched-Qt
/// `--zoom 96.0/dpi` flag so layouts that honour it stay at the intended scale.
fn inject_zoom(html: &str, zoom: Option<f64>) -> String {
    let Some(z) = zoom else {
        return html.to_string();
    };
    format!("<style>html {{ zoom: {z} }}</style>{html}")
}

fn engine_builder(a: &WkArgs, size: PageSize, margin: Margin) -> EngineBuilder {
    let mut builder = Engine::builder()
        .page_size(size)
        .margin(margin)
        .bookmarks(true);
    if let Some(t) = &a.title {
        builder = builder.title(t.clone());
    }
    builder
}

fn maybe_base_path(mut builder: EngineBuilder, a: &WkArgs, path: &Path) -> EngineBuilder {
    if !a.disable_local_file_access {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                builder = builder.base_path(parent);
            }
        }
    }
    builder
}

pub fn run(a: WkArgs) -> anyhow::Result<()> {
    let output = a.output.clone().context("missing output path")?;
    let size = page_size(&a);
    let margin = Margin {
        top: margin_pt(&a, "top"),
        bottom: margin_pt(&a, "bottom"),
        left: margin_pt(&a, "left"),
        right: margin_pt(&a, "right"),
    };

    // Header/footer fragments: laid out on a band-sized page (margin-top /
    // margin-bottom high) and stamped into that band on every body page.
    let header_pdf = a
        .header_html
        .as_ref()
        .map(|p| render_fragment(&a, p, size.width, margin.top.max(1.0)))
        .transpose()?;
    let footer_pdf = a
        .footer_html
        .as_ref()
        .map(|p| render_fragment(&a, p, size.width, margin.bottom.max(1.0)))
        .transpose()?;

    // Render each body file separately so per-record outlines stay intact,
    // then concatenate.
    let mut data_cache = DataUriCache::new()?;
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for path in &a.input_files {
        let html = std::fs::read_to_string(path)
            .with_context(|| format!("reading input {}", path.display()))?;
        let html = data_cache.rewrite(&html);
        let html = inject_zoom(&html, a.zoom);
        let engine = maybe_base_path(engine_builder(&a, size, margin), &a, path)
            .assets(data_cache.clone_bundle())
            .build();
        let pdf = engine
            .render(&html)
            .with_context(|| format!("rendering {}", path.display()))?;
        parts.push(pdf);
    }

    let merged = merge(parts)?;

    let stamped = if header_pdf.is_some() || footer_pdf.is_some() || a.header_line {
        stamp(&merged, header_pdf.as_deref(), footer_pdf.as_deref(), &a)?
    } else {
        merged
    };

    std::fs::write(&output, stamped)
        .with_context(|| format!("writing output {}", output.display()))
}

fn render_fragment(
    a: &WkArgs,
    path: &Path,
    page_width: f32,
    band_height: f32,
) -> anyhow::Result<Vec<u8>> {
    let html =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let frag = wrap_fragment(&html, None);
    let mut data_cache = DataUriCache::new()?;
    let frag = data_cache.rewrite(&frag);
    let size = PageSize {
        width: page_width,
        height: band_height.max(1.0),
    };
    let engine = maybe_base_path(
        Engine::builder()
            .page_size(size)
            .margin(Margin::uniform(0.0))
            .assets(data_cache.clone_bundle()),
        a,
        path,
    )
    .build();
    Ok(engine.render(&frag)?)
}

fn save_doc(mut doc: Document) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::new();
    doc.save_to(&mut buf)
        .map_err(|e| anyhow::anyhow!("writing PDF: {e}"))?;
    Ok(buf)
}

fn trailer_catalog_id(document: &Document) -> anyhow::Result<ObjectId> {
    match document.trailer.get(b"Root") {
        Ok(Object::Reference(id)) => Ok(*id),
        Ok(_) => anyhow::bail!("catalog Root is not a reference"),
        Err(e) => anyhow::bail!("missing catalog Root: {e}"),
    }
}

/// Concatenate PDFs while keeping a top-level outline entry per part (lopdf
/// has no `Document::merge`; this follows the library's merge example and
/// rebuilds bookmarks so Odoo can split the result per record).
pub(crate) fn merge(parts: Vec<Vec<u8>>) -> anyhow::Result<Vec<u8>> {
    if parts.len() <= 1 {
        return Ok(parts.into_iter().next().unwrap_or_default());
    }

    let mut max_id = 1u32;
    let mut documents_pages: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut documents_objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut page_order: Vec<ObjectId> = Vec::new();
    // (title, 0-based page index of this part's first page in the merged doc)
    let mut first_pages: Vec<(String, u32)> = Vec::new();

    for (idx, bytes) in parts.iter().enumerate() {
        let mut doc =
            Document::load_mem(bytes).with_context(|| format!("parsing PDF part {}", idx + 1))?;
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;

        let pages = doc.get_pages();
        let first_index = page_order.len() as u32;
        let mut saw_page = false;
        // `get_pages()` is keyed by page number, so values are already in
        // document order. Collect into a Vec — do NOT later iterate a
        // BTreeMap keyed by ObjectId (that would reorder by id).
        for object_id in pages.into_values() {
            if !saw_page {
                first_pages.push((format!("Document {}", idx + 1), first_index));
                saw_page = true;
            }
            page_order.push(object_id);
            if let Ok(obj) = doc.get_object(object_id) {
                documents_pages.insert(object_id, obj.to_owned());
            }
        }
        documents_objects.extend(doc.objects);
    }

    let mut document = Document::with_version("1.5");
    let mut catalog_object: Option<(ObjectId, Object)> = None;
    let mut pages_object: Option<(ObjectId, Object)> = None;

    for (object_id, object) in documents_objects {
        match object.type_name().unwrap_or(b"") {
            b"Catalog" => {
                catalog_object = Some((
                    catalog_object
                        .as_ref()
                        .map(|(id, _)| *id)
                        .unwrap_or(object_id),
                    object,
                ));
            }
            b"Pages" => {
                if let Ok(dictionary) = object.as_dict() {
                    let mut dictionary = dictionary.clone();
                    if let Some((_, ref object)) = pages_object {
                        if let Ok(old_dictionary) = object.as_dict() {
                            dictionary.extend(old_dictionary);
                        }
                    }
                    pages_object = Some((
                        pages_object
                            .as_ref()
                            .map(|(id, _)| *id)
                            .unwrap_or(object_id),
                        Object::Dictionary(dictionary),
                    ));
                }
            }
            b"Page" | b"Outlines" | b"Outline" => {}
            _ => {
                document.objects.insert(object_id, object);
            }
        }
    }

    let (pages_id, pages_obj) = pages_object.context("Pages root not found in merged PDF")?;
    let (catalog_id, catalog_obj) =
        catalog_object.context("Catalog root not found in merged PDF")?;

    for (object_id, object) in &documents_pages {
        if let Ok(dictionary) = object.as_dict() {
            let mut dictionary = dictionary.clone();
            dictionary.set("Parent", pages_id);
            document
                .objects
                .insert(*object_id, Object::Dictionary(dictionary));
        }
    }

    if let Ok(dictionary) = pages_obj.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Count", page_order.len() as u32);
        dictionary.set(
            "Kids",
            page_order
                .iter()
                .copied()
                .map(Object::Reference)
                .collect::<Vec<_>>(),
        );
        document
            .objects
            .insert(pages_id, Object::Dictionary(dictionary));
    }

    if let Ok(dictionary) = catalog_obj.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Pages", pages_id);
        dictionary.set("PageMode", "UseOutlines");
        dictionary.remove(b"Outlines");
        dictionary.remove(b"Dests");
        document
            .objects
            .insert(catalog_id, Object::Dictionary(dictionary));
    }

    document.trailer.set("Root", catalog_id);
    document.max_id = document.objects.keys().map(|id| id.0).max().unwrap_or(0);

    document.renumber_objects();
    document.adjust_zero_pages();
    attach_odoo_outlines(&mut document, &first_pages)?;

    save_doc(document)
}

/// wkhtml-style named destinations so Odoo’s `_split_pdf` can walk
/// `root['/Outlines']['/First']` → `node['/Dest']` → `root['/Dests'][dest][0]`.
///
/// Qt writes `/Dests` values as `[pageIndex /XYZ …]` with a **0-based page
/// number**, not a page object reference. Odoo then does
/// `outlines_pages[0] == 0` and `range(num, to)` / `getPage(j)` on those
/// integers (`ir_actions_report.py`).
fn attach_odoo_outlines(
    document: &mut Document,
    first_pages: &[(String, u32)],
) -> anyhow::Result<()> {
    if first_pages.is_empty() {
        return Ok(());
    }

    let catalog_id = trailer_catalog_id(document)?;
    let outlines_id = document.new_object_id();
    let item_ids: Vec<ObjectId> = (0..first_pages.len())
        .map(|_| document.new_object_id())
        .collect();

    let mut dests = Dictionary::new();
    for (i, (title, page_idx)) in first_pages.iter().enumerate() {
        let name = format!("Doc{}", i + 1);
        dests.set(
            name.as_str(),
            Object::Array(vec![
                Object::Integer(i64::from(*page_idx)),
                Object::Name(b"Fit".to_vec()),
            ]),
        );

        let mut item = Dictionary::new();
        item.set("Title", Object::string_literal(title.as_str()));
        item.set("Parent", Object::Reference(outlines_id));
        item.set("Count", 0);
        item.set("Dest", Object::Name(name.into_bytes()));
        if i > 0 {
            item.set("Prev", Object::Reference(item_ids[i - 1]));
        }
        if i + 1 < item_ids.len() {
            item.set("Next", Object::Reference(item_ids[i + 1]));
        }
        document
            .objects
            .insert(item_ids[i], Object::Dictionary(item));
    }

    let mut outlines = Dictionary::new();
    outlines.set("Type", Object::Name(b"Outlines".to_vec()));
    outlines.set("First", Object::Reference(item_ids[0]));
    outlines.set("Last", Object::Reference(*item_ids.last().unwrap()));
    outlines.set("Count", first_pages.len() as i64);
    document
        .objects
        .insert(outlines_id, Object::Dictionary(outlines));

    // Re-resolve in case anything above shifted ids (it shouldn't — we only
    // inserted new objects — but never reuse a pre-renumber catalog id).
    let catalog_id = trailer_catalog_id(document).unwrap_or(catalog_id);
    match document.get_object_mut(catalog_id) {
        Ok(Object::Dictionary(dict)) => {
            dict.set("Outlines", Object::Reference(outlines_id));
            dict.set("Dests", Object::Dictionary(dests));
            dict.set("PageMode", "UseOutlines");
        }
        _ => anyhow::bail!("catalog object {catalog_id:?} is not a dictionary"),
    }
    Ok(())
}

/// Stamp header/footer band Form XObjects onto every page of `base`, clipped
/// to the top/bottom margin band. Optionally stroke `--header-line`.
fn stamp(
    base: &[u8],
    header: Option<&[u8]>,
    footer: Option<&[u8]>,
    a: &WkArgs,
) -> anyhow::Result<Vec<u8>> {
    let mut doc = Document::load_mem(base).context("parsing base PDF")?;
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let header_band = margin_pt(a, "top").max(1.0);
    let footer_band = margin_pt(a, "bottom").max(1.0);

    if let Some(bytes) = header {
        let frag = Document::load_mem(bytes).context("parsing header PDF")?;
        let form_id = import_page_as_form(&mut doc, &frag).context("importing header form")?;
        overlay_form(&mut doc, &pages, form_id, b"Hdr", BandKind::Header, header_band)?;
    }
    if let Some(bytes) = footer {
        let frag = Document::load_mem(bytes).context("parsing footer PDF")?;
        let form_id = import_page_as_form(&mut doc, &frag).context("importing footer form")?;
        overlay_form(&mut doc, &pages, form_id, b"Ftr", BandKind::Footer, footer_band)?;
    }
    if a.header_line {
        stroke_header_line(&mut doc, &pages, a)?;
    }

    save_doc(doc)
}

enum BandKind {
    Header,
    Footer,
}

fn import_page_as_form(dest: &mut Document, src: &Document) -> anyhow::Result<ObjectId> {
    let mut src = src.clone();
    src.renumber_objects_with(dest.max_id + 1);
    dest.max_id = src.max_id;

    let page_id = src
        .get_pages()
        .into_values()
        .next()
        .context("empty fragment PDF")?;
    let page = src.get_dictionary(page_id)?.clone();
    let media_box = page
        .get(b"MediaBox")
        .cloned()
        .unwrap_or_else(|_| Object::Array(vec![0.into(), 0.into(), 595.into(), 842.into()]));
    let resources = page.get(b"Resources").ok().cloned();
    let content = src.get_page_content(page_id);

    for (id, obj) in src.objects {
        match obj.type_name().unwrap_or(b"") {
            b"Catalog" | b"Pages" | b"Page" | b"Outlines" | b"Outline" => {}
            _ => {
                dest.objects.insert(id, obj);
            }
        }
    }

    let mut form_dict = Dictionary::new();
    form_dict.set("Type", Object::Name(b"XObject".to_vec()));
    form_dict.set("Subtype", Object::Name(b"Form".to_vec()));
    form_dict.set("BBox", media_box);
    if let Some(res) = resources {
        form_dict.set("Resources", res);
    }
    let form = Stream::new(form_dict, content);
    Ok(dest.add_object(form))
}

fn pdf_f(n: f32) -> String {
    format!("{n:.4}")
}

fn object_f32(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// Page width/height in pt from MediaBox. Origin is assumed at (x0, y0).
fn page_media_wh(doc: &Document, page_id: ObjectId) -> (f32, f32) {
    let default = (595.0, 842.0);
    let Ok(page) = doc.get_dictionary(page_id) else {
        return default;
    };
    let mb = match page.get(b"MediaBox") {
        Ok(Object::Array(a)) => a.clone(),
        Ok(Object::Reference(id)) => match doc.get_object(*id) {
            Ok(Object::Array(a)) => a.clone(),
            _ => return default,
        },
        _ => return default,
    };
    if mb.len() < 4 {
        return default;
    }
    let x0 = object_f32(&mb[0]).unwrap_or(0.0);
    let y0 = object_f32(&mb[1]).unwrap_or(0.0);
    let x1 = object_f32(&mb[2]).unwrap_or(595.0);
    let y1 = object_f32(&mb[3]).unwrap_or(842.0);
    ((x1 - x0).abs(), (y1 - y0).abs())
}

fn overlay_form(
    doc: &mut Document,
    pages: &[ObjectId],
    form_id: ObjectId,
    name: &[u8],
    kind: BandKind,
    band_h: f32,
) -> anyhow::Result<()> {
    let name_str = std::str::from_utf8(name).unwrap_or("X");
    for &page_id in pages {
        let (page_w, page_h) = page_media_wh(doc, page_id);
        // Header: y-translate to the top band. `--header-spacing` is parsed
        // (Odoo uses it so the header fits inside margin-top) but placement
        // stays simple: ty = page_height − margin_top_pt. Footer sits at the
        // bottom-left origin.
        let (clip_y, ty) = match kind {
            BandKind::Header => {
                let y = page_h - band_h;
                (y, y)
            }
            BandKind::Footer => (0.0, 0.0),
        };
        let ops = format!(
            "q {x} {y} {w} {h} re W n 1 0 0 1 0 {ty} cm /{name} Do Q\n",
            x = pdf_f(0.0),
            y = pdf_f(clip_y),
            w = pdf_f(page_w),
            h = pdf_f(band_h),
            ty = pdf_f(ty),
            name = name_str,
        );
        doc.add_xobject(page_id, name.to_vec(), form_id)
            .map_err(|e| anyhow::anyhow!("adding form xobject: {e}"))?;
        doc.add_page_contents(page_id, ops.into_bytes())
            .map_err(|e| anyhow::anyhow!("appending overlay content: {e}"))?;
    }
    Ok(())
}

fn stroke_header_line(doc: &mut Document, pages: &[ObjectId], a: &WkArgs) -> anyhow::Result<()> {
    let margin_left = margin_pt(a, "left");
    let margin_right = margin_pt(a, "right");
    let margin_top = margin_pt(a, "top");
    let spacing = a.header_spacing.map(mm_to_pt).unwrap_or(0.0);
    for &page_id in pages {
        let (page_w, page_h) = page_media_wh(doc, page_id);
        let y = page_h - margin_top + spacing;
        let x1 = margin_left;
        let x2 = page_w - margin_right;
        let ops = format!(
            "q 0.5 w 0 G {x1} {y} m {x2} {y2} l S Q\n",
            x1 = pdf_f(x1),
            y = pdf_f(y),
            x2 = pdf_f(x2),
            y2 = pdf_f(y),
        );
        doc.add_page_contents(page_id, ops.into_bytes())
            .map_err(|e| anyhow::anyhow!("appending header-line: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_pdf(label: &str) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut font = Dictionary::new();
        font.set("Type", "Font");
        font.set("Subtype", "Type1");
        font.set("BaseFont", "Helvetica");
        let font_id = doc.add_object(Object::Dictionary(font));
        let content_id = doc.add_object(Stream::new(
            Dictionary::new(),
            format!("BT /F1 24 Tf 72 700 Td ({label}) Tj ET").into_bytes(),
        ));
        let mut font_res = Dictionary::new();
        font_res.set("F1", Object::Reference(font_id));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(font_res));
        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Parent", Object::Reference(pages_id));
        page.set(
            "MediaBox",
            vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(595),
                Object::Integer(842),
            ],
        );
        page.set("Contents", Object::Reference(content_id));
        page.set("Resources", Object::Dictionary(resources));
        let page_id = doc.add_object(Object::Dictionary(page));
        let mut pages = Dictionary::new();
        pages.set("Type", "Pages");
        pages.set("Count", 1);
        pages.set("Kids", vec![Object::Reference(page_id)]);
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set("Type", "Catalog");
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", catalog_id);
        doc.max_id = doc.objects.keys().map(|id| id.0).max().unwrap_or(0);
        save_doc(doc).expect("minimal pdf")
    }

    fn dests_dict<'a>(doc: &'a Document, catalog: &'a Dictionary) -> Dictionary {
        match catalog.get(b"Dests").expect("Dests on catalog") {
            Object::Dictionary(d) => d.clone(),
            Object::Reference(id) => doc
                .get_dictionary(*id)
                .expect("Dests ref")
                .clone(),
            other => panic!("Dests is {other:?}"),
        }
    }

    #[test]
    fn paper_sizes_match_odoo_qt_table() {
        assert_eq!(paper_size_mm("A4"), Some((210.0, 297.0)));
        assert_eq!(paper_size_mm("a4"), Some((210.0, 297.0)));
        assert_eq!(paper_size_mm("Letter"), Some((215.9, 279.4)));
        assert_eq!(paper_size_mm("LETTER"), Some((215.9, 279.4)));
        assert_eq!(paper_size_mm("Executive"), Some((190.5, 254.0)));
        assert_eq!(paper_size_mm("Legal"), Some((215.9, 355.6)));
        assert_eq!(paper_size_mm("Tabloid"), Some((279.4, 431.8)));
        assert_eq!(paper_size_mm("A3+"), Some((329.0, 483.0)));
        assert_eq!(paper_size_mm("B10"), Some((31.0, 44.0)));
        assert!(paper_size_mm("not-a-size").is_none());
    }

    #[test]
    fn wrap_fragment_passthrough_complete_html() {
        let full = "<!DOCTYPE html><html><head></head><body>hdr</body></html>";
        assert_eq!(wrap_fragment(full, None), full);
        let upper = "<HTML><BODY>x</BODY></HTML>";
        assert_eq!(wrap_fragment(upper, None), upper);
        let frag = "<div>hello</div>";
        let wrapped = wrap_fragment(frag, Some("t"));
        assert!(wrapped.contains("<html>"));
        assert!(wrapped.contains(frag));
        assert!(wrapped.contains("<title>t</title>"));
    }

    #[test]
    fn merge_outlines_match_odoo_splitter() {
        let merged = merge(vec![minimal_pdf("one"), minimal_pdf("two")]).unwrap();
        let doc = Document::load_mem(&merged).unwrap();
        assert!(
            doc.get_pages().len() >= 2,
            "merged document should have ≥2 pages"
        );

        let catalog_id = trailer_catalog_id(&doc).unwrap();
        let catalog = doc.get_dictionary(catalog_id).unwrap();
        let outlines_ref = catalog
            .get(b"Outlines")
            .expect("catalog /Outlines")
            .as_reference()
            .unwrap();
        let outlines = doc.get_dictionary(outlines_ref).unwrap();
        let first_id = outlines
            .get(b"First")
            .expect("Outlines /First")
            .as_reference()
            .unwrap();
        let first = doc.get_dictionary(first_id).unwrap();
        let dest_name = first.get(b"Dest").expect("item /Dest").as_name().unwrap();
        let dests = dests_dict(&doc, catalog);
        let dest_arr = dests
            .get(dest_name)
            .unwrap_or_else(|_| panic!("Dests missing name {}", String::from_utf8_lossy(dest_name)))
            .as_array()
            .unwrap();
        assert!(
            !dest_arr.is_empty(),
            "dest array should be [page /Fit]"
        );
        match &dest_arr[0] {
            Object::Integer(0) => {}
            Object::Reference(id) => {
                let first_page = *doc.get_pages().values().next().unwrap();
                assert_eq!(
                    *id, first_page,
                    "first dest page ref must be document page 0"
                );
            }
            other => panic!("first dest page should be 0, got {other:?}"),
        }

        // Chain /Next across both parts.
        assert!(first.get(b"Next").is_ok(), "first outline should have /Next");
        assert_eq!(
            first.get(b"Title").unwrap().as_str().unwrap(),
            b"Document 1"
        );
        assert_eq!(first.get(b"Count").unwrap().as_i64().unwrap(), 0);
    }

    #[test]
    fn merge_kids_preserve_input_order() {
        let merged = merge(vec![minimal_pdf("AAA"), minimal_pdf("BBB")]).unwrap();
        let doc = Document::load_mem(&merged).unwrap();
        let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
        assert_eq!(pages.len(), 2);
        let c0 = doc.get_page_content(pages[0]);
        let c1 = doc.get_page_content(pages[1]);
        let s0 = String::from_utf8_lossy(&c0);
        let s1 = String::from_utf8_lossy(&c1);
        assert!(s0.contains("AAA"), "first page should be first input: {s0}");
        assert!(s1.contains("BBB"), "second page should be second input: {s1}");
    }
}
