// wkhtml-rs — drop-in wkhtmltopdf replacement for Odoo, built on fulgur.
mod args;
mod render;

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // Odoo probes `wkhtmltopdf --version` and requires the string to contain
    // "(with patched qt)" plus a version >= 0.12.2 (for dpi zoom ratio).
    // We report 0.12.6 (the final upstream release) with patched qt.
    if argv.first().map(|s| s.as_str()) == Some("--version") {
        println!("{}", args::VERSION);
        return ExitCode::SUCCESS;
    }
    if argv.first().map(|s| s.as_str()) == Some("--help")
        || argv.first().map(|s| s.as_str()) == Some("-h")
    {
        args::print_help();
        return ExitCode::SUCCESS;
    }

    match args::parse(&argv) {
        Ok(a) => {
            let output = a.output.clone();
            match render::run(a) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    // Odoo treats exit 1 as success-with-warning and will still
                    // read the PDF. Real failures must be ≥2, and we drop any
                    // empty/corrupt output so Odoo cannot consume it.
                    if let Some(path) = output {
                        let _ = std::fs::remove_file(&path);
                    }
                    eprintln!("wkhtmltopdf: {e:#}");
                    ExitCode::from(2)
                }
            }
        }
        Err(e) => {
            eprintln!("wkhtmltopdf: {e:#}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_string_constant() {
        assert_eq!(
            crate::args::VERSION,
            "wkhtmltopdf 0.12.6 (with patched qt)"
        );
    }
}
