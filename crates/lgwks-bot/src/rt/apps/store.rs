//! The admitted plan store: one registry of operations, one verdict per body.

// ── The store ────────────────────────────────────────────────────────────────

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::cap::Cap;
use crate::proposal::{Refusal as ProposalRefusal, Surface, SurfaceError};
use crate::script::{FlowError, Scope};

use super::limits::PlanLimits;
use super::refusal::{PlanRefusal, RefusalKind, order, refuse};

/// The most plans one tenant may hold.
const MAX_APPS_PER_TENANT: usize = 256;

/// The most plans the store holds for every tenant together.
const MAX_APPS_TOTAL: usize = 4_096;

/// One operation a host runs: its handler behind shared ownership.
type HandlerFn = Arc<
    dyn Fn(Scope, Vec<u8>) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, FlowError>> + Send>>
        + Send
        + Sync,
>;

/// One operation a host runs: its handler behind shared ownership.
#[derive(Clone)]
pub struct OpHandler {
    /// The host's real handler: scope and input bytes in, printed bytes out.
    inner: HandlerFn,
}

impl OpHandler {
    /// Run the operation with `input` in `scope`.
    pub async fn call(&self, scope: Scope, input: Vec<u8>) -> Result<Vec<u8>, FlowError> {
        (self.inner)(scope, input).await
    }
}

impl std::fmt::Debug for OpHandler {
    /// The handler, unattributed: the plan names it, not this line.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OpHandler(..)")
    }
}

/// One registered operation: what it needs and what runs it.
struct OpEntry {
    /// The capabilities the run must already hold.
    requires: Vec<Cap>,
    /// The host's real handler.
    handler: OpHandler,
}

/// The operations one host runs, by name.
#[derive(Default)]
pub struct OpRegistry {
    /// The registered operations, keyed by name.
    ops: BTreeMap<String, OpEntry>,
}

impl OpRegistry {
    /// A registry holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `name` needing `requires` and running `handler`.
    ///
    /// The names admitted and the names runnable are one set: a plan can
    /// name exactly what its host registered, and every registered operation
    /// carries the handler the run calls.
    ///
    /// # Errors
    ///
    /// [`RegistryError`] when `name` is empty, too long, or holds a byte
    /// outside ASCII letters, digits, `-_.:`.
    pub fn operation<F, Fut>(
        mut self,
        name: &str,
        requires: &[Cap],
        handler: F,
    ) -> Result<Self, RegistryError>
    where
        F: Fn(Scope, Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<u8>, FlowError>> + Send + 'static,
    {
        if !valid_operation_name(name) {
            lgwks_std::trace::debug!(name, "apps: the operation name is refused");
            return Err(RegistryError::invalid(name));
        }
        let handler = OpHandler {
            inner: Arc::new(move |scope: Scope, input: Vec<u8>| {
                let answered = handler(scope, input);
                let boxed: Pin<Box<dyn Future<Output = Result<Vec<u8>, FlowError>> + Send>> =
                    Box::pin(answered);
                boxed
            }),
        };
        self.ops.insert(
            name.to_owned(),
            OpEntry {
                requires: requires.to_vec(),
                handler,
            },
        );
        Ok(self)
    }

    /// The handler registered under `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<OpHandler> {
        self.ops.get(name).map(|entry| entry.handler.clone())
    }

    /// The surface `tenant` holding `held` admits against: exactly the
    /// registered operations with the capabilities each needs.
    ///
    /// # Errors
    ///
    /// [`SurfaceError`] when the tenant name or an operation name is refused.
    pub fn surface(&self, tenant: &str, held: &[Cap]) -> Result<Surface, SurfaceError> {
        let mut builder = Surface::builder(tenant)?.holding(held);
        for (name, entry) in &self.ops {
            builder = builder.operation(name.as_str(), entry.requires.as_slice())?;
        }
        Ok(builder.build())
    }
}

/// Whether `name` is a usable operation name.
fn valid_operation_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= crate::proposal::MAX_FIELD_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

/// Why an operation could not be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError {
    /// What went wrong.
    detail: String,
}

impl RegistryError {
    /// `name` is not a usable operation name.
    fn invalid(name: &str) -> Self {
        Self {
            detail: format!(
                "{name:?} is not an ASCII operation name of 1..={} bytes",
                crate::proposal::MAX_FIELD_BYTES
            ),
        }
    }
}

impl std::fmt::Display for RegistryError {
    /// The detail.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.detail.as_str())
    }
}

impl std::error::Error for RegistryError {}

/// One operation the plan named, before execution names are assigned.
pub(crate) struct DraftOp {
    /// The operation as the plan wrote it.
    display: String,
    /// Where the plan earned a refusal for it.
    location: String,
    /// The ordering key.
    sort: String,
    /// Whether this operation came from a graph `node`.
    node: bool,
    /// The declared needs, attached from the tree walk.
    needs: Vec<String>,
}

/// What the bounds walk measures.
#[derive(Default)]
pub(crate) struct Stats {
    /// Steps at every level together.
    count: usize,
    /// The deepest nesting.
    depth: usize,
    /// The widest fan-out step.
    fanout: usize,
}

/// One step as the bounds walk sees it, from either body form.
pub(crate) struct View {
    /// The operation, when the step names one.
    pub(crate) display: Option<String>,
    /// Whether the operation came from a graph `node`.
    pub(crate) node: bool,
    /// Whether the step fans out (`each`, `graph`, `together`).
    pub(crate) fan: bool,
    /// `line:col` for text, a JSON pointer for JSON.
    pub(crate) location: String,
    /// The ordering key.
    pub(crate) sort: String,
    /// The nested steps.
    pub(crate) children: Vec<View>,
}

/// Walk `views` without recursion: bounds in `stats`, operations in `ops`.
pub(crate) fn walk(views: &[View], stats: &mut Stats, ops: &mut Vec<DraftOp>) {
    let mut stack: Vec<(&View, usize)> = views.iter().map(|view| (view, 1)).collect();
    while let Some((view, depth)) = stack.pop() {
        stats.count = stats.count.saturating_add(1);
        stats.depth = stats.depth.max(depth);
        if view.fan {
            stats.fanout = stats.fanout.max(view.children.len());
        }
        if let Some(ref display) = view.display {
            ops.push(DraftOp {
                display: display.clone(),
                location: view.location.clone(),
                sort: view.sort.clone(),
                node: view.node,
                needs: Vec::new(),
            });
        }
        for child in view.children.iter().rev() {
            stack.push((child, depth.saturating_add(1)));
        }
    }
}

/// One stored operation, execution-ready and freed with its plan.
#[derive(Clone, Debug)]
pub(crate) struct StoredOp {
    /// The unique execution name.
    pub(crate) exec: Arc<str>,
    /// The input bytes the handler runs on: the operation's name.
    pub(crate) input: Vec<u8>,
    /// The execution names this operation waits for.
    pub(crate) needs: Arc<[Arc<str>]>,
    /// The host's real handler.
    pub(crate) handler: OpHandler,
}

/// One stored plan, keyed by `(tenant, id)`.
#[derive(Clone, Debug)]
pub(crate) struct StoredPlan {
    /// The operations, in source order.
    pub(crate) ops: Vec<StoredOp>,
}

/// One operation the plan named and the run will execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedOp {
    /// The operation, unique across the plan.
    name: String,
}

impl AdmittedOp {
    /// The operation, unique across the plan.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }
}

/// A plan that passed every gate, waiting to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
    /// The digest of the exact bytes admitted, in hex.
    id: String,
    /// The tenant whose surface admitted it.
    tenant: String,
    /// The operations, in source order.
    operations: Vec<AdmittedOp>,
}

impl Admitted {
    /// The digest of the exact bytes admitted, in hex.
    #[must_use]
    pub fn id(&self) -> &str {
        self.id.as_str()
    }

    /// The tenant whose surface admitted it.
    #[must_use]
    pub fn tenant(&self) -> &str {
        self.tenant.as_str()
    }

    /// The operations, in source order.
    #[must_use]
    pub fn operations(&self) -> &[AdmittedOp] {
        self.operations.as_slice()
    }
}

/// A plan that did not become work, with everything it tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// Every refusal, sorted by location, kind and message.
    refusals: Vec<PlanRefusal>,
}

impl Refused {
    /// Every refusal, sorted by location, kind and message.
    #[must_use]
    pub fn refusals(&self) -> &[PlanRefusal] {
        self.refusals.as_slice()
    }
}

/// What admitting one plan body produced.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// The plan became work.
    Admitted(Admitted),
    /// The plan did not, with everything it tried.
    Refused(Refused),
}

impl Outcome {
    /// The admitted plan, when this outcome is one.
    #[must_use]
    pub const fn admitted(&self) -> Option<&Admitted> {
        match *self {
            Self::Admitted(ref admitted) => Some(admitted),
            Self::Refused(_) => None,
        }
    }

    /// The refusals, sorted, when this outcome is one.
    #[must_use]
    pub fn refusals(&self) -> &[PlanRefusal] {
        match *self {
            Self::Admitted(_) => &[],
            Self::Refused(ref refused) => refused.refusals(),
        }
    }

    /// The verdict as the edge answers it: `{"app": "<id>"}` or the full
    /// refusal list. Deterministic: the same plan and surface give
    /// byte-identical verdicts.
    #[must_use]
    pub fn verdict_json(&self) -> Vec<u8> {
        super::routes::encode_verdict(self)
    }

    /// The status the edge answers this verdict with: `200` admitted, `413`
    /// for a body past its ceiling, `400` for a body that is not a plan,
    /// `500` when the store is unavailable, and `422` for a plan the surface
    /// refuses.
    #[must_use]
    pub fn http_status(&self) -> u16 {
        match *self {
            Self::Admitted(_) => 200,
            Self::Refused(ref refused) => {
                if refused
                    .refusals()
                    .iter()
                    .any(|refusal| refusal.kind() == RefusalKind::StoreUnavailable)
                {
                    500
                } else if refused.refusals().iter().all(PlanRefusal::is_over_body) {
                    413
                } else if refused
                    .refusals()
                    .iter()
                    .all(|refusal| refusal.kind() == RefusalKind::Malformed)
                {
                    400
                } else {
                    422
                }
            }
        }
    }
}

/// The admitted plans every tenant holds, bounded per tenant and in total.
pub struct AppStore {
    /// The ceilings admissions are measured against.
    limits: PlanLimits,
    /// The plans by `(tenant, id)`.
    inner: Mutex<HashMap<(String, String), StoredPlan>>,
}

impl AppStore {
    /// An empty store admitting against `limits`.
    #[must_use]
    pub fn new(limits: PlanLimits) -> Self {
        Self {
            limits,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// The plan admitted under `(tenant, id)`, if the tenant holds one.
    ///
    /// A poisoned lock is a typed `Err`, logged where it is met: a store a
    /// panicking holder left behind is not silently reused.
    pub(crate) fn plan(&self, tenant: &str, id: &str) -> Result<Option<StoredPlan>, ()> {
        match self.inner.lock() {
            Ok(inner) => Ok(inner.get(&(tenant.to_owned(), id.to_owned())).cloned()),
            Err(_) => Err(()),
        }
    }

    /// Admit `body` as `script!` text against `surface`, running the
    /// handlers `registry` holds.
    #[must_use]
    pub fn admit_text(&self, surface: &Surface, registry: &OpRegistry, body: &[u8]) -> Outcome {
        match super::admit_text::parse_text(body, self.limits) {
            Err(refusal) => Outcome::Refused(Refused {
                refusals: vec![refusal],
            }),
            Ok(walked) => self.finish(surface, registry, body, walked),
        }
    }

    /// Admit `body` as JSON of the same tree against `surface`, running the
    /// handlers `registry` holds.
    #[must_use]
    pub fn admit_json(&self, surface: &Surface, registry: &OpRegistry, body: &[u8]) -> Outcome {
        match super::admit_json::parse_json(body, self.limits) {
            Err(refusal) => Outcome::Refused(Refused {
                refusals: vec![refusal],
            }),
            Ok(walked) => self.finish(surface, registry, body, walked),
        }
    }

    /// Authorize the walked plan, store it, and answer the verdict.
    fn finish(
        &self,
        surface: &Surface,
        registry: &OpRegistry,
        body: &[u8],
        walked: Walked,
    ) -> Outcome {
        let mut ops = walked.ops;
        let stats = walked.stats;
        let edges = walked.edges;
        let mut refusals = Vec::new();
        let tenant = surface.tenant().to_owned();
        if stats.count > self.limits.max_steps() {
            refusals.push(PlanRefusal::at(
                String::from("1:1"),
                order(1, 1),
                RefusalKind::BoundExceeded,
                format!(
                    "the plan holds {} steps, past the ceiling of {}",
                    stats.count,
                    self.limits.max_steps()
                ),
            ));
        }
        if stats.depth > self.limits.max_depth() {
            refusals.push(PlanRefusal::at(
                String::from("1:1"),
                order(1, 1),
                RefusalKind::BoundExceeded,
                format!(
                    "the plan nests {} deep, past the ceiling of {}",
                    stats.depth,
                    self.limits.max_depth()
                ),
            ));
        }
        if stats.fanout > self.limits.max_fan_out() {
            refusals.push(PlanRefusal::at(
                String::from("1:1"),
                order(1, 1),
                RefusalKind::BoundExceeded,
                format!(
                    "a step fans out to {}, past the ceiling of {}",
                    stats.fanout,
                    self.limits.max_fan_out()
                ),
            ));
        }
        if ops.is_empty() {
            refusals.push(PlanRefusal::at(
                String::from("1:1"),
                order(1, 1),
                RefusalKind::Malformed,
                String::from("the plan names no operation"),
            ));
        }
        for draft in &ops {
            if let Err(refusal) = authorize_op(surface, registry, draft) {
                refusals.push(refusal);
            }
        }
        refusals.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
        if !refusals.is_empty() {
            lgwks_std::trace::debug!(refusals = refusals.len(), "apps: the plan is refused");
            return Outcome::Refused(Refused { refusals });
        }
        attach_edges(&mut ops, &edges);
        let execs = assign_names(&ops);
        let id = lgwks_std::hash::blake3(body).to_hex();
        let key = (tenant.clone(), id.clone());
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "apps: the plan store lock is poisoned");
                return Outcome::Refused(Refused {
                    refusals: vec![PlanRefusal::unavailable()],
                });
            }
        };
        if !inner.contains_key(&key) {
            if inner.len() >= MAX_APPS_TOTAL {
                lgwks_std::trace::debug!("apps: the store is full");
                return Outcome::Refused(Refused {
                    refusals: vec![PlanRefusal::at(
                        String::from("1:1"),
                        order(1, 1),
                        RefusalKind::BoundExceeded,
                        format!(
                            "the store holds {} plans, past the ceiling of {MAX_APPS_TOTAL}",
                            inner.len()
                        ),
                    )],
                });
            }
            let held = inner
                .keys()
                .filter(|key| key.0.as_str() == tenant.as_str())
                .count();
            if held >= MAX_APPS_PER_TENANT {
                lgwks_std::trace::debug!(
                    tenant = tenant.as_str(),
                    "apps: the tenant holds too many plans"
                );
                return Outcome::Refused(Refused {
                    refusals: vec![PlanRefusal::at(
                        String::from("1:1"),
                        order(1, 1),
                        RefusalKind::BoundExceeded,
                        format!(
                            "tenant {tenant:?} holds {held} plans, past the ceiling of {MAX_APPS_PER_TENANT}"
                        ),
                    )],
                });
            }
            inner.insert(
                key,
                StoredPlan {
                    ops: lower(&ops, &execs, registry),
                },
            );
        }
        Outcome::Admitted(Admitted {
            id,
            tenant,
            operations: ops
                .iter()
                .map(|draft| AdmittedOp {
                    name: draft.display.clone(),
                })
                .collect(),
        })
    }
}

/// The walked plan both body forms hand to admission.
pub(crate) struct Walked {
    /// The drafted operations, in source order.
    pub(crate) ops: Vec<DraftOp>,
    /// The declared graph edges: node names with their needs.
    pub(crate) edges: Vec<(String, Vec<String>)>,
    /// What the walk measured.
    pub(crate) stats: Stats,
}

/// Authorize one drafted operation against the tenant's surface and registry.
///
/// The surface decides the authority and the registry holds the handler, so
/// a name the surface admits always has a handler to run, and a name with
/// no handler is refused rather than admitted without one.
fn authorize_op(
    surface: &Surface,
    registry: &OpRegistry,
    draft: &DraftOp,
) -> Result<(), PlanRefusal> {
    match surface.authorize(draft.display.as_str()) {
        Ok(_) => {}
        Err(ProposalRefusal::UnknownOperation { name }) => {
            return refuse(named(
                draft,
                RefusalKind::UnknownOperation,
                format!(
                    "unknown operation {name:?}: tenant {:?} registers no such operation",
                    surface.tenant()
                ),
            ));
        }
        Err(ProposalRefusal::CapabilityNotHeld {
            operation,
            required,
        }) => {
            return refuse(named(
                draft,
                RefusalKind::MissingCapability,
                format!(
                    "operation {operation:?} needs {required:?}, which tenant {:?} was not granted",
                    surface.tenant()
                ),
            ));
        }
        Err(other) => {
            return refuse(named(
                draft,
                RefusalKind::Malformed,
                format!("the surface refused {}: {other}", other.label()),
            ));
        }
    }
    if registry.get(draft.display.as_str()).is_none() {
        return refuse(named(
            draft,
            RefusalKind::UnknownOperation,
            format!(
                "unknown operation {:?}: no handler is registered for it",
                draft.display
            ),
        ));
    }
    Ok(())
}

/// A refusal for `draft` of `kind` with `message`.
fn named(draft: &DraftOp, kind: RefusalKind, message: String) -> PlanRefusal {
    PlanRefusal::at(draft.location.clone(), draft.sort.clone(), kind, message)
}

/// Unique execution names in order: the display, then suffixed.
fn assign_names(ops: &[DraftOp]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut execs = Vec::with_capacity(ops.len());
    for draft in ops {
        let used = seen.entry(draft.display.as_str()).or_insert(0);
        *used = used.saturating_add(1);
        if *used == 1 {
            execs.push(draft.display.clone());
        } else {
            execs.push(format!("{}~{}", draft.display, *used));
        }
    }
    execs
}

/// Attach declared needs to the drafted operations.
fn attach_edges(ops: &mut [DraftOp], edges: &[(String, Vec<String>)]) {
    for edge in edges.iter() {
        let node = edge.0.as_str();
        let needs = edge.1.clone();
        if let Some(draft) = ops
            .iter_mut()
            .find(|draft| draft.node && draft.display.as_str() == node)
        {
            draft.needs.clone_from(&needs);
        }
    }
}

/// Lower drafted operations into execution-ready stored operations.
///
/// The input each handler runs on is the operation's own name: plan data,
/// not a value invented here. The stored plan owns every name it runs
/// under, so dropping the plan frees them.
fn lower(ops: &[DraftOp], execs: &[String], registry: &OpRegistry) -> Vec<StoredOp> {
    let mut named: HashMap<&str, usize> = HashMap::new();
    for (index, draft) in ops.iter().enumerate() {
        if draft.node {
            named.entry(draft.display.as_str()).or_insert(index);
        }
    }
    let mut stored = Vec::with_capacity(ops.len());
    for (draft, exec) in ops.iter().zip(execs.iter()) {
        let mut needs = Vec::with_capacity(draft.needs.len());
        for need in &draft.needs {
            if let Some(&at) = named.get(need.as_str())
                && let Some(name) = execs.get(at)
            {
                needs.push(Arc::from(name.as_str()));
            }
        }
        if let Some(handler) = registry.get(draft.display.as_str()) {
            stored.push(StoredOp {
                exec: Arc::from(exec.as_str()),
                input: draft.display.clone().into_bytes(),
                needs: Arc::from(needs),
                handler,
            });
        }
    }
    stored
}
