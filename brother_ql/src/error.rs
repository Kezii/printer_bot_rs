use thiserror::Error;

#[derive(Error, Debug)]
pub enum BrotherQlError {
    #[error("io error")]
    Io(#[from] std::io::Error),
    #[error("image error")]
    Image(#[from] image::ImageError),
    #[error("invalid image")]
    InvalidImage,
}
