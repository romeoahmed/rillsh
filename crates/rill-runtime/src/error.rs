//! Language failures and unforgeable host control markers.
use crate::code::Source;
use rill_syntax::token::Span;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct Error {
    pub kind: String,
    pub message: String,
    pub span: Option<Span>,
    pub origin: Option<Rc<Source>>,
    pub exit_status: Option<u8>,
    pub notes: Vec<String>,
    pub details: Box<std::collections::BTreeMap<String, Detail>>,
    control: Option<Control>,
}
/// Structured failure facts; text is descriptive, integers retain their numeric meaning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detail {
    Text(String),
    Int(i64),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Cancel,
    Exit,
}

impl Error {
    #[must_use]
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            span: None,
            origin: None,
            exit_status: None,
            notes: Vec::new(),
            details: Box::default(),
            control: None,
        }
    }
    /// Host cancellation cannot be manufactured by raising a similarly named language error.
    #[must_use]
    pub fn cancelled(message: impl Into<String>) -> Self {
        Self {
            control: Some(Control::Cancel),
            ..Self::new("Interrupted", message)
        }
    }
    pub(crate) fn session_exit() -> Self {
        Self {
            control: Some(Control::Exit),
            ..Self::new("SessionExit", "session exit")
        }
    }
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self.control, Some(Control::Cancel))
    }
    #[must_use]
    pub const fn is_exit(&self) -> bool {
        matches!(self.control, Some(Control::Exit))
    }
    pub(crate) fn type_error(message: impl Into<String>) -> Self {
        Self::new("TypeError", message)
    }
    pub(crate) fn arithmetic() -> Self {
        Self::new(
            "ArithmeticError",
            "numeric result is outside the supported range",
        )
    }
}

impl From<rill_system::resources::SourceError> for Error {
    fn from(error: rill_system::resources::SourceError) -> Self {
        use rill_system::resources::SourceError;
        match error {
            SourceError::Selected { error, .. } => Self::from(*error),
            SourceError::Cleanup { primary, notes } => {
                let mut error = Self::from(*primary);
                error.notes.extend(notes);
                error
            }
            SourceError::Io(error) => Self::from(error),
            SourceError::Process { code, stage } => {
                let mut error = Self::new(
                    "ProcessError",
                    format!("stage {stage} failed with status {code}"),
                );
                error.exit_status = Some(code);
                error.details.insert(
                    "stage".into(),
                    Detail::Int(i64::try_from(stage).unwrap_or(i64::MAX)),
                );
                error
            }
            SourceError::Stopped => Self::new(
                "ProcessError",
                "process source stopped outside resumable evaluation",
            ),
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::from_io(&error)
    }
}
impl Error {
    /// Translate an OS failure without losing Rill's error category.
    #[must_use]
    pub fn from_io(error: &std::io::Error) -> Self {
        let kind = match error.kind() {
            std::io::ErrorKind::FileTooLarge => "LimitExceeded",
            std::io::ErrorKind::InvalidData => "DecodeError",
            std::io::ErrorKind::ResourceBusy => "ResourceBusy",
            _ => "IOError",
        };
        let mut result = Self::new(kind, error.to_string());
        result.details.insert(
            "io_kind".into(),
            Detail::Text(format!("{:?}", error.kind())),
        );
        if let Some(code) = error.raw_os_error() {
            result
                .details
                .insert("os_code".into(), Detail::Int(i64::from(code)));
        }
        result
    }
}
