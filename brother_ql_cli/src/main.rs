use brother_ql::error::BrotherQlError;
use brother_ql::image::{print_lines, render_image};
use brother_ql::Settings;
use clap::Parser;

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// File name to print
    file: String,
}

fn main() -> Result<(), BrotherQlError> {
    let args = Args::parse();

    let settings = Settings {
        dpi_600: false,
        auto_cut: true,
        dithering: true,
    };
    let lines = render_image(&args.file, &settings)?;
    print_lines(lines, &settings)
}
