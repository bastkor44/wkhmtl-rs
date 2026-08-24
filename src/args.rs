//! Parse the subset of the wkhtmltopdf CLI that Odoo actually emits
//! (see odoo/addons/base/models/ir_actions_report.py::_build_wkhtmltopdf_args),
//! plus the common global flags for robustness. Unknown flags are tolerated
//! (warned on stderr, suppressed by `--quiet`) so future Odoo versions keep working.

use anyhow::Context;
use std::path::PathBuf;

/// Spoofed `--version` line. Odoo’s `_wkhtml()` probe requires
/// `(with patched qt)` and a version ≥ 0.12.2 (for the dpi zoom ratio).
pub const VERSION: &str = "wkhtmltopdf 0.12.6 (with patched qt)";

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
    let num: String = t
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    Ok(num.parse::<f32>()?)
}

/// Split `--flag=value` on the first `=`. Bare tokens are unchanged.
fn split_flag(tok: &str) -> (String, Option<String>) {
    if let Some(rest) = tok.strip_prefix("--") {
        if let Some((name, val)) = rest.split_once('=') {
            return (format!("--{name}"), Some(val.to_string()));
        }
    }
    (tok.to_string(), None)
}

fn is_quiet_flag(tok: &str) -> bool {
    let flag = split_flag(tok).0;
    flag == "--quiet" || flag == "-q"
}

pub fn parse(argv: &[String]) -> anyhow::Result<WkArgs> {
    let extra: Vec<String> = if argv.iter().any(|s| s == "--read-args-from-stdin") {
        let stdin = std::io::read_to_string(std::io::stdin()).context("reading args from stdin")?;
        stdin.split_whitespace().map(str::to_string).collect()
    } else {
        Vec::new()
    };
    let combined: Vec<String> = argv
        .iter()
        .filter(|s| s.as_str() != "--read-args-from-stdin")
        .cloned()
        .chain(extra)
        .collect();
    parse_tokens(&combined)
}

fn parse_tokens(argv: &[String]) -> anyhow::Result<WkArgs> {
    let quiet = argv.iter().any(|s| is_quiet_flag(s));
    let mut a = WkArgs {
        quiet,
        ..WkArgs::default()
    };
    let mut i = 0;
    while i < argv.len() {
        let (flag, inline_val) = split_flag(argv[i].as_str());
        let mut inline_val = inline_val;
        let val = |i: &mut usize, inline_val: &mut Option<String>| -> anyhow::Result<String> {
            if let Some(v) = inline_val.take() {
                return Ok(v);
            }
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing value for {flag}"))
        };
        match flag.as_str() {
            "--page-size" => {
                a.page_size = Some(PageSizeSpec::Named(val(&mut i, &mut inline_val)?.to_uppercase()))
            }
            "--page-width" => {
                let w = parse_f32(&val(&mut i, &mut inline_val)?)?;
                let h = match &a.page_size {
                    Some(PageSizeSpec::CustomMm(_, h)) => *h,
                    _ => 0.0,
                };
                a.page_size = Some(PageSizeSpec::CustomMm(w, h));
            }
            "--page-height" => {
                let h = parse_f32(&val(&mut i, &mut inline_val)?)?;
                let w = match &a.page_size {
                    Some(PageSizeSpec::CustomMm(w, _)) => *w,
                    _ => 0.0,
                };
                a.page_size = Some(PageSizeSpec::CustomMm(w, h));
            }
            "--orientation" => a.orientation = Some(val(&mut i, &mut inline_val)?),
            "--margin-top" => a.margin_top = Some(parse_f32(&val(&mut i, &mut inline_val)?)?),
            "--margin-bottom" => a.margin_bottom = Some(parse_f32(&val(&mut i, &mut inline_val)?)?),
            "--margin-left" => a.margin_left = Some(parse_f32(&val(&mut i, &mut inline_val)?)?),
            "--margin-right" => a.margin_right = Some(parse_f32(&val(&mut i, &mut inline_val)?)?),
            "--dpi" => a.dpi = Some(val(&mut i, &mut inline_val)?.parse()?),
            "--zoom" => a.zoom = Some(val(&mut i, &mut inline_val)?.parse()?),
            "--header-html" => a.header_html = Some(PathBuf::from(val(&mut i, &mut inline_val)?)),
            "--footer-html" => a.footer_html = Some(PathBuf::from(val(&mut i, &mut inline_val)?)),
            "--header-spacing" => {
                a.header_spacing = Some(parse_f32(&val(&mut i, &mut inline_val)?)?)
            }
            "--header-line" => a.header_line = true,
            "--disable-smart-shrinking" => a.disable_smart_shrinking = true,
            "--javascript-delay" => {
                a.javascript_delay = Some(val(&mut i, &mut inline_val)?.parse()?)
            }
            "--viewport-size" => a.viewport_size = Some(val(&mut i, &mut inline_val)?),
            "--cookie-jar" => a.cookie_jar = Some(PathBuf::from(val(&mut i, &mut inline_val)?)),
            "--disable-local-file-access" => a.disable_local_file_access = true,
            "--quiet" | "-q" => a.quiet = true,
            "--title" => a.title = Some(val(&mut i, &mut inline_val)?),
            other if other.starts_with('-') => {
                // Unknown flags are boolean. `--flag=value` is consumed as a
                // pair and ignored; a bare `--flag` never eats the next token.
                if !a.quiet {
                    eprintln!("warning: ignoring unsupported option '{other}'");
                }
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
         \x20 --read-args-from-stdin         append whitespace-separated args from stdin\n\
         \x20 --quiet --title <t>"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    /// Realistic argv from Odoo `ir_actions_report._build_wkhtmltopdf_args`
    /// plus the header/footer/cookie-jar tokens the same method appends later.
    #[test]
    fn parse_odoo_ir_actions_report_vector() {
        let argv = s(&[
            "--disable-local-file-access",
            "--viewport-size",
            "1280x1024",
            "--quiet",
            "--page-size",
            "A4",
            "--margin-top",
            "40",
            "--dpi",
            "90",
            "--zoom",
            "1.0666666666666667",
            "--header-spacing",
            "35",
            "--margin-left",
            "7",
            "--margin-bottom",
            "20",
            "--margin-right",
            "7",
            "--orientation",
            "Portrait",
            "--header-line",
            "--disable-smart-shrinking",
            "--javascript-delay",
            "1000",
            "--cookie-jar",
            "/tmp/cookie.txt",
            "--header-html",
            "/tmp/hdr.html",
            "--footer-html",
            "/tmp/ftr.html",
            "/tmp/body1.html",
            "/tmp/body2.html",
            "/tmp/out.pdf",
        ]);
        let a = parse(&argv).expect("odoo argv should parse");
        assert!(a.disable_local_file_access);
        assert_eq!(a.viewport_size.as_deref(), Some("1280x1024"));
        assert!(a.quiet);
        assert_eq!(a.page_size, Some(PageSizeSpec::Named("A4".into())));
        assert_eq!(a.margin_top, Some(40.0));
        assert_eq!(a.dpi, Some(90));
        assert_eq!(a.zoom, Some(1.0666666666666667));
        assert_eq!(a.header_spacing, Some(35.0));
        assert_eq!(a.margin_left, Some(7.0));
        assert_eq!(a.margin_bottom, Some(20.0));
        assert_eq!(a.margin_right, Some(7.0));
        assert_eq!(a.orientation.as_deref(), Some("Portrait"));
        assert!(a.header_line);
        assert!(a.disable_smart_shrinking);
        assert_eq!(a.javascript_delay, Some(1000));
        assert_eq!(a.cookie_jar, Some(PathBuf::from("/tmp/cookie.txt")));
        assert_eq!(a.header_html, Some(PathBuf::from("/tmp/hdr.html")));
        assert_eq!(a.footer_html, Some(PathBuf::from("/tmp/ftr.html")));
        assert_eq!(
            a.input_files,
            vec![PathBuf::from("/tmp/body1.html"), PathBuf::from("/tmp/body2.html")]
        );
        assert_eq!(a.output, Some(PathBuf::from("/tmp/out.pdf")));
    }

    #[test]
    fn parse_flag_equals_value() {
        let argv = s(&["--page-size=Letter", "--margin-top=18mm", "in.html", "out.pdf"]);
        let a = parse(&argv).unwrap();
        assert_eq!(a.page_size, Some(PageSizeSpec::Named("LETTER".into())));
        assert_eq!(a.margin_top, Some(18.0));
        assert_eq!(a.input_files, vec![PathBuf::from("in.html")]);
        assert_eq!(a.output, Some(PathBuf::from("out.pdf")));
    }

    #[test]
    fn last_positional_is_output() {
        let argv = s(&["a.html", "b.html", "c.html", "merged.pdf"]);
        let a = parse(&argv).unwrap();
        assert_eq!(
            a.input_files,
            vec![
                PathBuf::from("a.html"),
                PathBuf::from("b.html"),
                PathBuf::from("c.html")
            ]
        );
        assert_eq!(a.output, Some(PathBuf::from("merged.pdf")));
    }

    #[test]
    fn unknown_flag_does_not_eat_next_token() {
        let argv = s(&["--foo", "bar.html", "in.html", "out.pdf"]);
        let a = parse(&argv).unwrap();
        assert_eq!(
            a.input_files,
            vec![PathBuf::from("bar.html"), PathBuf::from("in.html")]
        );
        assert_eq!(a.output, Some(PathBuf::from("out.pdf")));
    }

    #[test]
    fn unknown_flag_equals_does_not_eat_next_token() {
        let argv = s(&["--foo=bar", "in.html", "out.pdf"]);
        let a = parse(&argv).unwrap();
        assert_eq!(a.input_files, vec![PathBuf::from("in.html")]);
        assert_eq!(a.output, Some(PathBuf::from("out.pdf")));
    }

    #[test]
    fn missing_output_is_error() {
        let err = parse(&s(&["only.html"])).unwrap_err();
        assert!(err.to_string().contains("output"));
    }

    #[test]
    fn missing_input_is_error() {
        // Two identical positionals still yield one input + one output after pop.
        // Zero positionals is the empty-input path after the output check.
        let err = parse(&s(&[])).unwrap_err();
        assert!(err.to_string().contains("output") || err.to_string().contains("input"));
    }

    #[test]
    fn missing_flag_value_is_error() {
        let err = parse(&s(&["--page-size"])).unwrap_err();
        assert!(err.to_string().contains("missing value"));
    }

    #[test]
    fn page_width_height_mm_suffix() {
        let argv = s(&[
            "--page-width",
            "210mm",
            "--page-height",
            "297mm",
            "in.html",
            "out.pdf",
        ]);
        let a = parse(&argv).unwrap();
        assert_eq!(a.page_size, Some(PageSizeSpec::CustomMm(210.0, 297.0)));
    }

    #[test]
    fn version_string_matches_odoo_probe() {
        assert!(VERSION.contains("wkhtmltopdf 0.12.6"));
        assert!(VERSION.contains("(with patched qt)"));
    }
}
