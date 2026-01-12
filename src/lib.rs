pub mod driver;
pub mod error;
pub mod image;

#[derive(Debug)]
pub struct Settings {
    pub dpi_600: bool,
    pub auto_cut: bool,
    pub dithering: bool,
}
