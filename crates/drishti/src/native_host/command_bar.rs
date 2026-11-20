//! Command-bar intent pipeline and suggestions.
//!
//! Absorbed from `surfaces/native-host/Sources/CanvasCommandBar/
//! IntentPipeline.swift` and `CommandSuggestions.swift` (kernel#850 row 15).
//! Slash commands are fixed (`find`, `pin`, `unpin`, `help`); anything else
//! after `/` is unknown, and overlong input is rejected before parsing.

use std::collections::BTreeMap;

use lgwks_macros::Error;

use super::protocol::{JsonValue, PartialIntent};

/// Maximum input characters (Unicode scalar count).
const MAX_INPUT_LENGTH: usize = 512;

/// Intent parse failures.
///
/// # Examples
///
/// ```
/// use drishti::native_host::command_bar::{IntentParseError, IntentPipeline};
///
/// assert_eq!(
///     IntentPipeline::parse("/exec rm").err(),
///     Some(IntentParseError::UnknownCommand)
/// );
/// assert_eq!(
///     IntentPipeline::parse(&"x".repeat(513)).err(),
///     Some(IntentParseError::InputTooLong)
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum IntentParseError {
    /// Input exceeds 512 characters.
    #[error("command-bar input too long")]
    InputTooLong,
    /// Slash command is not one of the four known commands.
    #[error("unknown command-bar command")]
    UnknownCommand,
}

/// Parse command-bar input into a partial intent.
///
/// # Examples
///
/// ```
/// use drishti::native_host::command_bar::IntentPipeline;
///
/// let intent = IntentPipeline::parse("/find acme")?;
/// assert_eq!(intent.kind(), "intent.find");
/// assert_eq!(intent.text(), "acme");
/// # Ok::<(), drishti::native_host::command_bar::IntentParseError>(())
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct IntentPipeline;

impl IntentPipeline {
    /// Parse input: `/cmd rest` dispatches to the four slash commands
    /// (command names are case-insensitive), anything else becomes `intent.freeform`. Empty input is freeform with
    /// empty text; a bare `/` is unknown.
    ///
    /// # Errors
    ///
    /// Returns [`IntentParseError::InputTooLong`] past 512 characters and
    /// [`IntentParseError::UnknownCommand`] for unknown slash commands.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::command_bar::{IntentParseError, IntentPipeline};
    ///
    /// // Command names are case-insensitive; free text trims and is freeform.
    /// assert_eq!(IntentPipeline::parse("/PIN w1")?.kind(), "intent.pin");
    /// assert_eq!(IntentPipeline::parse("  hello  ")?.kind(), "intent.freeform");
    /// // A bare slash names no command.
    /// assert_eq!(IntentPipeline::parse("/").err(), Some(IntentParseError::UnknownCommand));
    /// # Ok::<(), IntentParseError>(())
    /// ```
    pub fn parse(input: &str) -> Result<PartialIntent, IntentParseError> {
        input
            .chars()
            .nth(MAX_INPUT_LENGTH)
            .is_none()
            .then_some(())
            .ok_or(IntentParseError::InputTooLong)?;
        let trimmed = input.trim();
        if let Some(without_slash) = trimmed.strip_prefix('/') {
            // Split at the first space; no space means the command stands
            // alone with no rest.
            let (command, rest) = match without_slash.split_once(' ') {
                Some(pair) => pair,
                None => (without_slash, ""),
            };
            return parse_slash_command(command, rest);
        }
        Ok(PartialIntent::new(
            String::from("intent.freeform"),
            JsonValue::Object(BTreeMap::from([(
                String::from("text"),
                JsonValue::String(String::from(trimmed)),
            )])),
            String::from(trimmed),
        ))
    }
}

/// Dispatch one slash command.
fn parse_slash_command(command: &str, rest: &str) -> Result<PartialIntent, IntentParseError> {
    // Case-insensitive like [`CommandSuggestions`]: what the bar completes,
    // the parser accepts. The text after the command keeps its case.
    match command.to_lowercase().as_str() {
        "find" => Ok(single_field_intent("intent.find", "query", rest)),
        "pin" => Ok(single_field_intent("intent.pin", "widget_id", rest)),
        "unpin" => Ok(PartialIntent::new(
            String::from("intent.unpin"),
            JsonValue::Object(BTreeMap::new()),
            String::from(rest),
        )),
        "help" => Ok(PartialIntent::new(
            String::from("intent.help"),
            JsonValue::Object(BTreeMap::new()),
            String::from(rest),
        )),
        _ => Err(IntentParseError::UnknownCommand),
    }
}

/// One intent carrying a single named text field.
fn single_field_intent(kind: &str, field: &str, rest: &str) -> PartialIntent {
    PartialIntent::new(
        String::from(kind),
        JsonValue::Object(BTreeMap::from([(
            String::from(field),
            JsonValue::String(String::from(rest)),
        )])),
        String::from(rest),
    )
}

/// Slash-command suggestions for partially typed input.
///
/// # Examples
///
/// ```
/// use drishti::native_host::command_bar::CommandSuggestions;
///
/// assert_eq!(CommandSuggestions::suggestions("/un"), vec![String::from("/unpin")]);
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct CommandSuggestions;

impl CommandSuggestions {
    /// Suggest completions: `/`-prefixed input filters the four commands
    /// case-insensitively; anything else suggests nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::command_bar::CommandSuggestions;
    ///
    /// assert_eq!(CommandSuggestions::suggestions("/P"), vec![String::from("/pin")]);
    /// assert_eq!(CommandSuggestions::suggestions("/").len(), 4);
    /// assert!(CommandSuggestions::suggestions("find").is_empty());
    /// ```
    #[must_use]
    pub fn suggestions(input: &str) -> Vec<String> {
        if !input.starts_with('/') {
            return Vec::new();
        }
        let lowered = input.to_lowercase();
        const COMMANDS: &[&str] = &["/find", "/pin", "/unpin", "/help"];
        COMMANDS
            .iter()
            .filter(|command| command.starts_with(lowered.as_str()))
            .map(|command| String::from(*command))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandSuggestions, IntentParseError, IntentPipeline};
    use crate::native_host::protocol::JsonValue;

    /// Assert one parse vector by kind and text.
    fn assert_parse(input: &str, kind: &str, text: &str) -> Result<(), String> {
        let intent = IntentPipeline::parse(input).map_err(|error| format!("{error}"))?;
        if intent.kind() != kind || intent.text() != text {
            return Err(format!(
                "parse({input:?}) = ({}, {}), want ({kind}, {text})",
                intent.kind(),
                intent.text()
            ));
        }
        Ok(())
    }

    /// Slash commands, freeform, and empty input parse to their shapes.
    #[test]
    fn commands_parse_to_intent_shapes() -> Result<(), String> {
        assert_parse("/find acme", "intent.find", "acme")?;
        assert_parse("/pin widget-1", "intent.pin", "widget-1")?;
        assert_parse("/unpin", "intent.unpin", "")?;
        assert_parse("/help", "intent.help", "")?;
        assert_parse("hello world", "intent.freeform", "hello world")?;
        assert_parse("", "intent.freeform", "")?;
        let freeform = IntentPipeline::parse("  padded  ").map_err(|error| format!("{error}"))?;
        if freeform.text() != "padded" {
            return Err(String::from("freeform must trim"));
        }
        Ok(())
    }

    /// Unknown commands and overlong input fail closed.
    #[test]
    fn unknown_and_overlong_inputs_fail() -> Result<(), String> {
        let Err(error) = IntentPipeline::parse("/exec rm") else {
            return Err(String::from("exec must fail"));
        };
        if error != IntentParseError::UnknownCommand {
            return Err(String::from("exec must be unknown"));
        }
        let Err(error) = IntentPipeline::parse("/") else {
            return Err(String::from("bare slash must fail"));
        };
        if error != IntentParseError::UnknownCommand {
            return Err(String::from("bare slash must be unknown"));
        }
        let long = "x".repeat(513);
        let Err(error) = IntentPipeline::parse(&long) else {
            return Err(String::from("513 chars must fail"));
        };
        if error != IntentParseError::InputTooLong {
            return Err(String::from("513 chars must be too long"));
        }
        let boundary = "x".repeat(512);
        IntentPipeline::parse(&boundary).map_err(|error| format!("{error}"))?;
        Ok(())
    }

    /// Payload shapes carry their fields.
    #[test]
    fn payloads_carry_fields() -> Result<(), String> {
        let find = IntentPipeline::parse("/find acme").map_err(|error| format!("{error}"))?;
        let Some(map) = find.payload().object_value() else {
            return Err(String::from("find payload must be an object"));
        };
        if map.get("query") != Some(&JsonValue::String(String::from("acme"))) {
            return Err(String::from("find payload must carry the query"));
        };
        Ok(())
    }

    /// Suggestions filter case-insensitively on the slash prefix.
    #[test]
    fn suggestions_filter() -> Result<(), String> {
        if CommandSuggestions::suggestions("/p") != [String::from("/pin")] {
            return Err(String::from("/p must suggest /pin"));
        }
        if CommandSuggestions::suggestions("/P") != [String::from("/pin")] {
            return Err(String::from("matching must be case-insensitive"));
        }
        if !CommandSuggestions::suggestions("find").is_empty() {
            return Err(String::from("non-slash input must suggest nothing"));
        }
        Ok(())
    }
}
