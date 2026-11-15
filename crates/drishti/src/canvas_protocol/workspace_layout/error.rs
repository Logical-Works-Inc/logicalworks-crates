//! Failure vocabulary of the workspace layout contract: the machine-readable
//! issue, the validation error that pairs it with a path, and the strict
//! loader's decode-or-invalid error.

use std::fmt;

/// Machine-readable reason a layout failed validation.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::workspace_layout::{WorkspaceLayoutError, WorkspaceLayoutIssue};
///
/// let error = WorkspaceLayoutError::new(WorkspaceLayoutIssue::DuplicateToolId, "tools[].tool_id");
/// assert_eq!(error.issue(), WorkspaceLayoutIssue::DuplicateToolId);
/// assert_ne!(error.issue(), WorkspaceLayoutIssue::InvalidTool);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspaceLayoutIssue {
    /// `schema_version` is not [`super::WORKSPACE_LAYOUT_SCHEMA_VERSION`].
    UnsupportedSchemaVersion,
    /// `layout_id` is not a qualified id.
    InvalidLayoutId,
    /// `title` is empty, too long, or holds control characters.
    InvalidTitle,
    /// `subtitle` is too long or holds control characters.
    InvalidSubtitle,
    /// Chrome placements are out of range, duplicated, or miss the default.
    InvalidChrome,
    /// Surface count is outside 1..=32.
    InvalidSurfaceCount,
    /// Two surfaces share a `widget_id`.
    DuplicateWidgetId,
    /// Two surfaces share a presentation `order`.
    DuplicateSurfaceOrder,
    /// A widget descriptor field is malformed.
    InvalidWidgetDescriptor,
    /// A widget requests a non-presentation capability.
    PrivilegedWidgetCapability,
    /// Presentation metadata is malformed or role/slot mismatched.
    InvalidPresentation,
    /// Tool count is outside 1..=16.
    InvalidToolCount,
    /// Two tools share a `tool_id`.
    DuplicateToolId,
    /// A tool descriptor is malformed.
    InvalidTool,
}

/// A layout validation failure: what went wrong and where.
///
/// Fields are private: [`WorkspaceLayoutError::new`] builds failures and
/// readers go through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::workspace_layout::{WorkspaceLayoutError, WorkspaceLayoutIssue};
///
/// let error = WorkspaceLayoutError::new(WorkspaceLayoutIssue::InvalidTitle, "title");
/// assert_eq!(error.to_string(), "workspace title is invalid at title");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkspaceLayoutError {
    /// Machine-readable reason.
    issue: WorkspaceLayoutIssue,
    /// Layout path the failure was found at.
    at: &'static str,
}

impl WorkspaceLayoutError {
    /// Build the error [`super::WorkspaceLayout::validate`] returns.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::workspace_layout::{WorkspaceLayoutError, WorkspaceLayoutIssue};
    ///
    /// let error = WorkspaceLayoutError::new(WorkspaceLayoutIssue::InvalidChrome, "chrome");
    /// assert_eq!((error.issue(), error.at()), (WorkspaceLayoutIssue::InvalidChrome, "chrome"));
    /// ```
    #[must_use]
    pub const fn new(issue: WorkspaceLayoutIssue, at: &'static str) -> Self {
        Self { issue, at }
    }

    /// Which contract rule failed, for callers that branch on the cause
    /// instead of parsing the message.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::workspace_layout::{WorkspaceLayoutError, WorkspaceLayoutIssue};
    ///
    /// let error = WorkspaceLayoutError::new(WorkspaceLayoutIssue::InvalidTool, "tools[]");
    /// assert_eq!(error.issue(), WorkspaceLayoutIssue::InvalidTool);
    /// ```
    #[must_use]
    pub const fn issue(&self) -> WorkspaceLayoutIssue {
        self.issue
    }

    /// Schema-style path of the offending field, such as `tools[].tool_id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::workspace_layout::{WorkspaceLayoutError, WorkspaceLayoutIssue};
    ///
    /// let error = WorkspaceLayoutError::new(WorkspaceLayoutIssue::InvalidTool, "tools[]");
    /// assert_eq!(error.at(), "tools[]");
    /// ```
    #[must_use]
    pub const fn at(&self) -> &'static str {
        self.at
    }
}

impl fmt::Display for WorkspaceLayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.issue {
            WorkspaceLayoutIssue::UnsupportedSchemaVersion => {
                "unsupported workspace layout schema version"
            }
            WorkspaceLayoutIssue::InvalidLayoutId => "workspace layout_id is invalid",
            WorkspaceLayoutIssue::InvalidTitle => "workspace title is invalid",
            WorkspaceLayoutIssue::InvalidSubtitle => "workspace subtitle is invalid",
            WorkspaceLayoutIssue::InvalidChrome => "workspace chrome contract is invalid",
            WorkspaceLayoutIssue::InvalidSurfaceCount => {
                "workspace surface count is outside 1..=32"
            }
            WorkspaceLayoutIssue::DuplicateWidgetId => "workspace widget_id is duplicated",
            WorkspaceLayoutIssue::DuplicateSurfaceOrder => "workspace surface order is duplicated",
            WorkspaceLayoutIssue::InvalidWidgetDescriptor => {
                "workspace widget descriptor is invalid"
            }
            WorkspaceLayoutIssue::PrivilegedWidgetCapability => {
                "workspace widget requests a non-presentation capability"
            }
            WorkspaceLayoutIssue::InvalidPresentation => {
                "workspace presentation metadata is invalid"
            }
            WorkspaceLayoutIssue::InvalidToolCount => "workspace tool count is outside 1..=16",
            WorkspaceLayoutIssue::DuplicateToolId => "workspace tool_id is duplicated",
            WorkspaceLayoutIssue::InvalidTool => "workspace tool descriptor is invalid",
        };
        write!(formatter, "{message} at {}", self.at)
    }
}

impl std::error::Error for WorkspaceLayoutError {}

/// Strict load failure: undecodable bytes or a validation issue.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::workspace_layout::{load_validated, WorkspaceLoadError};
///
/// let refused = load_validated(b"not json").err();
/// assert!(matches!(refused, Some(WorkspaceLoadError::Decode(_))));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspaceLoadError {
    /// Bytes do not decode: not JSON, wrong types (including bool/int
    /// confusion), unknown fields, or unknown enum variants.
    Decode(String),
    /// Decoded but invalid, with the validation failure attached.
    Invalid(WorkspaceLayoutError),
}

impl core::fmt::Display for WorkspaceLoadError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Decode(ref message) => write!(formatter, "workspace decode failed: {message}"),
            Self::Invalid(ref error) => write!(formatter, "workspace invalid: {error}"),
        }
    }
}

impl std::error::Error for WorkspaceLoadError {}
