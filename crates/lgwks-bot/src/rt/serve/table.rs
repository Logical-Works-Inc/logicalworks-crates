//! The route table, built once when the serve starts (#1008).
//!
//! A request used to find its route by a scan of the route list, then build a
//! task for it: a name validated, a closure made, the handler, tenant and
//! content type cloned into it. The table does that work once per route. Each
//! entry owns its task, whose body takes the tenant and content type in its
//! input (see `ServeInput`) instead of capturing them, so one task serves every
//! tenant and every request. A request does one ordered-map lookup by path and
//! borrows the task through an `Arc`.
//!
//! The table is fixed at start: it grows only with the routes the serve was
//! given, and a second route on one verb and path is dropped, as the first match
//! won in the scan this replaces.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::ingress::{ServeInput, ServeOutput};
use super::route::RouteHandler;
use super::{RouteMethod, ServeError, TypedRoute};
use crate::script::{FlowError, Scope};
use crate::task::{Task, task};

/// The boxed answer of a route's task body.
type BodyFuture = Pin<Box<dyn Future<Output = Result<ServeOutput, FlowError>> + Send>>;

/// A route's task body: one function for every tenant and request.
type Body = Box<dyn Fn(Scope, ServeInput) -> BodyFuture + Send + Sync>;

/// The task one route runs.
pub(super) type RouteTask = Task<Body>;

/// The name every route's task carries; it labels the run's first step.
const ROUTE_TASK: &str = "serve-route";

/// The body of the task for `handler`: call it with the input's tenant, content
/// type and body, and fold its answer into the status and bytes to reply.
fn body_of(handler: Arc<RouteHandler>) -> Body {
    Box::new(move |_scope: Scope, input: ServeInput| {
        let handler = Arc::clone(&handler);
        let answered: BodyFuture = Box::pin(async move {
            let ServeInput {
                body,
                tenant,
                content_type,
            } = input;
            Ok(match handler(tenant, content_type, body).await {
                Ok(bytes) => ServeOutput {
                    status: 200,
                    body: bytes,
                },
                Err((status, message)) => ServeOutput {
                    status,
                    body: message.into_bytes(),
                },
            })
        });
        answered
    })
}

/// Every route's task, by verb and path.
#[derive(Default)]
pub(super) struct Table {
    /// The `GET` routes by path.
    get: BTreeMap<String, Arc<RouteTask>>,
    /// The `POST` routes by path.
    post: BTreeMap<String, Arc<RouteTask>>,
}

impl Table {
    /// Build the table from the routes the serve was given.
    ///
    /// # Errors
    ///
    /// [`ServeError::Host`] when a route's task is refused or its verb is one
    /// this edge cannot dispatch.
    pub(super) fn build(routes: Vec<TypedRoute>) -> Result<Self, ServeError> {
        let mut table = Self::default();
        for route in routes {
            let built = task(ROUTE_TASK, body_of(route.handler)).map_err(|cause| {
                let refusal = ServeError::Host {
                    reason: format!("the route task is refused: {cause}"),
                };
                lgwks_std::trace::debug!(error = ?refusal, "serve: the route table is refused");
                refusal
            })?;
            let by_path = match route.method {
                RouteMethod::Get => &mut table.get,
                RouteMethod::Post => &mut table.post,
                // `RouteMethod` is non-exhaustive: a verb this edge cannot
                // dispatch is refused at start, never answered as a 404 later.
                other => {
                    let refusal = ServeError::Host {
                        reason: format!("the route verb {other:?} is not served"),
                    };
                    lgwks_std::trace::debug!(error = ?refusal, "serve: the route table is refused");
                    return Err(refusal);
                }
            };
            by_path.entry(route.path).or_insert_with(|| Arc::new(built));
        }
        Ok(table)
    }

    /// The task for `verb` and `path`, when a route answers them.
    pub(super) fn find(&self, verb: &str, path: &str) -> Option<Arc<RouteTask>> {
        let by_path = match verb {
            "GET" => &self.get,
            "POST" => &self.post,
            _ => return None,
        };
        by_path.get(path).map(Arc::clone)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::btree_map::Entry;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use lgwks_std::seeded::Seeded;

    use super::super::ingress::ServeInput;
    use super::{RouteMethod, Table, TypedRoute};
    use crate::sim_stream::{RECEIPT_START, TempDir, fold};
    use crate::task::{Host, RequestKey, Submission};

    /// The routes a band serves: verb, path and the label its handler answers.
    /// The second `POST /b` repeats a path, so the first one must win.
    const ROUTES: [(RouteMethod, &str, &str, &str); 4] = [
        (RouteMethod::Get, "GET", "/a", "get-a"),
        (RouteMethod::Post, "POST", "/a", "post-a"),
        (RouteMethod::Post, "POST", "/b", "post-b"),
        (RouteMethod::Post, "POST", "/b", "second-b"),
    ];

    /// The tenants a band draws from.
    const TENANTS: usize = 3;

    /// The content types a band draws from; `None` is a request that sent none.
    const CONTENT: [Option<&str>; 3] = [None, Some("text/plain"), Some("application/json")];

    /// The bodies a band draws from.
    const BODIES: [&str; 2] = ["alpha", "beta"];

    /// A route whose handler answers `label|tenant|content type|body` and counts
    /// the runs in `runs`.
    fn labelled(
        method: RouteMethod,
        path: &str,
        label: &'static str,
        runs: &Arc<AtomicUsize>,
    ) -> TypedRoute {
        let runs = Arc::clone(runs);
        TypedRoute::new_with_content_type(
            method,
            path,
            move |tenant: String, content: Option<String>, body: Vec<u8>| {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    let text = String::from_utf8_lossy(body.as_slice()).into_owned();
                    Ok(format!("{label}|{tenant}|{content:?}|{text}").into_bytes())
                }
            },
        )
    }

    /// `Ok` when `agreed`, else the message `what` builds.
    fn agree(agreed: bool, what: impl FnOnce() -> String) -> Result<(), String> {
        agreed.then_some(()).ok_or_else(what)
    }

    /// One seeded draw below `bound`, as an index.
    fn draw(draws: &mut Seeded, bound: usize) -> Result<usize, String> {
        let bound =
            u64::try_from(bound).map_err(|refusal| format!("a bound is refused: {refusal}"))?;
        let drawn = draws
            .below(bound)
            .map_err(|refusal| format!("a draw is refused: {refusal}"))?;
        usize::try_from(drawn).map_err(|refusal| format!("a draw does not fit: {refusal}"))
    }

    /// The text a host build refusal reports.
    fn built<T, E: std::fmt::Display>(step: &str, outcome: Result<T, E>) -> Result<T, String> {
        outcome.map_err(|cause| format!("{step} is refused: {cause}"))
    }

    /// One band: seeded requests through the table's tasks and real hosts, each
    /// checked against the answer the drawn route, tenant, content type and body
    /// say. The receipt folds every answer, so one seed is one trace.
    fn run_band(seed: u64, requests: usize) -> Result<u64, String> {
        let runs = Arc::new(AtomicUsize::new(0));
        let routes = ROUTES
            .iter()
            .map(|&(method, _, path, label)| labelled(method, path, label, &runs))
            .collect();
        let table = built("the table", Table::build(routes))?;
        let store = built("the store dir", TempDir::new("serve-table"))?;
        let mut hosts: BTreeMap<usize, Host> = BTreeMap::new();
        let mut draws = Seeded::from_seed(seed);
        let mut receipt = RECEIPT_START;
        let mut expected_runs = 0_usize;
        for number in 0..requests {
            let tenant = draw(&mut draws, TENANTS)?;
            let route = draw(&mut draws, ROUTES.len())?;
            let content = draw(&mut draws, CONTENT.len())?;
            let body = draw(&mut draws, BODIES.len())?;
            let name = format!("t{tenant}");
            if let Entry::Vacant(vacant) = hosts.entry(tenant) {
                let builder = built("the host builder", Host::builder(&name))?;
                let builder = built("the run store", builder.run_store(store.path()))?;
                vacant.insert(built("the host", builder.build())?);
            }
            let host = hosts
                .get(&tenant)
                .ok_or_else(|| String::from("the tenant's host is missing"))?;
            let &(_, verb, path, _) = ROUTES
                .get(route)
                .ok_or_else(|| String::from("the route draw is outside the table"))?;
            let found = table
                .find(verb, path)
                .ok_or_else(|| format!("seed {seed}: {verb} {path} found no task"))?;
            // The first route on a verb and path answers it, so the duplicate's
            // label is never expected.
            let label = ROUTES
                .iter()
                .find(|entry| entry.1 == verb && entry.2 == path)
                .map(|entry| entry.3)
                .ok_or_else(|| String::from("the route has no label"))?;
            let sent = CONTENT.get(content).copied().flatten().map(str::to_owned);
            let text = BODIES
                .get(body)
                .copied()
                .ok_or_else(|| String::from("the body draw is outside the table"))?;
            let want = format!("{label}|{name}|{sent:?}|{text}");
            let key = built("the key", RequestKey::new(&format!("key-{number}")))?;
            let input = ServeInput {
                body: text.as_bytes().to_vec(),
                tenant: name,
                content_type: sent,
            };
            let first =
                crate::rt::runtime::block_on(host.submit(&key, found.as_ref(), input.clone()));
            expected_runs = expected_runs.saturating_add(1);
            let answered = match built("the submission", first)? {
                Submission::Executed(report) => report.output().cloned(),
                Submission::Reattached(_) | Submission::InFlight(_) => None,
            };
            let answered =
                answered.ok_or_else(|| format!("seed {seed}: request {number} did not run"))?;
            agree(
                answered.status == 200 && answered.body == want.as_bytes(),
                || {
                    format!(
                        "seed {seed}: request {number} answered {:?}, wanted {want:?}",
                        answered.body
                    )
                },
            )?;
            receipt = fold(receipt, answered.body.as_slice());
            // A repeat under the same key and body, from another content type:
            // the content type is no identity, so it reattaches to the first
            // answer and the handler does not run again.
            let next = draw(&mut draws, CONTENT.len())?;
            let other = ServeInput {
                content_type: CONTENT.get(next).copied().flatten().map(str::to_owned),
                ..input
            };
            let repeat = crate::rt::runtime::block_on(host.submit(&key, found.as_ref(), other));
            let replayed = match built("the repeat", repeat)? {
                Submission::Reattached(report) => report.output().cloned(),
                Submission::Executed(_) | Submission::InFlight(_) => None,
            };
            let replayed = replayed
                .ok_or_else(|| format!("seed {seed}: request {number} did not reattach"))?;
            agree(replayed == answered, || {
                format!(
                    "seed {seed}: request {number} replayed {:?}, not the first answer",
                    replayed.body
                )
            })?;
        }
        agree(runs.load(Ordering::SeqCst) == expected_runs, || {
            format!(
                "seed {seed}: {} handler runs, wanted {expected_runs}",
                runs.load(Ordering::SeqCst)
            )
        })?;
        Ok(receipt)
    }

    /// Requirement: a request finds exactly the route its verb and path name
    /// (#1008).
    ///
    /// Catches a table that swaps the verbs, answers a path under the wrong verb,
    /// answers an unsupported or differently cased verb, or finds a path no route
    /// has.
    #[test]
    fn a_request_finds_its_route_by_verb_and_path() -> Result<(), String> {
        let runs = Arc::new(AtomicUsize::new(0));
        let routes = ROUTES
            .iter()
            .map(|&(method, _, path, label)| labelled(method, path, label, &runs))
            .collect();
        let table = built("the table", Table::build(routes))?;
        for (verb, path, found) in [
            ("GET", "/a", true),
            ("POST", "/a", true),
            ("POST", "/b", true),
            ("GET", "/b", false),
            ("POST", "/c", false),
            ("PUT", "/a", false),
            ("DELETE", "/b", false),
            ("get", "/a", false),
        ] {
            agree(table.find(verb, path).is_some() == found, || {
                format!("{verb} {path}: found should be {found}")
            })?;
        }
        Ok(())
    }

    /// Requirement: one task per route serves every tenant, content type and body
    /// its own input, and the first route on a verb and path wins (#1008).
    ///
    /// Catches a task that keeps the first request's tenant or content type, a
    /// duplicate route that displaces the first, a content type that counts as the
    /// request's identity, and a repeat that runs the handler again.
    #[test]
    fn sim_one_route_task_serves_every_tenant_its_own_input() -> Result<(), String> {
        for seed in 0..4_u64 {
            run_band(0x5EED_1008_u64.wrapping_add(seed), 18)?;
        }
        Ok(())
    }

    /// Requirement: determinism; one seed, one receipt, and two seeds differ
    /// (#1008).
    #[test]
    fn sim_route_table_bands_replay_from_their_seed() -> Result<(), String> {
        let first = run_band(0x5EED_1008, 18)?;
        let again = run_band(0x5EED_1008, 18)?;
        let other = run_band(0x5EED_1009, 18)?;
        agree(first == again, || {
            format!("one seed traced {first:#x} then {again:#x}")
        })?;
        agree(first != other, || {
            format!("two seeds traced one receipt {first:#x}")
        })
    }
}
