//! The app as a cell: route and `on` transitions over one tenant's state, and
//! the runner that answers requests and serves effects under the grants.

use std::sync::{Arc, Weak};

use super::{HostSlot, MAX_EFFECTS_PER_APPLY, Method, Provider};
use crate::cell::{
    Cell, CellHost, CellRefused, DynHost, Effect, EffectResult, EffectRunner, MAX_MESSAGE_BYTES,
    OnInput, ReplySlot, ReplyTag, Steps, first_reply,
};
use crate::ecs::Response;
use crate::gate::GrantSet;

/// A route transition over the raw body: decode, then fold.
type RouteFn<S> = Arc<dyn Fn(&mut S, &[u8]) -> Vec<Effect> + Send + Sync>;

/// An `on` transition: fold a provider's answer.
type OnFn<S> = Arc<dyn Fn(&mut S, &OnInput) -> Vec<Effect> + Send + Sync>;

/// One route: a verb, a path and its transition.
pub(super) struct RouteEntry<S> {
    /// The verb this route answers.
    pub(super) method: Method,
    /// The path below the tenant prefix, rooted at `/`.
    pub(super) path: Box<str>,
    /// Decode the body, then fold it into the tenant's state.
    pub(super) handler: RouteFn<S>,
}

impl<S> RouteEntry<S> {
    /// A route whose handler takes the body decoded as JSON `T`; a body that
    /// does not decode is answered `400` and never reaches `handler`.
    pub(super) fn new<T, F>(method: Method, path: Box<str>, handler: F) -> Self
    where
        T: lgwks_std::json::serde::de::DeserializeOwned + 'static,
        F: Fn(&mut S, T) -> Vec<Effect> + Send + Sync + 'static,
        S: 'static,
    {
        let decoded = move |state: &mut S, body: &[u8]| -> Vec<Effect> {
            let text: &[u8] = if body.is_empty() { b"null" } else { body };
            match lgwks_std::json::from_slice::<T>(text) {
                Ok(input) => handler(state, input),
                Err(error) => {
                    lgwks_std::trace::debug!(%error, "app: a body did not decode");
                    vec![Effect::reply(
                        400,
                        b"the body does not decode as this route's input".to_vec(),
                    )]
                }
            }
        };
        Self {
            method,
            path,
            handler: Arc::new(decoded),
        }
    }
}

/// One `on` handler: the tag it answers and its transition.
pub(super) struct OnEntry<S> {
    /// The tag of the effects whose answers arrive here.
    pub(super) tag: ReplyTag,
    /// Fold the answer into the tenant's state.
    pub(super) handler: OnFn<S>,
}

impl<S> OnEntry<S> {
    /// An `on` handler for `tag`.
    pub(super) fn new<F>(tag: ReplyTag, handler: F) -> Self
    where
        F: Fn(&mut S, &OnInput) -> Vec<Effect> + Send + Sync + 'static,
    {
        Self {
            tag,
            handler: Arc::new(handler),
        }
    }
}

/// One app message for one tenant: a request, or an effect's answer.
#[derive(Debug, Clone)]
pub(super) enum AppMessage {
    /// A request on `method` `path` with `body`.
    Route {
        /// Who the request is for.
        tenant: Box<str>,
        /// The verb.
        method: Method,
        /// The path below the tenant prefix.
        path: Box<str>,
        /// The body the edge read.
        body: Box<[u8]>,
    },
    /// The answer to an effect tagged `tag`.
    On {
        /// Who the answer is for.
        tenant: Box<str>,
        /// The answer, carrying its tag.
        input: OnInput,
    },
}

/// The app's cell: every route and `on` handler over one tenant's state.
pub(super) struct AppCell<S> {
    /// Routes in registration order.
    routes: Vec<RouteEntry<S>>,
    /// `on` handlers in registration order.
    ons: Vec<OnEntry<S>>,
}

impl<S> AppCell<S> {
    /// The cell over `routes` and `ons`.
    pub(super) fn new(routes: Vec<RouteEntry<S>>, ons: Vec<OnEntry<S>>) -> Self {
        Self { routes, ons }
    }
}

/// At most [`MAX_EFFECTS_PER_APPLY`] of `effects`, each clipped to its bounds.
fn bounded(mut effects: Vec<Effect>) -> Vec<Effect> {
    if effects.len() > MAX_EFFECTS_PER_APPLY {
        lgwks_std::trace::debug!(
            got = effects.len(),
            "app: a handler returned past its effect ceiling"
        );
        effects.truncate(MAX_EFFECTS_PER_APPLY);
    }
    effects.into_iter().map(Effect::bound).collect()
}

impl<S> Cell for AppCell<S>
where
    S: Clone + Default + Send + 'static,
{
    type State = S;
    type Message = AppMessage;

    fn tenant(message: &AppMessage) -> &str {
        match *message {
            AppMessage::Route { ref tenant, .. } | AppMessage::On { ref tenant, .. } => tenant,
        }
    }

    fn weight(message: &AppMessage) -> usize {
        match *message {
            AppMessage::Route {
                ref tenant,
                ref path,
                ref body,
                ..
            } => tenant
                .len()
                .saturating_add(path.len())
                .saturating_add(body.len()),
            AppMessage::On {
                ref tenant,
                ref input,
            } => {
                let answer = match *input.result() {
                    EffectResult::Value(ref bytes) => bytes.len(),
                    EffectResult::Refused(_) | EffectResult::Unavailable => 0,
                };
                tenant.len().saturating_add(answer)
            }
        }
    }

    fn apply(
        &self,
        state: &mut S,
        message: AppMessage,
        budget: &mut Steps,
    ) -> Result<Vec<Effect>, CellRefused> {
        budget.spend(1)?;
        match message {
            AppMessage::Route {
                method, path, body, ..
            } => match self
                .routes
                .iter()
                .find(|route| route.method == method && route.path == path)
            {
                Some(route) => Ok(bounded((route.handler)(state, &body))),
                None => Ok(vec![Effect::reply(404, b"no such route".to_vec())]),
            },
            AppMessage::On { input, .. } => {
                match self.ons.iter().find(|entry| entry.tag == *input.tag()) {
                    Some(entry) => Ok(bounded((entry.handler)(state, &input))),
                    None => {
                        lgwks_std::trace::debug!(
                            tag = input.tag().as_str(),
                            "app: no on handler names the tag"
                        );
                        Ok(Vec::new())
                    }
                }
            }
        }
    }
}

/// The app's runner: the first `Reply` answers the request; every other
/// effect is checked against the grants, served by the provider, and its
/// answer fed back through the tenant's mailbox.
pub(super) struct AppRunner<S: Clone + Default + Send + 'static> {
    /// The caps dispatch may run under.
    grants: GrantSet,
    /// Who serves the granted effects.
    provider: Arc<dyn Provider>,
    /// The host answers re-enter through.
    host: HostSlot<S>,
}

impl<S> AppRunner<S>
where
    S: Clone + Default + Send + 'static,
{
    /// A runner over `grants` and `provider`, bound to its host later.
    pub(super) fn new(grants: GrantSet, provider: Arc<dyn Provider>) -> Self {
        Self {
            grants,
            provider,
            host: HostSlot::new(),
        }
    }

    /// Feed answers back through `host`. Weak, so the runner the host owns
    /// never keeps the host alive.
    pub(super) fn bind(&self, host: &Arc<CellHost<AppCell<S>>>) {
        if self.host.set(Arc::downgrade(host)).is_err() {
            lgwks_std::trace::debug!("app: the runner was bound twice; the first host stands");
        }
    }

    /// Serve `effect` under the grants and feed its answer back for `tenant`.
    fn dispatch(&self, tenant: &str, effect: &Effect) {
        let Some(reply_to) = effect.reply_to().cloned() else {
            return;
        };
        let result = match effect.required_cap() {
            Some(needed) if !self.grants.grants(&needed) => {
                lgwks_std::trace::debug!(%needed, tenant, "app: an effect was denied its cap");
                EffectResult::Refused(needed)
            }
            _ => match self.provider.dispatch(tenant, effect) {
                Some(bytes) => EffectResult::Value(bytes.into_boxed_slice()),
                None => EffectResult::Unavailable,
            },
        };
        let Some(host) = self.host.get().and_then(Weak::upgrade) else {
            lgwks_std::trace::debug!(tenant, "app: the runner has no host to answer through");
            return;
        };
        let message = AppMessage::On {
            tenant: tenant.into(),
            input: OnInput::new(reply_to, result),
        };
        if <AppCell<S> as Cell>::weight(&message) > MAX_MESSAGE_BYTES {
            lgwks_std::trace::debug!(tenant, "app: a provider answer is past the host ceiling");
            return;
        }
        if let Err(refusal) = host.try_submit(tenant, message, Arc::new(ReplySlot::new())) {
            lgwks_std::trace::debug!(%refusal, tenant, "app: a provider answer was refused");
        }
    }
}

impl<S> EffectRunner for AppRunner<S>
where
    S: Clone + Default + Send + 'static,
{
    fn run(&self, tenant: &str, effects: Vec<Effect>) -> Option<Response> {
        first_reply(effects, |effect| self.dispatch(tenant, &effect))
    }
}
