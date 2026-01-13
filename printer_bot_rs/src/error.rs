use brother_ql::error::BrotherQlError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PrinterBotError {
    #[error("io error")]
    Io(#[from] std::io::Error),
    #[error("teloxide error")]
    Teloxide(#[from] teloxide_core::RequestError),
    #[error("file download error")]
    Download(#[from] teloxide_core::DownloadError),
    #[error("brother ql printer error")]
    PrinterError(#[from] BrotherQlError),
}
