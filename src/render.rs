//! Rendering pipeline: convert each input HTML body with fulgur, stamp
//! wkhtml-style header/footer HTML onto every page, then merge all bodies
//! into one PDF (Odoo passes one body file per record and expects a single
//! merged PDF whose top-level outline entries delimit each record).

use crate::args::{PageSizeSpec, WkArgs};
use anyhow::Context;
use fulgur::config::{Margin, PageSize};
use fulgur::engine::{Engine, EngineBuilder};
use lopdf::{Bookmark, Dictionary, Document, Object, ObjectId, Stream};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

fn page_size(a: &WkArgs) -> PageSize {
    let mut size = match &a.page_size {
        Some(PageSizeSpec::Named(name)) => named_size(name),
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

/// Common ISO 216 / US sizes in mm; anything else falls back to A4.
fn named_size(name: &str) -> PageSize {
    const SIZES: &[(&str, f32, f32)] = &[
        ("A0", 841.0, 1189.0),
        ("A1", 594.0, 841.0),
        ("A2", 420.0, 594.0),
        ("A3", 297.0, 420.0),
        ("A4", 210.0, 297.0),
        ("A5", 148.0, 210.0),
        ("A6", 105.0, 148.0),
        ("B4", 250.0, 353.0),
        ("B5", 176.0, 250.0),
        ("LETTER", 215.9, 279.4),
        ("LEGAL", 215.9, 355.6),
        ("TABLOID", 279.4, 431.8),
        ("EXECUTIVE", 184.15, 266.7),
    ];
    for (n, w, h) in SIZES {
        if name.eq_ignore_ascii_case(n) {
            return PageSize::custom(*w, *h);
        }
    }
    eprintln!("warning: unknown page size '{name}', using A4");
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

/// Wrap a header/footer HTML fragment into a full document sized to the
/// top/bottom margin band, so fulgur paginates it onto exactly one page that
/// we can overlay. Simplest robust approach: render it as its own single-page
/// PDF at the same page size with zero margins, then stamp the drawn content
/// at an offset onto each body page.
fn wrap_fragment(html: &str, title: Option<&str>) -> String {
    format!(
        "<!doctype html><html><head><meta charset='utf-8'>{}<style>\
         @page {{ margin: 0 }} html,body {{ margin:0; padding:0 }}\
         </style></head><body>{}</body></html>",
        title.map(|t| format!("<title>{t}</title>")).unwrap_or_default(),
        html
    )
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

    // Header/footer fragments: rendered once each into single-page PDFs and
    // stamped on every page of the merged document.
    let header_pdf = a
        .header_html
        .as_ref()
        .map(|p| render_fragment(&a, p, size))
        .transpose()?;
    let footer_pdf = a
        .footer_html
        .as_ref()
        .map(|p| render_fragment(&a, p, size))
        .transpose()?;

    // Render each body file separately so per-record outlines stay intact,
    // then concatenate. fulgur generates bookmarks from h1-h6, giving Odoo's
    // `_split_pdf_from_reports` its per-record top-level outlines.
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for path in &a.input_files {
        let html = std::fs::read_to_string(path)
            .with_context(|| format!("reading input {}", path.display()))?;
        let engine = maybe_base_path(engine_builder(&a, size, margin), &a, path).build();
        let pdf = engine
            .render(&html)
            .with_context(|| format!("rendering {}", path.display()))?;
        parts.push(pdf);
    }

    let merged = merge(parts)?;

    let stamped = if header_pdf.is_some() || footer_pdf.is_some() {
        stamp(&merged, header_pdf.as_deref(), footer_pdf.as_deref())?
    } else {
        merged
    };

    std::fs::write(&output, stamped)
        .with_context(|| format!("writing output {}", output.display()))
}

fn render_fragment(a: &WkArgs, path: &Path, size: PageSize) -> anyhow::Result<Vec<u8>> {
    let html =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let frag = wrap_fragment(&html, None);
    let engine = maybe_base_path(
        Engine::builder()
            .page_size(size)
            .margin(Margin::uniform(0.0)),
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

/// Concatenate PDFs while keeping a top-level outline entry per part (lopdf
/// has no `Document::merge`; this follows the library's merge example and
/// rebuilds bookmarks so Odoo can split the result per record).
fn merge(parts: Vec<Vec<u8>>) -> anyhow::Result<Vec<u8>> {
    if parts.len() <= 1 {
        return Ok(parts.into_iter().next().unwrap_or_default());
    }

    let mut max_id = 1u32;
    let mut documents_pages: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut documents_objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut first_pages: Vec<(String, ObjectId)> = Vec::new();

    for (idx, bytes) in parts.iter().enumerate() {
        let mut doc = Document::load_mem(bytes)
            .with_context(|| format!("parsing PDF part {}", idx + 1))?;
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;

        let pages = doc.get_pages();
        let mut first_object = None;
        for object_id in pages.into_values() {
            if first_object.is_none() {
                first_object = Some(object_id);
            }
            if let Ok(obj) = doc.get_object(object_id) {
                documents_pages.insert(object_id, obj.to_owned());
            }
        }
        if let Some(object_id) = first_object {
            first_pages.push((format!("Document {}", idx + 1), object_id));
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
                    catalog_object.as_ref().map(|(id, _)| *id).unwrap_or(object_id),
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
                        pages_object.as_ref().map(|(id, _)| *id).unwrap_or(object_id),
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
        dictionary.set("Count", documents_pages.len() as u32);
        dictionary.set(
            "Kids",
            documents_pages
                .into_keys()
                .map(Object::Reference)
                .collect::<Vec<_>>(),
        );
        document.objects.insert(pages_id, Object::Dictionary(dictionary));
    }

    if let Ok(dictionary) = catalog_obj.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Pages", pages_id);
        dictionary.set("PageMode", "UseOutlines");
        dictionary.remove(b"Outlines");
        document
            .objects
            .insert(catalog_id, Object::Dictionary(dictionary));
    }

    document.trailer.set("Root", catalog_id);
    document.max_id = document.objects.keys().map(|id| id.0).max().unwrap_or(0);

    for (title, page) in first_pages {
        document.add_bookmark(Bookmark::new(title, [0.0, 0.0, 0.0], 0, page), None);
    }

    document.renumber_objects();
    document.adjust_zero_pages();
    if let Some(outline_id) = document.build_outline() {
        if let Ok(Object::Dictionary(dict)) = document.get_object_mut(catalog_id) {
            dict.set("Outlines", Object::Reference(outline_id));
        }
    }

    save_doc(document)
}

/// Stamp header/footer content pages onto every page of `base` as Form
/// XObjects. Fragments were laid out on zero-margin pages matching the body
/// page size, so a 1:1 overlay is safe.
fn stamp(base: &[u8], header: Option<&[u8]>, footer: Option<&[u8]>) -> anyhow::Result<Vec<u8>> {
    let mut doc = Document::load_mem(base).context("parsing base PDF")?;
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();

    if let Some(bytes) = header {
        let frag = Document::load_mem(bytes).context("parsing header PDF")?;
        let form_id = import_page_as_form(&mut doc, &frag).context("importing header form")?;
        overlay_form(&mut doc, &pages, form_id, b"Hdr")?;
    }
    if let Some(bytes) = footer {
        let frag = Document::load_mem(bytes).context("parsing footer PDF")?;
        let form_id = import_page_as_form(&mut doc, &frag).context("importing footer form")?;
        overlay_form(&mut doc, &pages, form_id, b"Ftr")?;
    }

    save_doc(doc)
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

fn overlay_form(
    doc: &mut Document,
    pages: &[ObjectId],
    form_id: ObjectId,
    name: &[u8],
) -> anyhow::Result<()> {
    let mut ops = Vec::new();
    ops.write_all(b"q\n/")?;
    ops.write_all(name)?;
    ops.write_all(b" Do\nQ\n")?;
    for &page_id in pages {
        doc.add_xobject(page_id, name.to_vec(), form_id)
            .map_err(|e| anyhow::anyhow!("adding form xobject: {e}"))?;
        doc.add_page_contents(page_id, ops.clone())
            .map_err(|e| anyhow::anyhow!("appending overlay content: {e}"))?;
    }
    Ok(())
}
