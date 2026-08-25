//! `data:` URI extraction for Odoo reports.
//!
//! Odoo's report layouts embed company logos / print headers as
//! `<img src="data:image/png;base64,...">` (via `image_data_uri()`). fulgur
//! loads `<img>` bytes from its `AssetBundle`, not from URLs, so we decode
//! each data: URI and register it as a bundle asset, rewriting the src to
//! the synthetic asset name.

use base64::Engine as _;

/// Decodes data: URIs and hands the images to an `AssetBundle`.
pub struct DataUriCache {
    images: Vec<(String, Vec<u8>)>,
    counter: u64,
    last_dims: Option<(u32, u32)>,
}

impl DataUriCache {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            images: Vec::new(),
            counter: 0,
            last_dims: None,
        })
    }

    /// Register extracted images on an `AssetBundle`.
    pub fn into_asset_bundle(self) -> fulgur::asset::AssetBundle {
        self.clone_bundle()
    }

    /// Non-consuming variant — same bundle, keeps the cache usable.
    pub(crate) fn clone_bundle(&self) -> fulgur::asset::AssetBundle {
        let mut bundle = fulgur::asset::AssetBundle::new();
        for (name, data) in &self.images {
            bundle.add_image(name.clone(), data.clone());
        }
        bundle
    }    /// Rewrite with a default viewport width (A4 content area).
    pub fn rewrite(&mut self, html: &str) -> String {
        self.rewrite_with_viewport(html, 794.0)
    }

    /// Rewrite every `src="data:...;base64,..."` in `html` to a synthetic
    /// asset name. fulgur gives `<img>` with auto height a zero-height box,
    /// so when the tag has a width but no height we inject a style height
    /// derived from the intrinsic aspect ratio (`viewport_width_px` is the
    /// page content width used to resolve percentage widths).
    pub fn rewrite_with_viewport(&mut self, html: &str, viewport_width_px: f32) -> String {
        let re = regex::Regex::new(
            r#"(?i)(<img\b[^>]*?\bsrc\s*=\s*)("data:([^;"]+);base64,([^"]*)"|'data:([^;']+);base64,([^']*)')([^>]*>)"#,
        ).expect("static regex");
        re.replace_all(html, |caps: &regex::Captures| {
            let (mime, payload) = if let Some(m) = caps.get(3) {
                (
                    m.as_str().to_string(),
                    caps.get(4).map(|p| p.as_str()).unwrap_or(""),
                )
            } else {
                (
                    caps.get(5).map(|m| m.as_str()).unwrap_or("").to_string(),
                    caps.get(6).map(|p| p.as_str()).unwrap_or(""),
                )
            };
            let Some(name) = self.write_payload(&mime, payload) else {
                return caps.get(0).map(|m| m.as_str().to_string()).unwrap_or_default();
            };
            let pre = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let post = caps.get(7).map(|m| m.as_str()).unwrap_or("");
            let dims = self.last_dims.unwrap_or((0, 0));
            let lower_pre = pre.to_ascii_lowercase();
            let lower_post = post.to_ascii_lowercase();
            let has_height = lower_post
                .split(';')
                .any(|d| {
                    let d = d.trim();
                    d.starts_with("height") && !d.starts_with("max-height") && !d.starts_with("min-height")
                })
                || lower_pre.contains("height=");
            let needs_dims = !has_height && dims.0 > 0 && dims.1 > 0;
            if !needs_dims {
                return format!("<img src=\"{name}\"{}", post);
            }
            // Determine the effective width in px: style/attr px value, else
            // a percentage of the viewport.
            let width_px = lower_post
                .split(';')
                .find_map(|d| d.trim().strip_prefix("width:"))
                .and_then(|v| v.trim().trim_end_matches("px").trim().parse::<f32>().ok())
                .or_else(|| {
                    caps.get(0)
                        .map(|m| m.as_str())
                        .and_then(|tag| parse_attr_px(tag, "width"))
                })
                .unwrap_or_else(|| {
                    let pct = lower_post
                        .split(';')
                        .find_map(|d| d.trim().strip_prefix("width:"))
                        .and_then(|v| v.trim().trim_end_matches('%').trim().parse::<f32>().ok())
                        .or_else(|| caps.get(0).map(|m| m.as_str()).and_then(|tag| parse_attr_pct(tag, "width")))
                        .unwrap_or(100.0);
                    viewport_width_px * pct / 100.0
                });
            let height_px = (width_px * dims.1 as f32 / dims.0 as f32).round().max(1.0);
            format!(
                "<img src=\"{name}\"{}",
                inject_style_height(post, height_px)
            )
        })
        .to_string()
    }

    fn write_payload(&mut self, mime: &str, payload: &str) -> Option<String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .ok()?;
        let ext = mime.rsplit('/').next()?.split(';').next()?;
        let ext = match ext {
            "jpeg" => "jpg",
            "svg" => "svg",
            "png" => "png",
            "gif" => "gif",
            "webp" => "webp",
            _ => return None, // don't touch non-image data URIs
        };
        let dims = sniff_dims(&bytes, mime);
        self.counter += 1;
        let name = format!("wkrs-img-{}.{ext}", self.counter);
        self.last_dims = dims;
        self.images.push((name.clone(), bytes));
        Some(name)
    }
}

/// Parse a `width="123"` / `width="123px"` attribute value from a tag string.
fn parse_attr_px(tag: &str, attr: &str) -> Option<f32> {
    let re = regex::Regex::new(&format!(r#"(?i){attr}\s*=\s*["']?(\d+(?:\.\d+)?)px?["']?"#)).ok()?;
    re.captures(tag)?.get(1)?.as_str().parse().ok()
}

/// Parse a `width="50%"` attribute percentage.
fn parse_attr_pct(tag: &str, attr: &str) -> Option<f32> {
    let re = regex::Regex::new(&format!(r#"(?i){attr}\s*=\s*["']?(\d+(?:\.\d+)?)%["']?"#)).ok()?;
    re.captures(tag)?.get(1)?.as_str().parse().ok()
}

/// Append `height: Npx` to the tag's style attribute (creating one if absent).
fn inject_style_height(post: &str, height_px: f32) -> String {
    let lower = post.to_ascii_lowercase();
    if let Some(style_pos) = lower.find("style=") {
        // find the quote char after style=
        let after = &post[style_pos + 6..];
        if let Some(q) = after.chars().next() {
            if q == '"' || q == '\'' {
                if let Some(end_rel) = after[1..].find(q) {
                    let inner = &after[1..1 + end_rel];
                    let mut style = inner.to_string();
                    if !style.trim_end().ends_with(';') {
                        style.push(';');
                    }
                    style.push_str(&format!("height:{height_px}px"));
                    let start = style_pos + 6 + 1;
                    let end = start + end_rel;
                    return format!("{}{}{}", &post[..start], style, &post[end..]);
                }
            }
        }
    }
    format!("{post} style=\"height:{height_px}px\"")
}

/// Best-effort intrinsic pixel dimensions from PNG (IHDR) / JPEG (SOF0-15) headers.
fn sniff_dims(bytes: &[u8], mime: &str) -> Option<(u32, u32)> {
    if mime.contains("png") && bytes.len() > 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((w, h));
    }
    if mime.contains("jpeg") || mime.contains("jpg") {
        // walk JPEG markers
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF { i += 1; continue; }
            let marker = bytes[i + 1];
            // SOF0..SOF15 except DHT (C4), JPG (C8), DAC (CC)
            if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
                let h = u16::from_be_bytes(bytes[i + 5..i + 7].try_into().ok()?) as u32;
                let w = u16::from_be_bytes(bytes[i + 7..i + 9].try_into().ok()?) as u32;
                return Some((w, h));
            }
            let len = u16::from_be_bytes(bytes[i + 2..i + 4].try_into().ok()?) as usize;
            i += 2 + len;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_png() -> Vec<u8> {
        fn chunk(t: &[u8], d: &[u8]) -> Vec<u8> {
            let mut c = t.to_vec();
            c.extend_from_slice(d);
            let mut out = (d.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(&c);
            out.extend_from_slice(&crc32(&c));
            out
        }
        fn crc32(c: &[u8]) -> Vec<u8> {
            let table: Vec<u32> = (0..256)
                .map(|i| {
                    let mut c = i as u32;
                    for _ in 0..8 {
                        c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
                    }
                    c
                })
                .collect();
            let mut crc = 0xffff_ffffu32;
            for &b in c {
                crc = table[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
            }
            (crc ^ 0xffff_ffff).to_be_bytes().to_vec()
        }
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&8u32.to_be_bytes());
        ihdr.extend_from_slice(&8u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend_from_slice(&chunk(b"IHDR", &ihdr));
        let mut raw = Vec::new();
        for _ in 0..8 {
            raw.push(0u8);
            raw.extend_from_slice(&vec![[0xffu8, 0, 0]; 8].into_iter().flatten().collect::<Vec<u8>>());
        }
        // zlib stream: stored (uncompressed) deflate block + adler32
        let mut z = vec![0x78, 0x01];
        z.push(0x01);
        z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
        z.extend_from_slice(&raw);
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for &byte in &raw {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        png.extend_from_slice(&chunk(b"IDAT", &z));
        png.extend_from_slice(&chunk(b"IEND", &[]));
        png
    }

    fn red_png_b64() -> String {
        base64::engine::general_purpose::STANDARD.encode(test_png())
    }

    #[test]
    fn rewrites_double_quoted_data_uri() {
        let mut cache = DataUriCache::new().unwrap();
        let html = format!(
            "<img src=\"data:image/png;base64,{}\" style=\"width:10px\">",
            red_png_b64()
        );
        let out = cache.rewrite(&html);
        assert!(out.starts_with("<img src=\"wkrs-img-1.png\""), "got: {out}");
        let bundle = cache.into_asset_bundle();
        assert!(bundle.get_image("wkrs-img-1.png").is_some());
    }

    #[test]
    fn rewrites_single_quoted_data_uri() {
        let mut cache = DataUriCache::new().unwrap();
        let html = format!("<img src='data:image/jpeg;base64,{}'>", red_png_b64());
        let out = cache.rewrite(&html);
        assert!(out.contains("wkrs-img-1.jpg"), "got: {out}");
        assert!(cache.into_asset_bundle().get_image("wkrs-img-1.jpg").is_some());
    }

    #[test]
    fn leaves_non_data_src_alone() {
        let mut cache = DataUriCache::new().unwrap();
        let html = "<img src=\"https://x/logo.png\">";
        assert_eq!(cache.rewrite(html), html);
    }

    #[test]
    fn leaves_invalid_base64_alone() {
        let mut cache = DataUriCache::new().unwrap();
        let html = "<img src=\"data:image/png;base64,!!!notbase64!!!\">";
        assert_eq!(cache.rewrite(html), html);
    }

    #[test]
    fn debug_dims_injection() {
        let mut cache = DataUriCache::new().unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(test_png());
        let html = format!("<img src=\"data:image/png;base64,{b64}\" style=\"width:100%; display:block\"/>");
        let out = cache.rewrite(&html);
        println!("OUT: {out}");
    }

    #[test]
    fn multiple_images_rewritten() {
        let mut cache = DataUriCache::new().unwrap();
        let b64 = red_png_b64();
        let html = format!("<img src=\"data:image/png;base64,{b64}\"><img src=\"data:image/png;base64,{b64}\">");
        let out = cache.rewrite(&html);
        assert_eq!(out.matches("src=\"wkrs-img-").count(), 2);
        let bundle = cache.into_asset_bundle();
        assert!(bundle.get_image("wkrs-img-1.png").is_some());
        assert!(bundle.get_image("wkrs-img-2.png").is_some());
    }
}
