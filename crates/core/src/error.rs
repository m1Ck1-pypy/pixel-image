//! Ошибки библиотеки: единый тип для файловых операций, декодирования и
//! прикладных проверок.

use std::{error::Error as StdError, fmt, io};

use image::ImageError;

/// Ошибка обработки изображения.
#[derive(Debug)]
pub enum Error {
    /// Не удалось открыть, декодировать или сохранить изображение.
    Image(ImageError),
    /// Ошибка ввода-вывода, не связанная с кодеком.
    Io(io::Error),
    /// Прикладная ошибка с готовым текстом для пользователя.
    Message(String),
}

impl Error {
    /// Создаёт прикладную ошибку с произвольным сообщением.
    pub fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Image(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Image(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Message(_) => None,
        }
    }
}

impl From<ImageError> for Error {
    fn from(error: ImageError) -> Self {
        Self::Image(error)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Результат операций библиотеки.
pub type Result<T> = std::result::Result<T, Error>;
