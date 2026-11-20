//! Variable scope, compiled templates and the interpolator. Split from `session.rs` (#526).

use super::{
    AnswerDomain, BTreeMap, MAX_RECORD_BYTES, MAX_VALUE_BYTES, SessionError, Value, VarType, bound,
    valid_identifier, validate_declarations,
};

/// Named, typed variable declarations and their assigned values.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct VarScope {
    /// Declared variable types.
    pub(super) declarations: BTreeMap<String, VarType>,
    /// Values assigned by accepted answers.
    pub(super) values: BTreeMap<String, Value>,
}

impl VarScope {
    /// Construct an empty-value scope from declarations.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use lgwks_bot::session::{VarScope, VarType};
    ///
    /// let scope = VarScope::new(BTreeMap::from([("size".to_owned(), VarType::Integer)]))?;
    /// assert!(scope.values().is_empty());
    /// assert!(VarScope::new(BTreeMap::from([("not valid".to_owned(), VarType::Integer)])).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(declarations: BTreeMap<String, VarType>) -> Result<Self, SessionError> {
        validate_declarations(&declarations)?;
        Ok(Self {
            declarations,
            values: BTreeMap::new(),
        })
    }

    /// Return the declared variable types.
    #[must_use]
    pub fn declarations(&self) -> &BTreeMap<String, VarType> {
        &self.declarations
    }

    /// Returns how an answer writing `name` must be read.
    ///
    /// Defaults to [`AnswerDomain::Label`] for a name that is not declared,
    /// which is the permissive reading rather than a silent claim: flow
    /// validation rejects an undeclared writer, so reaching this with an unknown
    /// name is a caller's mistake and not a question about the answer.
    #[must_use]
    pub fn answer_domain(&self, name: &str) -> AnswerDomain {
        match self.declarations.get(name) {
            Some(declared) => declared.answer_domain(),
            None => AnswerDomain::Label,
        }
    }

    /// Return all currently assigned values.
    #[must_use]
    pub fn values(&self) -> &BTreeMap<String, Value> {
        &self.values
    }

    /// Return one assigned value, if the variable has been written.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    /// Assign a typed value after checking its declaration.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use lgwks_bot::session::{Value, VarScope, VarType};
    ///
    /// let mut scope = VarScope::new(BTreeMap::from([("size".to_owned(), VarType::Integer)]))?;
    /// scope.set("size", Value::Integer(3))?;
    /// assert_eq!(scope.get("size"), Some(&Value::Integer(3)));
    /// assert!(scope.set("size", Value::Boolean(true)).is_err());
    /// assert!(scope.set("colour", Value::Integer(1)).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set(&mut self, name: &str, value: Value) -> Result<(), SessionError> {
        let Some(declared) = self.declarations.get(name) else {
            let refusal = Err(SessionError::UndeclaredVariable {
                name: name.to_owned(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "set: returning an error to the caller");
            return refusal;
        };
        if value_matches_type(declared, &value) {
            self.values.insert(name.to_owned(), value);
            Ok(())
        } else {
            Err(SessionError::VariableTypeMismatch {
                name: name.to_owned(),
            })
        }
    }

    /// Parse and assign an answer according to the variable declaration.
    ///
    /// Runs [`VarType::decode_answer`], the same decoder flow validation runs
    /// over every ask candidate. For a validated flow the decode cannot fail:
    /// the candidate that reaches here is one validation already accepted. It
    /// stays a `Result` because this method is public and a caller may hold a
    /// scope that no `FlowSpec` validated.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use lgwks_bot::session::{Value, VarScope, VarType};
    ///
    /// let mut scope = VarScope::new(BTreeMap::from([("size".to_owned(), VarType::Integer)]))?;
    /// scope.set_from_answer("size", " 42 ")?;
    /// assert_eq!(scope.get("size"), Some(&Value::Integer(42)));
    /// assert!(scope.set_from_answer("size", "large").is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set_from_answer(&mut self, name: &str, answer: &str) -> Result<(), SessionError> {
        self.set_from_answer_within(name, answer, MAX_VALUE_BYTES)
    }

    /// Parse and assign an answer under an explicit value ceiling.
    ///
    /// The decoder and the store, plus the third step a session needs: the
    /// value has to fit. Sizing happens between decoding and storing, so a
    /// candidate that decodes is refused before the scope holds it — which is
    /// what keeps "accepted" and "retained" the same word.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::InvalidVariableValue`] for an answer the declared
    /// type cannot hold, and [`SessionError::ValueTooLarge`] for one that decodes
    /// to a value wider than `value_bytes`.
    pub fn set_from_answer_within(
        &mut self,
        name: &str,
        answer: &str,
        value_bytes: usize,
    ) -> Result<(), SessionError> {
        let Some(declared) = self.declarations.get(name) else {
            let refusal = Err(SessionError::UndeclaredVariable {
                name: name.to_owned(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "set_from_answer_within: returning an error to the caller");
            return refusal;
        };
        // One decoder, not two: `VarType::decode_answer` is the same reading of
        // an answer that flow validation runs, so a value this store accepts is
        // a value the declaration would have accepted. The rejection is carried
        // into the error rather than dropped: the scope's contract is one
        // "cannot hold this" error, and naming *why* keeps a caller from having
        // to guess between a non-integer, an out-of-range integer, a bad boolean
        // and an undeclared choice.
        let value = declared.decode_answer(answer).map_err(|rejection| {
            SessionError::InvalidVariableValue {
                variable: name.to_owned(),
                value: answer.to_owned(),
                reason: rejection.to_string(),
            }
        })?;
        let bytes = value.rendered_bytes();
        if bytes > value_bytes {
            let refusal = Err(SessionError::ValueTooLarge {
                variable: name.to_owned(),
                bytes,
                limit: value_bytes,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "set_from_answer_within: returning an error to the caller");
            return refusal;
        }
        self.set(name, value)
    }

    /// Interpolate `${name}` markers using the assigned values, under the
    /// operator's shipped record ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::MalformedTemplate`] for invalid syntax,
    /// [`SessionError::VariableUnset`] for an unresolved marker, and
    /// [`SessionError::TemplateExpansionTooLarge`] past the ceiling.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use lgwks_bot::session::{VarScope, VarType};
    ///
    /// let mut scope = VarScope::new(BTreeMap::from([("name".to_owned(), VarType::String)]))?;
    /// assert!(scope.interpolate("hello ${name}").is_err());
    /// scope.set_from_answer("name", "Ada")?;
    /// assert_eq!(scope.interpolate("hello ${name}")?, "hello Ada");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn interpolate(&self, template: &str) -> Result<String, SessionError> {
        self.interpolate_within(template, MAX_RECORD_BYTES)
    }

    /// Interpolate `${name}` markers under an explicit byte ceiling.
    ///
    /// A scope on its own has no session behind it, so the ceiling is an
    /// argument. The expansion is sized before it is allocated and refused
    /// rather than truncated: a truncated record would be a record the journal
    /// claims happened and that nobody can read.
    ///
    /// # Errors
    ///
    /// As [`Self::interpolate`], with `record_bytes` in place of the shipped
    /// ceiling.
    pub fn interpolate_within(
        &self,
        template: &str,
        record_bytes: usize,
    ) -> Result<String, SessionError> {
        TemplateInterpolator::new().interpolate(template, self, "<scope>", record_bytes)
    }
}

/// Check one scalar against its declared type.
fn value_matches_type(declared: &VarType, value: &Value) -> bool {
    match *declared {
        VarType::String => matches!(value, &Value::String(_)),
        VarType::Integer => matches!(value, &Value::Integer(_)),
        VarType::Boolean => matches!(value, &Value::Boolean(_)),
        VarType::Choice(ref options) => match *value {
            Value::Choice(ref value) => options.contains(value),
            _ => false,
        },
    }
}

/// One compiled piece of a template.
///
/// Borrowed from the document rather than owned: a compiled template is a
/// second view of bytes that are already resident, and copying them would make
/// compiling cost what the expansion costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TemplatePart<'a> {
    /// Text copied to the output unchanged.
    Literal(&'a str),
    /// A `${name}` marker replaced by the scope's value for `name`.
    Variable(&'a str),
}

/// A template compiled once into literal and variable parts.
///
/// Compilation is the step that makes a byte ceiling enforceable before
/// allocation. The parts are the template's placeholders resolved to nothing
/// but the names they mention, so the expanded size of the whole is one
/// addition per part, computed with checked arithmetic
/// ([`Self::expanded_bytes`]) and compared with the caller's ceiling *before*
/// [`Self::render`] allocates the output. Rendering an unbounded expansion to
/// find out that it is unbounded is exactly the failure this ordering removes.
///
/// `a_compiled_template_splits_into_parts_and_cannot_be_sized_before_its_name_has_a_value`
/// (`tests/it/session_front_door.rs`) runs it: `hello ${name}` compiles to two parts, and its
/// expanded size is an error while `name` has no value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CompiledTemplate<'a> {
    /// Literal and variable parts in source order.
    pub(super) parts: Vec<TemplatePart<'a>>,
}

impl<'a> CompiledTemplate<'a> {
    /// Compile `${name}` markers into parts.
    ///
    /// The one template parser: the loader's template check uses it to confirm
    /// every marker names a declared variable, and the interpolator uses it to
    /// size and render. A second scanner would be a second grammar.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::MalformedTemplate`] for an unterminated marker or a
    /// name that is not an identifier.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::session::{CompiledTemplate, TemplatePart};
    ///
    /// let template = CompiledTemplate::compile("hi ${name}")?;
    /// assert_eq!(
    ///     template.parts(),
    ///     &[TemplatePart::Literal("hi "), TemplatePart::Variable("name")]
    /// );
    /// assert!(CompiledTemplate::compile("hi ${name").is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn compile(template: &'a str) -> Result<Self, SessionError> {
        let mut parts = Vec::new();
        let mut rest = template;
        while let Some((literal, after)) = rest.split_once("${") {
            if !literal.is_empty() {
                parts.push(TemplatePart::Literal(literal));
            }
            let Some((name, tail)) = after.split_once('}') else {
                let refusal = Err(SessionError::MalformedTemplate {
                    node: "<runtime>".into(),
                    field: "template",
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "compile: returning an error to the caller");
                return refusal;
            };
            if !valid_identifier(name) {
                let refusal = Err(SessionError::MalformedTemplate {
                    node: "<runtime>".into(),
                    field: "template",
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "compile: returning an error to the caller");
                return refusal;
            }
            parts.push(TemplatePart::Variable(name));
            rest = tail;
        }
        if !rest.is_empty() {
            parts.push(TemplatePart::Literal(rest));
        }
        Ok(Self { parts })
    }

    /// Return the compiled parts in source order.
    #[must_use]
    pub fn parts(&self) -> &[TemplatePart<'a>] {
        &self.parts
    }

    /// The exact number of bytes this template expands to under `scope`.
    ///
    /// Exact, and computed before any output is allocated. Every part
    /// contributes a known count, so the total is a checked sum and cannot
    /// overflow into a small number that would pass a ceiling it should fail.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::VariableUnset`] for a marker with no value, or
    /// [`SessionError::TemplateExpansionTooLarge`] with `bytes` of [`usize::MAX`]
    /// if the sum overflows, which no real expansion can and which is reported
    /// rather than wrapped.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use lgwks_bot::session::{CompiledTemplate, VarScope, VarType};
    ///
    /// let mut scope = VarScope::new(BTreeMap::from([("name".to_owned(), VarType::String)]))?;
    /// let template = CompiledTemplate::compile("hi ${name}")?;
    /// assert!(template.expanded_bytes(&scope).is_err());
    /// scope.set_from_answer("name", "Ada")?;
    /// assert_eq!(template.expanded_bytes(&scope)?, 6);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn expanded_bytes(&self, scope: &VarScope) -> Result<usize, SessionError> {
        let mut total = 0usize;
        for part in &self.parts {
            let bytes = match *part {
                TemplatePart::Literal(text) => text.len(),
                TemplatePart::Variable(name) => bound(scope, name)?.rendered_bytes(),
            };
            total = total
                .checked_add(bytes)
                .ok_or(SessionError::TemplateExpansionTooLarge {
                    node: "<runtime>".into(),
                    bytes: usize::MAX,
                    limit: usize::MAX,
                })?;
        }
        Ok(total)
    }

    /// Render the template into a new string.
    ///
    /// Allocates exactly [`Self::expanded_bytes`] and cannot fail where that
    /// did not: every marker was resolved during sizing, and the values are
    /// re-read from the same scope. Callers with a ceiling call
    /// [`Self::expanded_bytes`] first — [`Interpolate::interpolate`] is that
    /// caller — so the refusal happens before this runs rather than after it
    /// has produced the text it was supposed to avoid producing.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::VariableUnset`] if a value disappeared between
    /// sizing and rendering. A scope is not mutated during a render, so this is
    /// reachable only through a caller that interleaves the two.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use lgwks_bot::session::{CompiledTemplate, VarScope, VarType};
    ///
    /// let mut scope = VarScope::new(BTreeMap::from([("name".to_owned(), VarType::String)]))?;
    /// scope.set_from_answer("name", "Ada")?;
    /// let template = CompiledTemplate::compile("hi ${name}")?;
    /// assert_eq!(template.render(&scope)?, "hi Ada");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn render(&self, scope: &VarScope) -> Result<String, SessionError> {
        let mut output = String::new();
        for part in &self.parts {
            match *part {
                TemplatePart::Literal(text) => output.push_str(text),
                TemplatePart::Variable(name) => {
                    output.push_str(&bound(scope, name)?.to_string());
                }
            }
        }
        Ok(output)
    }
}

/// One question's candidate vocabulary, as the resolver seam sees it.
///
/// A resolver is handed three things and a slice of options is only two of
/// them: what the person typed, the options in front of them, and the *identity
/// of the question* those options belong to. The identity is not decoration. A
/// learned alias is a confirmed fact about a question — *in this question, "the
/// usual" means "Repeat last order"* — and a slice of strings is not a question:
/// the same slice can appear in two different questions, and the same question
/// offers a *different* slice after an edit or a reorder. Carrying the identity
/// alongside the options is what lets a confirmation follow its option to a new
/// position, and refuse to follow whatever occupies its old one.
///
/// Borrowed rather than owned, because the caller already holds both: an owned
/// copy would be a second copy of the same fact, and two copies of a fact drift.
///
/// The third fact is how the answers are read. [`AnswerDomain`] travels with the
/// options because it is a property of *this* question, and a resolver that had
/// to infer it from the option strings would be guessing: the same list is two
/// distinct answers at an integer question and one folded answer at a label
/// question.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Question<'a> {
    /// Stable identity of the question. In a flow this is the ask node id.
    pub(super) id: &'a str,
    /// The options offered right now.
    pub(super) options: &'a [String],
    /// How this question's answers are meant to be read.
    pub(super) domain: AnswerDomain,
}

impl<'a> Question<'a> {
    /// Names one question's vocabulary, read as natural-language labels.
    #[must_use]
    pub const fn new(id: &'a str, options: &'a [String]) -> Self {
        Self {
            id,
            options,
            domain: AnswerDomain::Label,
        }
    }

    /// Reads this question's answers under `domain`.
    ///
    /// A builder rather than a fourth argument on [`Self::new`], because the
    /// overwhelming majority of questions are label questions and a resolver
    /// call site should not have to spell that out. The default is the one the
    /// option strings are written in; a caller that has a typed variable in hand
    /// has the one fact that changes it.
    #[must_use]
    pub const fn with_domain(self, domain: AnswerDomain) -> Self {
        Self { domain, ..self }
    }

    /// Returns the question's stable identity.
    #[must_use]
    pub const fn id(&self) -> &'a str {
        self.id
    }

    /// Returns the options offered right now.
    #[must_use]
    pub const fn options(&self) -> &'a [String] {
        self.options
    }

    /// Returns how this question's answers are meant to be read.
    #[must_use]
    pub const fn domain(&self) -> AnswerDomain {
        self.domain
    }
}

/// Interpolation seam for replacing the template engine later.
pub trait Interpolate {
    /// Expand `${name}` markers using the supplied scope, refusing an
    /// expansion larger than `limit` bytes.
    ///
    /// `limit` is a parameter rather than a property of the interpolator
    /// because the ceiling is the session's, not the engine's: the same
    /// interpolator serves a scope with the shipped ceiling and a session with
    /// a tighter one. `node` names the flow node the template came from and is
    /// carried into the refusal, which is what makes an oversized expansion
    /// attributable to the line that caused it.
    ///
    /// Implementations must check the size *before* allocating the output.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::MalformedTemplate`] for invalid syntax,
    /// [`SessionError::VariableUnset`] for an unresolved marker, and
    /// [`SessionError::TemplateExpansionTooLarge`] for an expansion over `limit`.
    fn interpolate(
        &self,
        template: &str,
        scope: &VarScope,
        node: &str,
        limit: usize,
    ) -> Result<String, SessionError>;
}

/// The shipped bounded interpolation implementation.
///
/// Allocation-bounded in the strict sense: the expanded byte count is computed
/// from the compiled template and refused when it exceeds the caller's ceiling,
/// so the output buffer this creates is never larger than that ceiling no
/// matter how many times a placeholder repeats.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct TemplateInterpolator;

impl TemplateInterpolator {
    /// Construct the default interpolator.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Interpolate for TemplateInterpolator {
    fn interpolate(
        &self,
        template: &str,
        scope: &VarScope,
        node: &str,
        limit: usize,
    ) -> Result<String, SessionError> {
        let compiled = CompiledTemplate::compile(template).map_err(|error| match error {
            // The compile-time diagnostics name the runtime as their node,
            // because the compiler is not handed the node it is compiling for.
            // Here it is known, so it replaces the placeholder.
            SessionError::MalformedTemplate { field, .. } => SessionError::MalformedTemplate {
                node: node.to_owned(),
                field,
            },
            other => other,
        })?;
        let bytes = compiled.expanded_bytes(scope)?;
        if bytes > limit {
            let refusal = Err(SessionError::TemplateExpansionTooLarge {
                node: node.to_owned(),
                bytes,
                limit,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "interpolate: returning an error to the caller");
            return refusal;
        }
        compiled.render(scope)
    }
}
