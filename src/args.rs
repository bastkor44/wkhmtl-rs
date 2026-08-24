//! Parse the subset of the wkhtmltopdf CLI that Odoo actually emits
//! (see odoo/addons/base/models/ir_actions_report.py::_build_wkhtmltopdf_args),
//! plus the common global flags for robustness. Unknown flags are tolerated
//! (warned on stderr) so future Odoo versions keep working.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum PageSizeSpec {
    /// Named format (A4, Letter, ...). Stored uppercase.
    Named(String),
    /// Custom width/height in millimetres.
    CustomMm(f32, f32),
}

#[derive(Debug, Default)]
pub struct WkArgs {
    pub input_files: Vec<PathBuf>,
    pub output: Option<PathBuf>,

    pub page_size: Option<PageSizeSpec>,
    pub orientation: Option<String>, // "Landscape" | "Portrait" (case-insensitive)
    pub margin_top: Option<f32>,
    pub margin_right: Option<f32>,
    pub margin_bottom: Option<f32>,
    pub margin_left: Option<f32>,
    /// wkhtmltopdf margins are in mm; --dpi affects zoom only in Odoo's usage.
    pub dpi: Option<u32>,
    pub zoom: Option<f64>,
    pub header_html: Option<PathBuf>,
    pub footer_html: Option<PathBuf>,
    pub header_spacing: Option<f32>, // mm
    pub header_line: bool,
    pub disable_smart_shrinking: bool,
    pub javascript_delay: Option<u64>, // ms; honoured as render settle hint
    pub viewport_size: Option<String>,
    pub cookie_jar: Option<PathBuf>, // parsed but unused (no network fetching)
    pub disable_local_file_access: bool,
    pub quiet: bool,

    pub title: Option<String>,
}

fn parse_f32(s: &str) -> anyhow::Result<f32> {
    let t = s.trim();
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    Ok(num.parse::<f32>()?)
}

pub fn parse(argv: &[String]) -> anyhow::Result<WkArgs> {
    let mut a = WkArgs::default();
    let mut i = 0;
    while i < argv.len() {
        let flag = argv[i].as_str();
        let val = |i: &mut usize| -> anyhow::Result<String> {
            *i += 1;
            argv.get(*i).cloned()
                .ok_or_else(|| anyhow::anyhow!("missing value for {flag}"))
        };
        match flag {
            "--page-size" => a.page_size = Some(PageSizeSpec::Named(val(&mut i)?.to_uppercase())),
            "--page-width" => {
                let w = parse_f32(&val(&mut i)?)?;
                let h = match &a.page_size {
                    Some(PageSizeSpec::CustomMm(_, h)) => *h,
                    _ => 0.0,
                };
                a.page_size = Some(PageSizeSpec::CustomMm(w, h));
            }
            "--page-height" => {
                let h = parse_f32(&val(&mut i)?)?;
                let w = match &a.page_size {
                    Some(PageSizeSpec::CustomMm(w, _)) => *w,
                    _ => 0.0,
                };
                a.page_size = Some(PageSizeSpec::CustomMm(w, h));
            }
            "--orientation" => a.orientation = Some(val(&mut i)?),
            "--margin-top" => a.margin_top = Some(parse_f32(&val(&mut i)?)?),
            "--margin-bottom" => a.margin_bottom = Some(parse_f32(&val(&mut i)?)?),
            "--margin-left" => a.margin_left = Some(parse_f32(&val(&mut i)?)?),
            "--margin-right" => a.margin_right = Some(parse_f32(&val(&mut i)?)?),
            "--dpi" => a.dpi = Some(val(&mut i)?.parse()?),
            "--zoom" => a.zoom = Some(val(&mut i)?.parse()?),
            "--header-html" => a.header_html = Some(PathBuf::from(val(&mut i)?)),
            "--footer-html" => a.footer_html = Some(PathBuf::from(val(&mut i)?)),
            "--header-spacing" => a.header_spacing = Some(parse_f32(&val(&mut i)?)?),
            "--header-line" => a.header_line = true,
            "--disable-smart-shrinking" => a.disable_smart_shrinking = true,
            "--javascript-delay" => a.javascript_delay = Some(val(&mut i)?.parse()?),
            "--viewport-size" => a.viewport_size = Some(val(&mut i)?),
            "--cookie-jar" => a.cookie_jar = Some(PathBuf::from(val(&mut i)?)),
            "--disable-local-file-access" => a.disable_local_file_access = true,
            "--quiet" | "-q" => a.quiet = true,
            "--title" => a.title = Some(val(&mut i)?),
            "--read-args-from-stdin" => unreachable!(),
            other if other.starts_with('-') => {
                // Tolerate unknown flags (with or without a value is ambiguous;
                // wkhtmltopdf's known value-taking flags are covered above, so
                // treat unknown ones as boolean and warn).
                eprintln!("warning: ignoring unsupported option '{other}'");
            }
            other => a.input_files.push(PathBuf::from(other)),
        }
        i += 1;
    }

    // Last non-flag argument = output path (wkhtmltopdf convention).
    if a.input_files.len() >= 2 {
        a.output = a.input_files.pop();
    } else if a.output.is_none() {
        return Err(anyhow::anyhow!("expected an output file path"));
    }
    if a.input_files.is_empty() {
        return Err(anyhow::anyhow!("expected at least one input HTML file"));
    }
    Ok(a)
}

pub fn print_help() {
    println!(
        "wkhtml-rs: drop-in wkhtmltopdf replacement (pure Rust, fulgur engine)\n\
         Usage: wkhtmltopdf [options] <input.html...> <output.pdf>\n\
         \n\
         Supported options (Odoo dialect):\n\
         \x20 --version                     print version string and exit\n\
         \x20 --page-size <name>            A4, LETTER, A3, ...\n\
         \x20 --page-width/--page-height <n>[mm]\n\
         \x20 --orientation <landscape|portrait>\n\
         \x20 --margin-top/bottom/left/right <n>[mm]\n\
         \x20 --dpi <n> --zoom <f>\n\
         \x20 --header-html <file> --footer-html <file>\n\
         \x20 --header-spacing <mm> --header-line\n\
         \x20 --disable-smart-shrinking --disable-local-file-access\n\
         \x20 --javascript-delay <ms> --viewport-size WxH --cookie-jar <file>\n\
         \x20 --quiet --title <t>"
    );
}
