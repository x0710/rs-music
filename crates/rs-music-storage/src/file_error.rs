use std::{
    fmt, io,
    path::{Path, PathBuf},
};

#[derive(Debug)]
struct FileError {
    operation: &'static str,
    path: PathBuf,
    source: io::Error,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.operation, self.path.display())
    }
}

impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

pub(crate) fn path_error(operation: &'static str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        FileError {
            operation,
            path: path.to_owned(),
            source: error,
        },
    )
}
