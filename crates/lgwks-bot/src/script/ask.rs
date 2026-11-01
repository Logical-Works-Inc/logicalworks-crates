//! `ask`: one model call as a step of a flow.
//!
//! A model is a port, not a dependency: [`ModelPort`] is the trait a provider
//! implements, and the tests install [`StubModel`](crate::proposal::StubModel)
//! through it, so no test makes a paid call or reaches the network. The call
//! takes a token budget and a deadline, and its output is decoded into the
//! declared type through [`AskDecode`](super::AskDecode): a truncated or
//! undecodable output is refused typed, never decoded into a claim about what
//! the whole output said.
//!
//! # Errors
//!
//! The word's own failures are [`FlowError`] arms: [`FlowError::Blocked`]
//! without the `bot.model` grant, [`FlowError::TimedOut`] past the deadline,
//! [`FlowError::InvalidBound`] for a truncated output or a token budget of
//! zero, and [`FlowError::Failed`] for output no declared type decodes. A
//! refusal carries no model text, so a log line can never echo a prompt.

use std::time::Duration;

use crate::cap::Cap;
use crate::journal::frame::SaturatingFrom;
use crate::proposal::StubModel;

use super::{FlowError, Scope, Site};

/// One model call's token budget and deadline.
///
/// The prompt travels beside it, as the word's other argument, so the budget
/// reads as what it bounds: at most `max_tokens` of output, answered within
/// `deadline` when one is declared, or whenever the model answers when none
/// is. A zero `max_tokens` is refused: a call that may not emit is not a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskRequest {
    /// The prompt, as bytes.
    prompt: Vec<u8>,
    /// The most output tokens the call may emit.
    max_tokens: u32,
    /// The bound on the call, or `None` to inherit the enclosing blocks'.
    deadline: Option<Duration>,
}

impl AskRequest {
    /// Hand the model these bytes; the default is empty, and no refusal echoes them.
    #[must_use]
    pub fn prompt(mut self, prompt: Vec<u8>) -> Self {
        self.prompt = prompt;
        self
    }

    /// Let the call emit at most `tokens`; zero is refused when the ask runs.
    #[must_use]
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }

    /// Race the call against `deadline`; zero is refused when the ask runs.
    #[must_use]
    pub fn deadline(mut self, deadline: Duration) -> Self {
        self.deadline = Some(deadline);
        self
    }
}

impl Default for AskRequest {
    /// An empty prompt under a thousand tokens and no deadline of its own.
    fn default() -> Self {
        Self {
            prompt: Vec::new(),
            max_tokens: 1_000,
            deadline: None,
        }
    }
}

/// What a model call produced: the bytes and whether they are whole.
///
/// A port reports truncation rather than the caller inferring it, because
/// only the producer knows whether its output ended: a decoder that guessed
/// from the shape would admit a cut document as complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOutput {
    /// The output bytes, whole or cut at the token budget.
    bytes: Vec<u8>,
    /// Whether the output was cut before it ended.
    truncated: bool,
}

impl ModelOutput {
    /// The output bytes, whole or cut at the token budget.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Whether the output was cut before it ended.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

impl From<(Vec<u8>, bool)> for ModelOutput {
    /// The output `bytes`, marked truncated as `truncated` says.
    ///
    /// The one construction a caller outside the crate needs: the stub a
    /// test installs answers a fixed output through it, and truncation is a
    /// fact its port reports rather than a shape it assembles.
    fn from((bytes, truncated): (Vec<u8>, bool)) -> Self {
        Self { bytes, truncated }
    }
}

/// The provider behind an `ask`: a port the tests stub.
///
/// Implemented by the model the run calls. The contract is total: `complete`
/// answers the output for `prompt` within `max_tokens`, marking it truncated
/// when the budget cut it, and refuses typed when the provider itself failed.
/// Nothing here dials out; that is the implementor's, and the tests'
/// implementor is [`StubModel`](crate::proposal::StubModel), a pure function
/// of its seed.
pub trait ModelPort: Sync {
    /// The provider's own failure, answered as the flow's.
    type Error: Into<FlowError>;

    /// Complete `prompt` within `max_tokens`.
    ///
    /// The returned future resolves with the output and whether the budget
    /// cut it; a provider failure resolves with the provider's typed error.
    fn complete(
        &self,
        prompt: &[u8],
        max_tokens: u32,
    ) -> impl Future<Output = Result<ModelOutput, Self::Error>> + Send;
}

impl<M> ModelPort for &M
where
    M: ModelPort + Sync,
{
    /// The provider's own failure, answered as the flow's.
    type Error = M::Error;

    /// Complete through the shared reference.
    fn complete(
        &self,
        prompt: &[u8],
        max_tokens: u32,
    ) -> impl Future<Output = Result<ModelOutput, Self::Error>> + Send {
        (*self).complete(prompt, max_tokens)
    }
}

impl ModelPort for StubModel {
    /// The stub never fails: its output is a pure function of its seed.
    type Error = FlowError;

    /// The seed's output, cut at `max_tokens` and marked truncated when the
    /// budget cut it — or when the seed drew the truncated shape, whose bytes
    /// end mid-document however large the budget is.
    async fn complete(&self, _prompt: &[u8], max_tokens: u32) -> Result<ModelOutput, FlowError> {
        let mut bytes = self.output().to_vec();
        let mut truncated = self.shape() == crate::proposal::Shape::Truncated;
        let budget = usize::saturating_from(max_tokens);
        if bytes.len() > budget {
            bytes.truncate(budget);
            truncated = true;
        }
        Ok(ModelOutput { bytes, truncated })
    }
}

/// `ask model with (request)`: one model call as a step of its own.
///
/// Enters the `ask` step, checks the run holds `bot.model`, races the call
/// against `request`'s deadline when one is declared, refuses a truncated
/// output, and decodes the whole one into `T`.
///
/// # Errors
///
/// [`FlowError::Blocked`] without the grant; [`FlowError::InvalidBound`] for
/// a zero token budget or a truncated output; [`FlowError::TimedOut`] past
/// the deadline; [`FlowError::Failed`] when `T` does not decode the output;
/// and the provider's own failure as the flow's.
///
/// # Examples
///
/// ```rust
/// lgwks_bot::script! {
///     pub flow draft(model: &lgwks_bot::proposal::StubModel) -> String:
///         let request = lgwks_bot::script::AskRequest::default().max_tokens(0)
///         let reply: String = ask model with (request)
///         give back reply
/// }
///
/// # fn main() -> Result<(), lgwks_bot::script::FlowError> {
/// let scope = lgwks_bot::script::Scope::root(lgwks_bot::script::Tenant::new("acme")?);
/// let model = lgwks_bot::proposal::StubModel::from_seed(0);
/// let refused = lgwks_bot::block_on(draft(&scope, &model));
/// assert!(
///     matches!(refused, Err(lgwks_bot::script::FlowError::InvalidBound { .. })),
///     "a zero token budget is refused before the model is called"
/// );
/// # Ok(())
/// # }
/// ```
pub async fn ask<T, M>(
    scope: &Scope,
    step: &str,
    model: &M,
    request: &AskRequest,
) -> Result<T, FlowError>
where
    T: super::AskDecode,
    M: ModelPort,
{
    let child = scope.enter(step)?;
    let outcome = ask_in(&child, model, request).await;
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}

/// [`ask`] for an `ask <model> with (request)` line that `script!` wrote at
/// `site`. Called by `script!`.
///
/// # Errors
///
/// As [`ask`], located at the step.
#[doc(hidden)]
pub async fn ask_at<T, M>(
    scope: &Scope,
    step: &str,
    site: &'static Site,
    model: &M,
    request: &AskRequest,
) -> Result<T, FlowError>
where
    T: super::AskDecode,
    M: ModelPort,
{
    let child = scope.enter_at(step, site)?;
    let outcome = ask_in(&child, model, request).await;
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}

/// The call itself, in the step `scope` already entered.
async fn ask_in<T, M>(scope: &Scope, model: &M, request: &AskRequest) -> Result<T, FlowError>
where
    T: super::AskDecode,
    M: ModelPort,
{
    if scope.justification().is_some() {
        Box::pin(scope.justified(0)).await?;
    }
    scope.require(&[Cap::new("bot.model")])?;
    if request.max_tokens == 0 {
        let refusal = Err(FlowError::InvalidBound {
            what: "the ask token budget",
            value: 0,
            max: u32::MAX.into(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "ask_in: returning an error to the caller");
        return refusal;
    }
    let answered = match request.deadline {
        Some(limit) if limit.is_zero() => {
            let refusal = Err(FlowError::InvalidBound {
                what: "the ask deadline",
                value: 0,
                max: u64::MAX,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "ask_in: returning an error to the caller");
            return refusal;
        }
        Some(limit) => {
            super::control::within(scope, "ask", limit, async {
                model
                    .complete(&request.prompt, request.max_tokens)
                    .await
                    .map_err(Into::into)
            })
            .await?
        }
        None => model
            .complete(&request.prompt, request.max_tokens)
            .await
            .map_err(Into::into)?,
    };
    if answered.truncated {
        let got = u64::saturating_from(answered.bytes.len());
        let refusal = Err(FlowError::InvalidBound {
            what: "the ask model output",
            value: got,
            max: request.max_tokens.into(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "ask_in: the model output was truncated");
        return refusal;
    }
    match T::decode(&answered.bytes) {
        Ok(typed) => Ok(typed),
        Err(failed) => {
            lgwks_std::trace::debug!(error = ?failed, "ask_in: the output did not decode");
            Err(failed)
        }
    }
}
