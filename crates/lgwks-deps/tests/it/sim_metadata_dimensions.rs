//! Deterministic seeded simulation of `lgwks_deps::metadata::parse` over
//! generated `cargo metadata --format-version 1` documents (issue #158 A2/A3).
//!
//! One `u64` seed drives the document: how many members exist, which authored
//! dependency dimension each varies, and, in the fault families, which structural
//! inconsistency or byte damage Cargo's answer carries. Each scenario is modelled
//! independently of the parser, and `metadata::parse` must agree with the model
//! for every seed; a failure prints its seed. The generator is the shared
//! splitmix64: no new dependency, no wall clock, no OS entropy.

use std::error::Error;

use lgwks_deps::metadata::{self, DirectEdge, MetadataError};

use crate::sim::{Rng, Trace, receipt};

/// What every generator and test returns, so a draw a generator could not make
/// reaches the test that owns the seed as that draw's own refusal.
type Outcome<T> = Result<T, Box<dyn Error>>;

/// What every test in this suite returns.
type TestResult = Outcome<()>;

/// The one registry origin the generated documents name.
const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// Documents Cargo would never emit, each wrong in one field.
const MALFORMED: [&str; 6] = [
    r#"{"packages":123,"workspace_members":[]}"#,
    r#"{"packages":"nope","workspace_members":[]}"#,
    r#"{"packages":[],"workspace_members":{}}"#,
    r#"{"packages":[],"workspace_members":"later"}"#,
    r#"{"packages":[{"id":"a","name":"a","manifest_path":"/repo/Cargo.toml","dependencies":123}],"workspace_members":["a"]}"#,
    r#"{"packages":[{"id":"a","name":"a","manifest_path":"/repo/Cargo.toml","dependencies":[{"name":"dep","source":"registry+https://example.invalid/index","req":"1.0","optional":"yes"}]}],"workspace_members":["a"]}"#,
];

/// Every authored dependency dimension the general generator can vary.
const ALL_DIMS: &[Dim] = &[
    Dim::Kind,
    Dim::Target,
    Dim::Optional,
    Dim::DefaultFeatures,
    Dim::Features,
    Dim::Rename,
    Dim::Path,
];

/// The member counts a scenario draws: every one a workspace can have.
const MEMBER_COUNTS: [usize; 6] = [1, 2, 3, 4, 5, 6];

/// The dependency kinds the generator draws, every one of them a Cargo kind.
const KINDS: [&str; 3] = ["normal", "build", "dev"];

/// The names a member path edge may carry while its member record says otherwise.
const MISMATCHED_NAMES: [&str; 2] = ["renamed", "helper_copy"];

/// The manifest directories two members may claim together.
const SHARED_DIRS: [&str; 3] = ["/repo/shared", "/repo/common", "/repo/dup"];

/// The shares, in eighths, a truncation cuts at: always strictly inside.
const CUT_SHARES: [usize; 7] = [1, 2, 3, 4, 5, 6, 7];

/// How many shares a document is divided into.
const EIGHTH: usize = 8;

/// The cut offset a drawn share names in a document of `length` bytes, or `None`
/// for a document too short to have an eighth to cut at.
fn cut_offset(rng: &mut Rng, length: usize) -> Option<usize> {
    let share = *rng.pick(&CUT_SHARES)?;
    Some(share.saturating_mul(length.checked_div(EIGHTH)?))
}

/// One authored dependency dimension a member's declaration varies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dim {
    /// The `kind` (normal / build / dev).
    Kind,
    /// The `target` cfg scope.
    Target,
    /// The `optional` bit.
    Optional,
    /// The `default-features` bit.
    DefaultFeatures,
    /// The enabled `features` list.
    Features,
    /// The local `package =` rename.
    Rename,
    /// A `path` edge (internal member or external directory).
    Path,
}

/// A dependency edge as the model predicts it, in exactly the fields `parse`
/// returns through the public accessors.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ModeledEdge {
    /// Workspace package declaring the edge.
    consumer: String,
    /// Upstream package identity.
    package: String,
    /// Authored semver requirement.
    requirement: String,
    /// `normal`, `build`, or `dev`.
    kind: &'static str,
    /// `registry`, `git`, `path`, or `other`.
    class: &'static str,
    /// The exact Cargo source or path string.
    detail: String,
    /// Whether the declaration is optional.
    optional: bool,
    /// Whether the target is a workspace member.
    workspace: bool,
    /// Repository of a workspace path target, when it declares one.
    target_repository: Option<String>,
    /// Enabled features, in emitted order.
    features: Vec<String>,
    /// Whether default features are enabled.
    uses_default_features: bool,
    /// Target cfg scope.
    target: Option<String>,
    /// Local rename alias.
    rename: Option<String>,
}

/// The independently modelled outcome of one generated document.
#[derive(Debug)]
enum Model {
    /// The document decodes to exactly these edges, sorted.
    Edges(Vec<ModeledEdge>),
    /// The document is refused with a schema message containing this fragment.
    Schema(&'static str),
    /// The document is refused as invalid JSON.
    Json,
}

/// A generated document and the model's expectation for it.
struct Scenario {
    /// The bytes handed to `parse`.
    document: String,
    /// The expected outcome.
    model: Model,
}

/// What `parse` actually returned, normalized for comparison with the model.
#[derive(Debug)]
enum Observed {
    /// A decoded edge set, sorted.
    Edges(Vec<ModeledEdge>),
    /// A schema refusal carrying the message.
    Schema(String),
    /// A JSON decode refusal.
    Json,
    /// A refusal shape `parse` should never produce.
    Unexpected,
}

/// Projects one decoded edge into the model's comparable shape.
fn modeled(edge: &DirectEdge) -> ModeledEdge {
    ModeledEdge {
        consumer: edge.consumer().to_owned(),
        package: edge.package().to_owned(),
        requirement: edge.requirement().to_owned(),
        kind: edge.kind().as_str(),
        class: edge.source().class(),
        detail: edge.source().detail().to_owned(),
        optional: edge.optional(),
        workspace: edge.workspace(),
        target_repository: edge.target_repository().map(str::to_owned),
        features: edge.features().to_vec(),
        uses_default_features: edge.uses_default_features(),
        target: edge.target().map(str::to_owned),
        rename: edge.rename().map(str::to_owned),
    }
}

/// Normalizes a `parse` result into the model's comparable shape.
fn observe(result: Result<Vec<DirectEdge>, MetadataError>) -> Observed {
    match result {
        Ok(edges) => {
            let mut projected: Vec<ModeledEdge> = edges.iter().map(modeled).collect();
            projected.sort();
            Observed::Edges(projected)
        }
        Err(MetadataError::Json(_)) => Observed::Json,
        Err(MetadataError::Schema(message)) => Observed::Schema(message),
        Err(_) => Observed::Unexpected,
    }
}

/// The policy class a Cargo `source` string names.
fn class_of(source: &str) -> &'static str {
    if source.starts_with("registry+") || source.starts_with("sparse+") {
        "registry"
    } else if source.starts_with("git+") {
        "git"
    } else {
        "other"
    }
}

/// The model's kind spelling for a Cargo `kind` value.
fn kind_of(kind: Option<&str>) -> &'static str {
    match kind {
        Some("build") => "build",
        Some("dev") => "dev",
        _ => "normal",
    }
}

/// One authored dependency declaration in the generated document.
#[derive(Clone)]
struct DepPlan {
    /// Upstream package name.
    name: String,
    /// Cargo `source` string, or `None` for a path edge.
    source: Option<String>,
    /// Manifest semver requirement.
    req: String,
    /// `kind` spelling, or `None` for Cargo's normal default.
    kind: Option<&'static str>,
    /// Whether the declaration is optional.
    optional: bool,
    /// Manifest path for a path edge.
    path: Option<String>,
    /// Enabled features, in emitted order.
    features: Vec<String>,
    /// Whether default features are on.
    defaults: bool,
    /// Target `cfg` scope, when the declaration is target-specific.
    scope: Option<String>,
    /// Local `package =` alias, when the manifest renamed the edge.
    alias: Option<String>,
}

impl DepPlan {
    /// A registry declaration with Cargo's defaults.
    fn registry(name: String) -> Self {
        Self {
            name,
            source: Some(REGISTRY.to_owned()),
            req: "1.0".to_owned(),
            kind: None,
            optional: false,
            path: None,
            features: Vec::new(),
            defaults: true,
            scope: None,
            alias: None,
        }
    }
}

/// One workspace member in the generated document.
struct MemberPlan {
    /// Cargo package id.
    id: String,
    /// Cargo package name.
    name: String,
    /// Declared repository URL, when any.
    repository: Option<String>,
    /// Manifest path, or `None` to omit the key entirely.
    manifest_path: Option<String>,
}

impl MemberPlan {
    /// A member whose id is its name, with no repository, at `manifest_path`.
    fn bare(name: &str, manifest_path: String) -> Self {
        Self {
            id: name.to_owned(),
            name: name.to_owned(),
            repository: None,
            manifest_path: Some(manifest_path),
        }
    }
}

/// A member's name and the repository it declares, as the generator drew them.
struct Named {
    /// The member's package name.
    name: String,
    /// The repository it declares, when any.
    repository: Option<String>,
}

/// Renders an optional JSON string value.
fn json_string(value: Option<&str>) -> String {
    match value {
        Some(text) => format!("\"{text}\""),
        None => "null".to_owned(),
    }
}

/// Renders one dependency declaration as Cargo JSON.
fn dep_json(dep: &DepPlan) -> String {
    let source = json_string(dep.source.as_deref());
    let kind = json_string(dep.kind);
    let path = json_string(dep.path.as_deref());
    let target = json_string(dep.scope.as_deref());
    let rename = json_string(dep.alias.as_deref());
    let quoted: Vec<String> = dep
        .features
        .iter()
        .map(|feature| format!("\"{feature}\""))
        .collect();
    let features = quoted.join(",");
    let (name, req, optional, defaults) = (&dep.name, &dep.req, dep.optional, dep.defaults);
    format!(
        "{{\"name\":\"{name}\",\"source\":{source},\"req\":\"{req}\",\"kind\":{kind},\"optional\":{optional},\"path\":{path},\"features\":[{features}],\"uses_default_features\":{defaults},\"target\":{target},\"rename\":{rename}}}"
    )
}

/// Renders one package record, omitting `manifest_path` when the plan has none.
fn member_json(member: &MemberPlan, deps: &[DepPlan]) -> String {
    let repository = json_string(member.repository.as_deref());
    let rendered: Vec<String> = deps.iter().map(dep_json).collect();
    let dependencies = rendered.join(",");
    let manifest = match member.manifest_path {
        Some(ref path) => format!(",\"manifest_path\":\"{path}\""),
        None => String::new(),
    };
    let (id, name) = (&member.id, &member.name);
    format!(
        "{{\"id\":\"{id}\",\"name\":\"{name}\",\"repository\":{repository},\"dependencies\":[{dependencies}]{manifest}}}"
    )
}

/// Assembles a metadata document from package records and member ids.
fn document(packages: &[String], members: &[String]) -> String {
    let joined_packages = packages.join(",");
    let quoted: Vec<String> = members
        .iter()
        .map(|identifier| format!("\"{identifier}\""))
        .collect();
    let joined_members = quoted.join(",");
    format!("{{\"packages\":[{joined_packages}],\"workspace_members\":[{joined_members}]}}")
}

/// A generated valid workspace: the records to serialize, the plans behind
/// them, the member ids, and the model edge each member decodes to.
struct Workspace {
    /// Serialized package records, one per member, in member order.
    records: Vec<String>,
    /// The member/declaration plans, for faults that re-serialize.
    plans: Vec<(MemberPlan, DepPlan)>,
    /// Member package ids, referenced by `workspace_members`.
    ids: Vec<String>,
    /// The modelled edge each member authors.
    edges: Vec<ModeledEdge>,
}

impl Workspace {
    /// The document bytes for the current records and ids.
    fn document(&self) -> String {
        document(&self.records, &self.ids)
    }

    /// Applies `edit` to the first member and its declaration, then
    /// re-serializes every record.
    fn edit_first(&mut self, edit: impl FnOnce(&mut MemberPlan, &mut DepPlan)) {
        if let Some(pair) = self.plans.first_mut() {
            edit(&mut pair.0, &mut pair.1);
        }
        self.records = self
            .plans
            .iter()
            .map(|pair| member_json(&pair.0, std::slice::from_ref(&pair.1)))
            .collect();
    }
}

/// Builds the member at `index` varying `dim`, with the model edge it decodes to.
fn build_member(
    index: usize,
    own: &Named,
    dim: Dim,
    members: &[Named],
    rng: &mut Rng,
) -> Outcome<(MemberPlan, DepPlan, ModeledEdge)> {
    let consumer = own.name.clone();
    let member = MemberPlan {
        id: format!("path+file:///repo/{consumer}#{consumer}@0.1.0"),
        name: consumer.clone(),
        repository: own.repository.clone(),
        manifest_path: Some(format!("/repo/{consumer}/Cargo.toml")),
    };
    let mut dep = DepPlan::registry(format!("dep{index}"));
    match dim {
        Dim::Kind => dep.kind = Some(*rng.pick_named("dependency kinds", &KINDS)?),
        Dim::Target => {
            if rng.coin() {
                dep.scope = Some("cfg(unix)".to_owned());
            }
        }
        Dim::Optional => dep.optional = rng.coin(),
        Dim::DefaultFeatures => dep.defaults = rng.coin(),
        Dim::Features => {
            dep.features = ["a", "b", "c"]
                .iter()
                .copied()
                .filter(|_| rng.coin())
                .map(str::to_owned)
                .collect();
        }
        Dim::Rename => {
            if rng.coin() {
                dep.alias = Some(format!("alias{index}"));
            }
        }
        Dim::Path => {
            dep.source = None;
            if rng.coin() {
                let picked = rng.pick_named("member names", members)?;
                dep.path = Some(format!("../{}", picked.name));
                dep.name.clone_from(&picked.name);
            } else {
                dep.path = Some(format!("../outside/dep{index}"));
            }
        }
    }
    let (class, detail) = match (dep.source.as_deref(), dep.path.as_deref()) {
        (Some(source), _) => (class_of(source), source.to_owned()),
        (None, Some(path)) => ("path", path.to_owned()),
        (None, None) => ("other", String::new()),
    };
    let mut edge = ModeledEdge {
        consumer,
        package: dep.name.clone(),
        requirement: dep.req.clone(),
        kind: kind_of(dep.kind),
        class,
        detail,
        optional: dep.optional,
        workspace: false,
        target_repository: None,
        features: dep.features.clone(),
        uses_default_features: dep.defaults,
        target: dep.scope.clone(),
        rename: dep.alias.clone(),
    };
    // The generator's only path shapes are `../<member>` and
    // `../outside/<dep>`, so membership is decided by the leading component.
    if dep.source.is_none()
        && let Some(relative) = dep
            .path
            .as_deref()
            .and_then(|path| path.strip_prefix("../"))
        && let Some(target) = members.iter().find(|candidate| candidate.name == relative)
    {
        edge.workspace = true;
        edge.target_repository.clone_from(&target.repository);
    }
    Ok((member, dep, edge))
}

/// Builds a valid workspace varying `allowed` dimensions, one per member.
fn workspace(seed: u64, allowed: &[Dim]) -> Outcome<Workspace> {
    let mut rng = Rng::new(seed);
    let count = *rng.pick_named("member counts", &MEMBER_COUNTS)?;
    let mut members = Vec::new();
    for slot in 0..count {
        let name = format!("m{slot}");
        let repository = rng
            .coin()
            .then(|| format!("https://example.invalid/{name}"));
        members.push(Named { name, repository });
    }
    let mut built = Workspace {
        records: Vec::new(),
        plans: Vec::new(),
        ids: Vec::new(),
        edges: Vec::new(),
    };
    for (index, own) in members.iter().enumerate() {
        let dim = if allowed.is_empty() {
            Dim::Kind
        } else {
            *rng.pick_named("varying dimensions", allowed)?
        };
        let (member, dep, edge) = build_member(index, own, dim, &members, &mut rng)?;
        built.ids.push(member.id.clone());
        let record = member_json(&member, std::slice::from_ref(&dep));
        built.records.push(record);
        built.plans.push((member, dep));
        built.edges.push(edge);
    }
    Ok(built)
}

/// A valid scenario varying `allowed` dimensions.
fn valid_scenario(seed: u64, allowed: &[Dim]) -> Outcome<Scenario> {
    let built = workspace(seed, allowed)?;
    let document = built.document();
    let mut edges = built.edges;
    edges.sort();
    Ok(Scenario {
        document,
        model: Model::Edges(edges),
    })
}

/// A valid workspace `apply` damages (given a coin drawn from the seed), which
/// `parse` must refuse as a schema error naming `fragment`.
fn fault_scenario(
    seed: u64,
    fragment: &'static str,
    apply: impl FnOnce(&mut Workspace, bool),
) -> Outcome<Scenario> {
    let coin = Rng::new(seed).coin();
    let mut built = workspace(seed, ALL_DIMS)?;
    apply(&mut built, coin);
    Ok(Scenario {
        document: built.document(),
        model: Model::Schema(fragment),
    })
}

/// A path edge that resolves into a member directory under another name is a
/// schema refusal, never a quiet misattribution.
fn mismatch_scenario(seed: u64) -> Outcome<Scenario> {
    let mut rng = Rng::new(seed);
    let first = MemberPlan::bare("app", "/repo/app/Cargo.toml".to_owned());
    let second = MemberPlan::bare("helper", "/repo/helper/Cargo.toml".to_owned());
    let wrong_name = *rng.pick_named("member-path name mismatches", &MISMATCHED_NAMES)?;
    let dep = DepPlan {
        name: wrong_name.to_owned(),
        source: None,
        req: "*".to_owned(),
        path: Some("../helper".to_owned()),
        ..DepPlan::registry(String::new())
    };
    let records = vec![
        member_json(&first, std::slice::from_ref(&dep)),
        member_json(&second, &[]),
    ];
    let ids = vec![first.id, second.id];
    Ok(Scenario {
        document: document(&records, &ids),
        model: Model::Schema("resolves to workspace package"),
    })
}

/// Two members claiming one manifest directory are refused, not merged.
fn shared_dir_scenario(seed: u64) -> Outcome<Scenario> {
    let mut rng = Rng::new(seed);
    let shared = *rng.pick_named("shared manifest directories", &SHARED_DIRS)?;
    let first = MemberPlan::bare("first", format!("{shared}/Cargo.toml"));
    let second = MemberPlan::bare("second", format!("{shared}/Cargo.toml"));
    let records = vec![member_json(&first, &[]), member_json(&second, &[])];
    let ids = vec![first.id, second.id];
    Ok(Scenario {
        document: document(&records, &ids),
        model: Model::Schema("multiple workspace packages claim manifest directory"),
    })
}

/// A strict prefix of a valid document is invalid JSON, never a partial graph.
fn truncated_scenario(seed: u64) -> Outcome<Scenario> {
    let mut rng = Rng::new(seed);
    let full = workspace(seed, ALL_DIMS)?.document();
    // Every share is inside the document, so the slice is bounded by the draw
    // rather than by a clamped index a short document could push past the end.
    let offset = cut_offset(&mut rng, full.len())
        .ok_or("a document shorter than one eighth has no strict prefix to cut")?;
    let cut = full
        .get(..offset)
        .ok_or("a cut share must name a byte boundary inside the document")?;
    Ok(Scenario {
        document: cut.to_owned(),
        model: Model::Json,
    })
}

/// A document with a wrongly typed field is a JSON refusal.
fn wrong_type_scenario(seed: u64) -> Outcome<Scenario> {
    let mut rng = Rng::new(seed);
    let document = (*rng.pick_named("wrongly typed documents", &MALFORMED)?).to_owned();
    Ok(Scenario {
        document,
        model: Model::Json,
    })
}

/// A virtual workspace with no members is still valid, and has no edges.
fn empty_scenario() -> Scenario {
    Scenario {
        document: r#"{"packages":[],"workspace_members":[]}"#.to_owned(),
        model: Model::Edges(Vec::new()),
    }
}

/// Asserts `parse` agrees with the model, naming the seed on failure.
fn assert_model(seed: u64, scenario: &Scenario) {
    let observed = observe(metadata::parse(&scenario.document));
    let agrees = match scenario.model {
        Model::Edges(ref expected) => {
            matches!(&observed, Observed::Edges(actual) if actual == expected)
        }
        Model::Schema(fragment) => {
            matches!(&observed, Observed::Schema(message) if message.contains(fragment))
        }
        Model::Json => matches!(&observed, Observed::Json),
    };
    assert!(
        agrees,
        "seed {seed}: expected {:?}, observed {observed:?}",
        scenario.model
    );
}

/// Runs one scenario family over `0..count`, asserting the model each time.
fn sweep(count: u64, mut scenario: impl FnMut(u64) -> Outcome<Scenario>) -> TestResult {
    for seed in 0..count {
        assert_model(seed, &scenario(seed)?);
    }
    Ok(())
}

/// Folds eight generated documents for a derived seed into one trace hash.
fn trace(seed: u64) -> Outcome<u64> {
    let mut trace = Trace::new();
    for step in 0..8_u64 {
        let derived = seed.wrapping_mul(8).wrapping_add(step);
        let scenario = valid_scenario(derived, ALL_DIMS)?;
        trace.record(&scenario.document);
        let observed = observe(metadata::parse(&scenario.document));
        trace.record(&format!("{observed:?}"));
        assert_model(derived, &scenario);
    }
    Ok(receipt(&trace)?)
}

/// One test per scenario family: each runs `$count` seeds of `$scenario`
/// through the model.
macro_rules! sweeps {
    ($($(#[$doc:meta])* $name:ident: $count:expr, $scenario:expr;)*) => {
        $(
            $(#[$doc])*
            #[test]
            fn $name() -> TestResult {
                sweep($count, $scenario)
            }
        )*
    };
}

sweeps! {
    /// Any mix of dimensions decodes exactly as the model predicts.
    a_seeded_document_matches_the_modelled_edge_set: 64, |seed| valid_scenario(seed, ALL_DIMS);
    /// The `kind` dimension is decoded and modelled per seed.
    the_kind_dimension_is_modelled_and_decoded: 64, |seed| valid_scenario(seed, &[Dim::Kind]);
    /// The `target` scope is decoded and modelled per seed.
    the_target_dimension_is_modelled_and_decoded: 64, |seed| valid_scenario(seed, &[Dim::Target]);
    /// The `optional` bit is decoded and modelled per seed.
    the_optional_dimension_is_modelled_and_decoded: 64, |seed| valid_scenario(seed, &[Dim::Optional]);
    /// The `default-features` bit is decoded and modelled per seed.
    the_default_features_dimension_is_modelled_and_decoded: 64, |seed| valid_scenario(seed, &[Dim::DefaultFeatures]);
    /// The enabled `features` list is decoded and modelled per seed.
    the_features_dimension_is_modelled_and_decoded: 64, |seed| valid_scenario(seed, &[Dim::Features]);
    /// A local rename keeps the upstream identity and is decoded per seed.
    the_rename_dimension_is_modelled_and_decoded: 64, |seed| valid_scenario(seed, &[Dim::Rename]);
    /// Internal and external path edges are decoded and modelled per seed.
    the_path_dimension_is_modelled_and_decoded: 64, |seed| valid_scenario(seed, &[Dim::Path]);
    /// A virtual workspace with no members stays valid and yields no edges.
    an_empty_workspace_decodes_to_no_edges: 16, |_seed| Ok(empty_scenario());
    /// A duplicate package record is refused before any graph is built.
    a_duplicate_package_record_is_a_schema_refusal: 64, |seed| fault_scenario(seed, "duplicate Cargo package id", |built, _| {
        built.records.extend(built.records.first().cloned());
    });
    /// A member id with no package record is a schema refusal, not an empty graph.
    a_missing_package_record_is_a_schema_refusal: 64, |seed| fault_scenario(seed, "has no package record", |built, _| {
        built.ids.push("path+file:///repo/ghost#ghost@0.1.0".to_owned());
    });
    /// A repeated workspace member id is a schema refusal.
    a_duplicate_member_id_is_a_schema_refusal: 64, |seed| fault_scenario(seed, "duplicate Cargo workspace member id", |built, _| {
        built.ids.extend(built.ids.first().cloned());
    });
    /// A member with no manifest path is a schema refusal.
    a_member_without_a_manifest_path_is_a_schema_refusal: 64, |seed| fault_scenario(seed, "has no manifest_path", |built, _| {
        built.edit_first(|member, _| member.manifest_path = None);
    });
    /// A blank identity is a schema refusal.
    a_blank_identity_is_a_schema_refusal: 64, |seed| fault_scenario(seed, "blank id or name", |built, blank_id| {
        built.edit_first(|member, _| if blank_id { member.id.clear() } else { member.name.clear() });
    });
    /// An unknown dependency kind is a schema refusal, never folded to normal.
    an_unknown_dependency_kind_is_a_schema_refusal: 64, |seed| fault_scenario(seed, "unknown Cargo dependency kind", |built, _| {
        built.edit_first(|_, dep| {
            dep.source = Some(REGISTRY.to_owned());
            dep.path = None;
            dep.kind = Some("proc-macro");
        });
    });
    /// A declaration with neither source nor path is a schema refusal.
    a_dependency_without_source_or_path_is_a_schema_refusal: 64, |seed| fault_scenario(seed, "has neither source nor path", |built, _| {
        built.edit_first(|_, dep| {
            dep.source = None;
            dep.path = None;
            dep.kind = None;
        });
    });
    /// A path target whose name disagrees with its member record is refused.
    a_member_path_name_mismatch_is_a_schema_refusal: 64, mismatch_scenario;
    /// Two members claiming one manifest directory are refused, never merged.
    two_members_sharing_a_manifest_directory_are_refused: 32, shared_dir_scenario;
    /// A strict prefix of a valid document is a JSON refusal.
    truncated_documents_are_json_refusals: 64, truncated_scenario;
    /// A wrongly typed field is a JSON refusal.
    wrongly_typed_fields_are_json_refusals: 64, wrong_type_scenario;
}

/// Across the general family the sweep exercises internal member path edges,
/// external path edges, and every dependency kind.
#[test]
fn the_sweep_covers_members_external_paths_and_kinds() -> TestResult {
    let (mut internal, mut external) = (false, false);
    let (mut normal, mut build, mut dev) = (false, false, false);
    for seed in 0..128_u64 {
        let scenario = valid_scenario(seed, ALL_DIMS)?;
        if let Model::Edges(ref edges) = scenario.model {
            for edge in edges {
                internal |= edge.workspace;
                external |= edge.class == "path" && !edge.workspace;
                match edge.kind {
                    "build" => build = true,
                    "dev" => dev = true,
                    _ => normal = true,
                }
            }
        }
        assert_model(seed, &scenario);
    }
    assert!(
        internal && external,
        "the sweep must exercise internal and external path edges"
    );
    assert!(
        normal && build && dev,
        "the sweep must exercise normal, build and dev kinds: {normal} {build} {dev}"
    );
    Ok(())
}

/// Replaying a seed produces the same trace hash.
#[test]
fn same_seed_same_trace() -> TestResult {
    for seed in 0..32_u64 {
        let first = trace(seed)?;
        let second = trace(seed)?;
        assert_eq!(
            first, second,
            "seed {seed} produced two different trace hashes"
        );
    }
    Ok(())
}
