//! The condition vocabulary a spec names, and the erased [`Condition`] it builds.

use crate::ecs::EcsError;
use crate::spec::{EvaluateAny, TypedEval};
use crate::verb::{Evaluate, Observe};

/// A condition, erased to the view a chain's walk calls.
///
/// A spec names a condition as text and [`Source::condition`](super::Source::condition)
/// builds one for its own output type; a native chain names one directly
/// through `ObserveBuilder::on` (feature `ecs`). Both arrive here, so the two
/// paths share one erasure and one type-mismatch report rather than each
/// carrying its own.
///
/// This is the *only* way a condition is built for a materialized chain. The
/// constructor is generic over the value it evaluates — as the verb traits are —
/// and the downcast to that value is checked when the condition runs, so a
/// condition built for one type and handed a value of another reports
/// [`EcsError::EvaluateError`] rather than
/// answering a false.
///
/// # Examples
///
/// ```
/// use lgwks_bot::Condition;
///
/// let loud = Condition::new::<u16, _>(|value: &u16| *value >= 500);
/// assert_eq!(format!("{loud:?}"), "Condition");
/// ```
pub struct Condition(Box<dyn EvaluateAny>);

impl Condition {
    /// Erase a concrete condition, typed against `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::Condition;
    ///
    /// let always = Condition::new::<String, _>(|_: &String| true);
    /// assert_eq!(format!("{always:?}"), "Condition");
    /// ```
    #[must_use]
    pub fn new<T, C>(condition: C) -> Self
    where
        T: 'static,
        C: Evaluate<T> + 'static,
    {
        Self(Box::new(TypedEval {
            inner: condition,
            _marker: std::marker::PhantomData::<T>,
        }))
    }

    /// Take the erased condition, for the chain assembler.
    pub(crate) fn into_evaluate_any(self) -> Box<dyn EvaluateAny> {
        self.0
    }
}

impl std::fmt::Debug for Condition {
    /// Prints `Condition`, because an erased condition carries no identity of
    /// its own: what it evaluates is the source's output type, which the chain,
    /// not this handle, is what names.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Condition")
    }
}

/// Build the order-free wire condition `condition_id` names, for source `O`.
///
/// Monomorphized on `O`, so `O::Output` is the concrete type the condition
/// evaluates and no schema table has to be matched by hand.
pub(super) fn make_condition<O>(condition_id: &str) -> Result<Condition, EcsError>
where
    O: Observe + 'static,
    O::Output: Clone + PartialEq + 'static,
{
    let identifier = condition_id.trim();
    match identifier {
        "changed" => Ok(Condition::new::<O::Output, _>(
            crate::domain::eval::Changed::new(),
        )),
        "always" => Ok(Condition::new::<O::Output, _>(|_: &O::Output| true)),
        _ => Err(EcsError::UnknownCondition {
            condition: identifier.to_owned(),
            argument_parse: String::from("not an identifier this build knows"),
        }),
    }
}

/// Build the wire condition `condition_id` names for an ordered source `O`:
/// the two thresholds, then the order-free vocabulary of [`make_condition`].
pub(super) fn make_ordered_condition<O>(condition_id: &str) -> Result<Condition, EcsError>
where
    O: Observe + 'static,
    O::Output: Clone + PartialEq + PartialOrd + std::str::FromStr + 'static,
    // Propagated from `parse_bound`, which carries the parse failure into the
    // error rather than dropping it. An output type whose `FromStr` error cannot
    // be rendered cannot be used as a threshold bound, because a caller
    // repairing a bad spec would have nothing to read.
    <O::Output as std::str::FromStr>::Err: std::fmt::Display,
{
    let identifier = condition_id.trim();
    if let Some(argument) = parenthesized(identifier, "threshold::above") {
        let bound = parse_bound::<O::Output>(argument, identifier)?;
        return Ok(Condition::new::<O::Output, _>(
            crate::domain::eval::Above::new(bound),
        ));
    }
    if let Some(argument) = parenthesized(identifier, "threshold::below") {
        let bound = parse_bound::<O::Output>(argument, identifier)?;
        return Ok(Condition::new::<O::Output, _>(
            crate::domain::eval::Below::new(bound),
        ));
    }
    make_condition::<O>(identifier)
}

/// The argument inside `prefix(...)`, or `None` when `identifier` is not that.
fn parenthesized<'a>(identifier: &'a str, prefix: &str) -> Option<&'a str> {
    identifier
        .strip_prefix(prefix)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

/// Parse a threshold argument as `T`, reporting the whole identifier on failure.
///
/// The `FromStr` error is carried rather than dropped. `ParseIntError`'s own
/// text is "invalid digit found in string" or "cannot parse integer from empty
/// string", which says what the standard library saw and nothing about the
/// condition the caller has to repair — but it is the part that distinguishes
/// an empty threshold from a malformed one, so it rides along beside
/// `identifier`.
fn parse_bound<T>(argument: &str, identifier: &str) -> Result<T, EcsError>
where
    T: std::str::FromStr,
    // The parse failure is carried into the error, so the error type has to be
    // renderable. Every std numeric and bool parser meets this; a `FromStr`
    // whose `Err` cannot print would be refusing in a way this error cannot
    // report, which is why the bound is here rather than the field being
    // dropped.
    T::Err: std::fmt::Display,
{
    argument
        .trim()
        .parse::<T>()
        .map_err(|not_this_type| EcsError::UnknownCondition {
            condition: identifier.to_owned(),
            argument_parse: not_this_type.to_string(),
        })
}
