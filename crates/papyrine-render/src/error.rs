use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// libpdfium could not be found or loaded.
    Library(String),
    /// Password required to open the document.
    PasswordRequired,
    /// A password was supplied but is wrong.
    PasswordIncorrect,
    /// Not a PDF, or too damaged for PDFium to open.
    Format,
    /// Unsupported security handler.
    Security,
    /// File access failed (custom reader returned an error).
    File,
    /// Total byte length exceeds what `FPDF_FILEACCESS` can address on this platform.
    TooLarge,
    PageOutOfRange(usize),
    PageLoad(usize),
    Render(String),
    /// The cancel callback asked to stop; no pixels are returned.
    Cancelled,
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Library(s) => write!(f, "PDFium library: {s}"),
            Error::PasswordRequired => f.write_str("password required"),
            Error::PasswordIncorrect => f.write_str("incorrect password"),
            Error::Format => f.write_str("not a valid PDF"),
            Error::Security => f.write_str("unsupported security handler"),
            Error::File => f.write_str("file access error"),
            Error::TooLarge => {
                f.write_str("document too large for this platform's FPDF_FILEACCESS")
            }
            Error::PageOutOfRange(i) => write!(f, "page {i} out of range"),
            Error::PageLoad(i) => write!(f, "could not load page {i}"),
            Error::Render(s) => write!(f, "render failed: {s}"),
            Error::Cancelled => f.write_str("render cancelled"),
            Error::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
