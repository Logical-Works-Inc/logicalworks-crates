//! Refusing an app before it serves: bounds, duplicates and grants, and the
//! same checks for an app built from a model-written plan.

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::transition::RouteEntry;
use super::{
    App, AppError, MAX_EFFECTS_PER_APPLY, MAX_NAME_BYTES, MAX_PATH_BYTES, MAX_PLAN_BYTES,
    MAX_ROUTES, Method, NoProvider, Principal, normalise_path, refused,
};
use crate::cap::Cap;
use crate::cell::{Effect, EffectResult, OnInput};
use crate::gate::GrantSet;

/// The bodies a route is probed with: each JSON shape a handler's input can
/// take, so a handler's effects are seen without serving a request.
const ROUTE_PROBES: [&[u8]; 6] = [b"null", b"{}", b"[]", b"\"\"", b"0", b"true"];

/// The caps among `effects` that `grants` does not hold.
fn ungranted(grants: &GrantSet, effects: &[Effect]) -> Vec<Cap> {
    effects
        .iter()
        .take(MAX_EFFECTS_PER_APPLY)
        .filter_map(Effect::required_cap)
        .filter(|cap| !grants.grants(cap))
        .collect()
}

/// What [`App::check`] found missing, without repeats.
#[derive(Default)]
struct Missing {
    /// Routes that need a missing cap.
    routes: Vec<usize>,
    /// `on` handlers that need a missing cap.
    handlers: Vec<usize>,
    /// The distinct missing caps.
    caps: Vec<Cap>,
}

impl Missing {
    /// Record that `caps` are missing for `index` in `into`.
    fn note(&mut self, caps: Vec<Cap>, index: usize, route: bool) {
        if caps.is_empty() {
            return;
        }
        let indices = if route {
            &mut self.routes
        } else {
            &mut self.handlers
        };
        if !indices.contains(&index) {
            indices.push(index);
        }
        for cap in caps {
            if !self.caps.contains(&cap) {
                self.caps.push(cap);
            }
        }
    }
}

/// Run `transition` on a fresh state, catching a panic as no effects: a
/// handler that panics on a probe is contained by the host when it serves.
fn probe<S: Default>(transition: impl FnOnce(&mut S) -> Vec<Effect>) -> Vec<Effect> {
    match catch_unwind(AssertUnwindSafe(|| transition(&mut S::default()))) {
        Ok(effects) => effects,
        Err(_) => {
            lgwks_std::trace::debug!("app: a handler panicked on a check probe");
            Vec::new()
        }
    }
}

impl<S> App<S>
where
    S: Clone + Default + Send + 'static,
{
    /// Refuse the app before it serves.
    ///
    /// Every route is probed with each JSON shape and every `on` handler with
    /// an answer, a refusal and an unavailable result, on a fresh state. Each
    /// effect declares its cap, so the probes show which caps a handler
    /// reaches without dispatching anything. The runner also refuses an
    /// ungranted effect at dispatch, so a handler that hides an effect from
    /// every probe still cannot bypass a grant.
    ///
    /// # Errors
    ///
    /// [`AppError::BadName`], [`AppError::TooMany`],
    /// [`AppError::DuplicateRoute`], [`AppError::NoAuthenticator`] or
    /// [`AppError::MissingCaps`].
    pub fn check(&self) -> Result<(), AppError> {
        if self.name.is_empty() || self.name.len() > MAX_NAME_BYTES {
            return refused(AppError::BadName {
                len: self.name.len(),
            });
        }
        for (what, len) in [("routes", self.routes.len()), ("handlers", self.ons.len())] {
            if len > MAX_ROUTES {
                return refused(AppError::TooMany { what, len });
            }
        }
        for (second, route) in self.routes.iter().enumerate() {
            let earlier = self.routes.iter().take(second);
            if let Some(first) = earlier
                .enumerate()
                .find(|&(_, other)| other.method == route.method && other.path == route.path)
                .map(|(first, _)| first)
            {
                return refused(AppError::DuplicateRoute { first, second });
            }
        }
        if self.authenticator.is_none()
            && let Some(route) = self.routes.iter().position(|route| route.needs_principal)
        {
            return refused(AppError::NoAuthenticator { route });
        }
        let who = Principal::probe();
        let mut missing = Missing::default();
        for (index, route) in self.routes.iter().enumerate() {
            for body in ROUTE_PROBES {
                let effects = probe(|state: &mut S| (route.handler)(state, Some(&who), body));
                missing.note(ungranted(&self.grants, &effects), index, true);
            }
        }
        for (index, entry) in self.ons.iter().enumerate() {
            let results = [
                EffectResult::Value(Box::default()),
                EffectResult::Refused(Cap::new("app.probe")),
                EffectResult::Unavailable,
            ];
            for result in results {
                let input = OnInput::new(entry.tag.clone(), result);
                let effects = probe(|state: &mut S| (entry.handler)(state, &input));
                missing.note(ungranted(&self.grants, &effects), index, false);
            }
        }
        if missing.caps.is_empty() {
            return Ok(());
        }
        refused(AppError::MissingCaps {
            routes: missing.routes,
            handlers: missing.handlers,
            caps: missing.caps,
        })
    }

    /// Build an app from a plan a model wrote, through the same [`App::check`].
    ///
    /// The plan is JSON `{"name":"…","routes":[{"method":"POST","path":"/add",
    /// "status":200,"body":"…"}]}` of at most 64 KiB and 256 routes. Each route
    /// answers its fixed status and body and reaches no cap.
    ///
    /// # Errors
    ///
    /// [`AppError::PlanTooLarge`], [`AppError::MalformedPlan`], and whatever
    /// [`App::check`] refuses.
    pub fn from_plan(bytes: &[u8]) -> Result<Self, AppError> {
        if bytes.len() > MAX_PLAN_BYTES {
            return refused(AppError::PlanTooLarge { bytes: bytes.len() });
        }
        let plan: Plan = match lgwks_std::json::from_slice(bytes) {
            Ok(plan) => plan,
            Err(error) => {
                lgwks_std::trace::debug!(%error, "app: the plan does not decode");
                return refused(AppError::MalformedPlan { at: None });
            }
        };
        if plan.routes.len() > MAX_ROUTES {
            return refused(AppError::TooMany {
                what: "routes",
                len: plan.routes.len(),
            });
        }
        let mut routes = Vec::with_capacity(plan.routes.len());
        for (at, route) in plan.routes.into_iter().enumerate() {
            let method = Method::parse(&route.method);
            let well_formed = route.path.starts_with('/')
                && route.path.len() <= MAX_PATH_BYTES
                && (100..=599).contains(&route.status);
            let (Some(method), true) = (method, well_formed) else {
                return refused(AppError::MalformedPlan { at: Some(at) });
            };
            let (status, body) = (route.status, route.body.into_bytes());
            routes.push(RouteEntry::new(
                method,
                normalise_path(&route.path),
                move |_: &mut S, _: lgwks_std::json::Value| {
                    vec![Effect::reply(status, body.clone())]
                },
            ));
        }
        let app = Self {
            name: crate::cell::clip(&plan.name, MAX_NAME_BYTES.saturating_add(1)),
            grants: GrantSet::empty(),
            provider: std::sync::Arc::new(NoProvider),
            approvals: None,
            authenticator: None,
            commands: super::CommandLimits::default(),
            routes,
            ons: Vec::new(),
        };
        app.check()?;
        Ok(app)
    }
}

/// A model-written plan.
#[derive(lgwks_std::json::Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
struct Plan {
    /// The app name.
    name: String,
    /// Routes in order.
    routes: Vec<PlanRoute>,
}

/// One planned route.
#[derive(lgwks_std::json::Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
struct PlanRoute {
    /// `GET` or `POST`.
    method: String,
    /// Below the tenant prefix, rooted at `/`.
    path: String,
    /// The status the route answers.
    status: u16,
    /// The body the route answers.
    body: String,
}
