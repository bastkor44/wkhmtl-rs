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
        println!("wkhtmltopdf 0.12.6 (with patched qt)");
        return ExitCode::SUCCESS;
    }
    if argv.first().map(|s| s.as_str()) == Some("--help")
        || argv.first().map(|s| s.as_str()) == Some("-h")
    {
        args::print_help();
        return ExitCode::SUCCESS;
    }

    match args::parse(&argv).and_then(render::run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wkhtmltopdf: {e:#}");
            ExitCode::from(1)
        }
    }
}
