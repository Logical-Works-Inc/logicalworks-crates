//! What every `lgwks` subcommand shares: refusals, verdicts, file reading and
//! the one script lookup.
//!
//! The refusal shape is one type on purpose: `check`, `expand` and `run`
//! report the same `location`, `kind`, `message` and `suggestion`, as text for
//! humans and as the same JSON object for agents.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The exit code for a refusal: the invocation was understood and the file
/// answered, but the answer is a refusal rather than a result.
pub(crate) const EXIT_REFUSED: u8 = 2;

/// The usage line each refusal points at.
const USAGE_HINT: &str = "see `lgwks --help`";

/// Why one file did not become a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefusalKind {
    /// A word the script vocabulary does not hold, near one it does.
    UnknownWord,
    /// The file is past one of the admission ceilings.
    BoundExceeded,
    /// The file is not a readable one-file app.
    Malformed,
}

impl RefusalKind {
    /// The stable name this refusal carries in a verdict and in `--json`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::UnknownWord => "unknown-word",
            Self::BoundExceeded => "bound-exceeded",
            Self::Malformed => "malformed",
        }
    }
}

/// One refusal, located where the file earned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// `line:col`, 1-based, or `1:1` for the file as a whole.
    location: String,
    /// Which arm fired.
    kind: RefusalKind,
    /// What it names, and what to write instead.
    message: String,
    /// The vocabulary word a misspelling meant, when one is near.
    suggestion: Option<String>,
}

impl Refusal {
    /// One refusal at `location` of `kind`, naming `message`.
    pub(crate) fn at(
        location: String,
        kind: RefusalKind,
        message: String,
        suggestion: Option<String>,
    ) -> Self {
        Self {
            location,
            kind,
            message,
            suggestion,
        }
    }

    /// A malformed-file refusal of the file as a whole, naming `message`.
    pub(crate) fn whole(message: String) -> Self {
        Self::at(String::from("1:1"), RefusalKind::Malformed, message, None)
    }

    /// A ceiling the file as a whole is past, naming `message`.
    pub(crate) fn bound(message: String) -> Self {
        Self::at(
            String::from("1:1"),
            RefusalKind::BoundExceeded,
            message,
            None,
        )
    }

    /// Where the file earned this refusal.
    pub(crate) fn location(&self) -> &str {
        self.location.as_str()
    }

    /// Which arm fired.
    pub(crate) const fn kind(&self) -> RefusalKind {
        self.kind
    }

    /// What it names, and what to write instead.
    pub(crate) fn message(&self) -> &str {
        self.message.as_str()
    }

    /// The vocabulary word a misspelling meant, when one is near.
    pub(crate) fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_deref()
    }

    /// This refusal as a JSON value, with the stable `check --json` keys.
    pub(crate) fn to_json(&self) -> lgwks_std::json::Value {
        let suggestion = match self.suggestion.clone() {
            Some(word) => lgwks_std::json::Value::String(word),
            None => lgwks_std::json::Value::Null,
        };
        lgwks_std::json::json!({
            "location": self.location,
            "kind": self.kind.label(),
            "message": self.message,
            "suggestion": suggestion,
        })
    }
}

/// Hand the refusal back as the error.
///
/// The `Result` is returned directly, as `refuse_at` in the script reader
/// is: the verdict is logged once where it is reported, in [`report`].
pub(crate) fn refuse<T>(refusal: Refusal) -> Result<T, Refusal> {
    Err(refusal)
}

/// The verdict over a file's refusals: exit code and both renderings.
///
/// Human output names each refusal on its own stderr line; `--json` prints
/// one object on stdout with the same facts. A refused verdict exits 2.
pub(crate) fn report(
    ok: bool,
    refusals: &[Refusal],
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    for refusal in refusals {
        lgwks_std::trace::debug!(
            kind = refusal.kind().label(),
            location = refusal.location(),
            message = refusal.message(),
            "lgwks: the file is refused"
        );
    }
    if json {
        let values: Vec<lgwks_std::json::Value> = refusals.iter().map(Refusal::to_json).collect();
        let rendered = lgwks_std::json::json!({
            "ok": ok,
            "refusals": values,
        });
        json_line(&rendered, out)?;
    } else if ok {
        writeln!(out, "OK")?;
    } else {
        for refusal in refusals {
            match refusal.suggestion() {
                Some(word) => writeln!(
                    err,
                    "{}: {}: {} (did you mean `{word}`?)",
                    refusal.location(),
                    refusal.kind().label(),
                    refusal.message(),
                )?,
                None => writeln!(
                    err,
                    "{}: {}: {}",
                    refusal.location(),
                    refusal.kind().label(),
                    refusal.message(),
                )?,
            }
        }
        writeln!(err, "{USAGE_HINT}")?;
    }
    if ok {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(EXIT_REFUSED))
    }
}

/// The verdict over one refusal.
pub(crate) fn report_one(
    refusal: Refusal,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    report(false, &[refusal], json, out, err)
}

/// The verdict over one whole-file refusal naming `message`.
pub(crate) fn fail(
    message: String,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    report_one(Refusal::whole(message), json, out, err)
}

/// Print `value` as one JSON line on `out`; a value that cannot render is an
/// I/O error naming why.
pub(crate) fn json_line(
    value: &lgwks_std::json::Value,
    out: &mut impl Write,
) -> std::io::Result<()> {
    match lgwks_std::json::to_string(value) {
        Ok(line) => writeln!(out, "{line}"),
        Err(error) => {
            lgwks_std::trace::debug!(error = %error, "lgwks: the JSON line could not be rendered");
            Err(std::io::Error::other(format!(
                "the JSON line could not be rendered: {error}"
            )))
        }
    }
}

/// The one positional argument of `command` (a `what`, such as a path or a
/// name), or the exit code the invocation already earned: `--help` printed
/// `usage`, or an unknown option or a second positional was refused with it.
pub(crate) fn one_argument<'a>(
    command: &str,
    usage: &str,
    what: &str,
    args: &'a [String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<Result<Option<&'a str>, ExitCode>> {
    let mut named: Option<&'a str> = None;
    for argument in args {
        if argument.as_str() == "--help" || argument.as_str() == "-h" {
            return write!(out, "{usage}").map(|()| Err(ExitCode::SUCCESS));
        }
        let refused = if argument.starts_with("--") {
            Some(format!("unknown option {argument}"))
        } else if named.is_some() {
            Some(format!("one {what}, and {argument} is a second {what}"))
        } else {
            None
        };
        if let Some(why) = refused {
            lgwks_std::trace::debug!(
                command,
                why = why.as_str(),
                "lgwks: refusing the invocation"
            );
            writeln!(err, "lgwks {command}: {why}")?;
            return write!(err, "{usage}").map(|()| Err(ExitCode::from(EXIT_REFUSED)));
        }
        named = Some(argument.as_str());
    }
    Ok(Ok(named))
}

/// The app file a `[file] [--json]` command's `args` name, read, or the exit
/// code the invocation already earned: usage, a refused invocation, or the
/// verdict over a file that could not be read.
pub(crate) fn open_app(
    command: &str,
    usage: &str,
    args: &[String],
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<Result<AppSource, ExitCode>> {
    let named = match one_argument(command, usage, "path", args, out, err)? {
        Ok(named) => named,
        Err(code) => return Ok(Err(code)),
    };
    match read_app(named) {
        Ok(source) => Ok(Ok(source)),
        Err(error) => fail(error.to_string(), json, out, err).map(Err),
    }
}

/// Turns a command's write result into the process exit code.
///
/// `BrokenPipe` is success: the reader closed the pipe on purpose, which is
/// what `| head -1` does. Any other write fault is a real failure.
pub(crate) fn settle(result: std::io::Result<ExitCode>) -> ExitCode {
    match result {
        Ok(code) => code,
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(EXIT_REFUSED),
    }
}

/// The command summary printed by `--help` and by a refused invocation.
pub(crate) const USAGE: &str = "\
lgwks: one-file apps and the Rust they compile to

USAGE
  lgwks new <name> [--json]     scaffold <name>/Cargo.toml and <name>/app.rs
  lgwks check [file] [--json]   parse and admit the file without building it
  lgwks expand [file] [--json]  print the Rust the file's script! compiles to
  lgwks run [file] [--json]     build and serve the file, printing build times
  lgwks legacy <codec> ...      read and write EBCDIC, COMP-3, copybooks, OLE
                                compound files, VBA, Access and fax TIFF

Every command takes --json: the same facts as JSON for agents.
Exit 0 on success, 2 on a refusal or a refused invocation.
";

/// Why the app file could not be read.
#[derive(Debug)]
pub(crate) enum ReadError {
    /// No file was named and none sits in the working directory.
    Missing {
        /// What was looked for.
        wanted: String,
    },
    /// The path names a directory with no app file in it.
    NoAppInDir {
        /// The directory that holds no app file.
        dir: String,
    },
    /// The file could not be read.
    Unreadable {
        /// The file that could not be read.
        file: String,
        /// What the operating system said.
        cause: String,
    },
    /// The file is not UTF-8 text.
    NotText {
        /// The file that is not text.
        file: String,
    },
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Missing { ref wanted } => {
                write!(formatter, "no app file: {wanted}")
            }
            Self::NoAppInDir { ref dir } => {
                write!(formatter, "the directory {dir} holds no app.rs")
            }
            Self::Unreadable {
                ref file,
                ref cause,
            } => {
                write!(formatter, "the file {file} could not be read: {cause}")
            }
            Self::NotText { ref file } => {
                write!(formatter, "the file {file} is not UTF-8 text")
            }
        }
    }
}

impl std::error::Error for ReadError {}

/// An app file on disk: where it lives and what it holds.
#[derive(Debug, Clone)]
pub(crate) struct AppSource {
    /// The directory the app builds in.
    dir: PathBuf,
    /// The app file itself.
    file: PathBuf,
    /// The file's text.
    text: String,
}

impl AppSource {
    /// The directory the app builds in.
    pub(crate) fn dir(&self) -> &Path {
        self.dir.as_path()
    }

    /// The app file itself.
    pub(crate) fn file(&self) -> &Path {
        self.file.as_path()
    }

    /// The file's text.
    pub(crate) fn text(&self) -> &str {
        self.text.as_str()
    }
}

/// Read the app file `named` names: the path itself, its `app.rs` when it is
/// a directory, or `app.rs` in the working directory when nothing is named.
pub(crate) fn read_app(named: Option<&str>) -> Result<AppSource, ReadError> {
    let file = match named {
        Some(path) => {
            let candidate = PathBuf::from(path);
            if candidate.is_dir() {
                candidate.join("app.rs")
            } else {
                candidate
            }
        }
        None => PathBuf::from("app.rs"),
    };
    if !file.is_file() {
        let wanted = file.display().to_string();
        let refusal = match named {
            Some(path) if Path::new(path).is_dir() => ReadError::NoAppInDir {
                dir: String::from(path),
            },
            _ => ReadError::Missing { wanted },
        };
        lgwks_std::trace::debug!(error = ?refusal, "lgwks: the app file is missing");
        return Err(refusal);
    }
    let text = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) => {
            let refusal = ReadError::Unreadable {
                file: file.display().to_string(),
                cause: error.to_string(),
            };
            lgwks_std::trace::debug!(error = ?refusal, "lgwks: the app file is unreadable");
            return Err(refusal);
        }
    };
    let text = match String::from_utf8(text) {
        Ok(text) => text,
        Err(_) => {
            let refusal = ReadError::NotText {
                file: file.display().to_string(),
            };
            lgwks_std::trace::debug!(error = ?refusal, "lgwks: the app file is not text");
            return Err(refusal);
        }
    };
    let dir = match file.parent() {
        Some(parent) if parent.as_os_str().is_empty() => PathBuf::from("."),
        Some(parent) => parent.to_path_buf(),
        None => PathBuf::from("."),
    };
    Ok(AppSource { dir, file, text })
}

/// The closest a misspelling may be to still earn a suggestion.
const SUGGEST_WITHIN: usize = 2;

/// The distance between `left` and `right`: insertions, deletions and
/// substitutions, each costing one.
///
/// Bounded work: the vocabulary is thirty short words and the input is one
/// source line, so the table stays small however long the file is.
fn distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0_usize..right.len().saturating_add(1)).collect();
    for (row, want) in left.iter().enumerate() {
        let mut current = vec![row.saturating_add(1)];
        for (column, got) in right.iter().enumerate() {
            let cost = if want == got { 0 } else { 1 };
            let deletion = match previous.get(column.saturating_add(1)) {
                Some(value) => value.saturating_add(1),
                None => usize::MAX,
            };
            let insertion = match current.get(column) {
                Some(value) => value.saturating_add(1),
                None => usize::MAX,
            };
            let substitution = match previous.get(column) {
                Some(value) => value.saturating_add(cost),
                None => usize::MAX,
            };
            current.push(deletion.min(insertion).min(substitution));
        }
        previous = current;
    }
    match previous.get(right.len()) {
        Some(value) => *value,
        None => usize::MAX,
    }
}

/// The vocabulary word `word` meant, when one is near.
///
/// The search runs over the script vocabulary's own spellings, so a
/// suggestion is always a word the language actually reads.
fn suggest(word: &str) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for entry in lgwks_ast::script::LEXICON.iter() {
        let spelling = entry.kind().spelling();
        let edited = distance(word, spelling);
        let nearer = match best {
            Some((held, _)) => edited < held,
            None => true,
        };
        if nearer {
            best = Some((edited, spelling));
        }
    }
    match best {
        Some((edited, spelling)) if edited <= SUGGEST_WITHIN => Some(spelling.to_owned()),
        Some(_) | None => None,
    }
}

/// Whether `word` is a word of the script vocabulary.
fn is_word(word: &str) -> bool {
    lgwks_ast::script::LEXICON
        .iter()
        .any(|entry| entry.kind().spelling() == word)
}
/// The first word of `line`: letters, digits and `_`, lowercased, after any
/// leading blank space. The refusal's own line decides the suggestion, so a
/// detached child never earns a word for the line it left.
fn first_word(line: &str) -> String {
    let mut word = String::new();
    for cell in line.chars() {
        if cell.is_alphanumeric() || cell == '_' {
            word.push(cell.to_ascii_lowercase());
        } else if !word.is_empty() {
            break;
        }
    }
    word
}

/// Read the one `script!` block `text` holds, through the one parser.
///
/// A refusal at an unknown word earns its suggestion here, so every caller
/// reports the same `did you mean`.
pub(crate) fn load_script(text: &str) -> Result<lgwks_ast::script::Script, Refusal> {
    let rows: Vec<&str> = text.lines().collect();
    let found = match lgwks_ast::script::read_source(text) {
        Ok(found) => found,
        Err(refusal) => {
            return refuse(Refusal::at(
                format!("{}:{}", refusal.line(), refusal.column().saturating_add(1)),
                RefusalKind::Malformed,
                refusal.message().to_owned(),
                None,
            ));
        }
    };
    if found.is_empty() {
        return refuse(Refusal::at(
            String::from("1:1"),
            RefusalKind::Malformed,
            String::from("this file holds no `script!` block to run"),
            None,
        ));
    }
    if found.len() > 1 {
        let mut ordered = found.iter();
        let _ = ordered.next();
        let second = match ordered.next() {
            Some(second) => second,
            None => {
                return refuse(Refusal::at(
                    String::from("1:1"),
                    RefusalKind::Malformed,
                    String::from("a one-file app holds one `script!` block"),
                    None,
                ));
            }
        };
        return refuse(Refusal::at(
            format!("{}:{}", second.line(), second.column().saturating_add(1)),
            RefusalKind::Malformed,
            String::from("a one-file app holds one `script!` block; this is the second"),
            None,
        ));
    }
    let only = match found.first() {
        Some(only) => only,
        None => {
            return refuse(Refusal::at(
                String::from("1:1"),
                RefusalKind::Malformed,
                String::from("this file holds no `script!` block to run"),
                None,
            ));
        }
    };
    match only.read() {
        Ok(script) => Ok(script.clone()),
        Err(refusal) => {
            let line = refusal.line();
            let column = refusal.column().saturating_add(1);
            let candidate = match rows.get(line.saturating_sub(1)) {
                Some(row) => first_word(row),
                None => String::new(),
            };
            // A line that opens with a vocabulary word is already spelled
            // right; its refusal is structural (a detached block, a missing
            // ceiling), and a did-you-mean for the word it holds would blame
            // the one thing the author got right.
            let suggestion = if is_word(candidate.as_str()) {
                None
            } else {
                suggest(candidate.as_str())
            };
            let kind = if suggestion.is_some() {
                RefusalKind::UnknownWord
            } else {
                RefusalKind::Malformed
            };
            refuse(Refusal::at(
                format!("{line}:{column}"),
                kind,
                refusal.message().to_owned(),
                suggestion,
            ))
        }
    }
}
