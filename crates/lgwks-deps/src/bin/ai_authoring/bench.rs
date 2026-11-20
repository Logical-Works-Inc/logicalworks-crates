//! The benchmark as it declares itself: `bench.toml` in the benchmark's own
//! directory names its arms, tasks, profiles, file layout, limits, defaults, the
//! model command line and the protocol it records, so the runner holds none of
//! them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::toml_subset::{Table, list, raw, read_tables, text};
use super::{BenchError, fail, read};

/// The manifest's file name inside the benchmark directory.
pub(super) const MANIFEST: &str = "bench.toml";

/// Where the benchmark's files are, relative to its directory.
///
/// `prompt`, `oracle`, `reference` and `mutant` are patterns: `{task}` and
/// `{surface}` stand for the names they say.
pub(super) struct Layout {
    /// The crate template's directory.
    pub(super) template: String,
    /// The committed lock every trial copies.
    pub(super) lock: String,
    /// A task's prompt.
    pub(super) prompt: String,
    /// A task's hidden oracle.
    pub(super) oracle: String,
    /// A reference solution.
    pub(super) reference: String,
    /// A task's deliberate mutant.
    pub(super) mutant: String,
    /// Where committed runs go.
    pub(super) runs: String,
}

/// One path the template names that the runner points at a real directory.
pub(super) struct TemplatePath {
    /// The text in the template's manifest.
    pub(super) token: String,
    /// The directory it stands for, relative to the benchmark directory.
    pub(super) target: String,
}

/// How the crate template becomes a trial crate.
pub(super) struct Template {
    /// The text the template's package name is written as.
    pub(super) name_token: String,
    /// The paths to point at real directories.
    pub(super) paths: Vec<TemplatePath>,
}

/// The model command and the sandbox that keeps its author closed-book.
pub(super) struct ModelCli {
    /// The model command.
    pub(super) command: String,
    /// The sandbox program that wraps the model command.
    pub(super) sandbox: String,
    /// The model command's arguments; `{model}` stands for the model id.
    pub(super) args: Vec<String>,
    /// The system directories the author may not read, besides the home
    /// directory and the run's work root.
    pub(super) denied: Vec<String>,
    /// The model command's own settings directory, under the home directory.
    pub(super) config_dir: String,
    /// What the settings directory holds that the author may not read.
    pub(super) withheld: Vec<String>,
    /// Further directories under the home directory the command needs, by
    /// name; none when it keeps everything in its settings directory.
    pub(super) allowed: Vec<String>,
    /// The environment the command is given beyond the run's own, as
    /// `NAME=value`; `{scratch}` stands for the trial's scratch directory.
    pub(super) env: Vec<String>,
}

/// How long each kind of child may run.
pub(super) struct Limits {
    /// One model call.
    pub(super) model_call: Duration,
    /// One `cargo build`.
    pub(super) build: Duration,
    /// One oracle run.
    pub(super) oracle: Duration,
    /// One `git` query.
    pub(super) git: Duration,
    /// The most tokens, read and written, every model call of a run may bill.
    pub(super) token_budget: u64,
    /// The most tokens one reply may bill, reasoning included; a call is
    /// admitted only when this and its prompt still fit the budget.
    pub(super) call_tokens: u64,
}

/// What a run does when the command line does not say.
pub(super) struct Defaults {
    /// The models a models run asks.
    pub(super) models: Vec<String>,
    /// Trials per cell.
    pub(super) trials: u32,
    /// Trials running at once.
    pub(super) parallel: usize,
    /// Repairs a failing compile gets.
    pub(super) max_repairs: u32,
}

/// One fixed persona preamble.
pub(super) struct Profile {
    /// The profile's name.
    pub(super) name: String,
    /// The preamble prepended to the prompt skeleton.
    pub(super) text: String,
}

/// One API surface an author is given a sheet for.
pub(super) struct Arm {
    /// The arm's name.
    pub(super) name: String,
    /// The sheet the author reads, relative to the benchmark directory.
    pub(super) sheet: String,
    /// The surface whose reference solutions this arm's dry run compiles.
    pub(super) reference: String,
}

/// A reference solution one arm borrows from another surface for one task.
pub(super) struct Borrowed {
    /// The arm that borrows.
    pub(super) arm: String,
    /// The task it borrows for.
    pub(super) task: String,
    /// The surface it borrows from.
    pub(super) surface: String,
}

/// One held-out task.
pub(super) struct Task {
    /// The task's name.
    pub(super) name: String,
    /// The oracle clause that says a dropped future left nothing live.
    pub(super) drop_clause: String,
    /// The surface whose reference a mutant of this task is one mutation from.
    pub(super) mutant_base: String,
}

/// An arm and task pair the benchmark does not run, and why.
pub(super) struct Exclusion {
    /// The task.
    pub(super) task: String,
    /// The arm.
    pub(super) arm: String,
    /// Why the arm cannot express the task.
    pub(super) why: String,
}

/// A protocol statement a summary records.
pub(super) enum Setting {
    /// A sentence.
    Words(String),
    /// A yes or no.
    Flag(bool),
}

/// The benchmark.
pub(super) struct Bench {
    /// The benchmark directory.
    pub(super) root: PathBuf,
    /// The reply contract that closes every prompt.
    pub(super) framing: String,
    /// The text the committed oracles name the crate under test with.
    pub(super) oracle_token: String,
    /// The prefix of every trial crate's package name.
    pub(super) package_prefix: String,
    /// The tokens counted as author burden.
    pub(super) tokens: Vec<String>,
    /// The arm a mutant's record is filed under.
    pub(super) mutant_arm: String,
    /// What a record says about the lock its trial built from.
    pub(super) lockfile_note: String,
    /// Where the files are.
    pub(super) layout: Layout,
    /// The crate template.
    pub(super) template: Template,
    /// The model command.
    pub(super) model: ModelCli,
    /// The bounds on children.
    pub(super) limits: Limits,
    /// What a run does by default.
    pub(super) defaults: Defaults,
    /// The profiles, in order.
    pub(super) profiles: Vec<Profile>,
    /// The arms, in order.
    pub(super) arms: Vec<Arm>,
    /// The references arms borrow.
    pub(super) borrowed: Vec<Borrowed>,
    /// The tasks, in order.
    pub(super) tasks: Vec<Task>,
    /// The pairs that are not run.
    pub(super) exclusions: Vec<Exclusion>,
    /// The protocol statements a summary records.
    pub(super) protocol: Vec<(String, Setting)>,
}

/// The string under `key`, or a refusal naming the table and key.
fn required(table: &Table, key: &str) -> Result<String, BenchError> {
    match text(table, key) {
        Some(found) if !found.is_empty() => Ok(found.to_owned()),
        Some(_) | None => fail(
            BenchError::Manifest(format!("[{}] needs a non-empty {key}", table.path)),
            "required",
        ),
    }
}

/// The strings under `key`, or a refusal when there are none.
fn required_list(table: &Table, key: &str) -> Result<Vec<String>, BenchError> {
    match list(table, key) {
        Some(found) if !found.is_empty() => Ok(found.to_vec()),
        Some(_) | None => fail(
            BenchError::Manifest(format!("[{}] needs a non-empty list {key}", table.path)),
            "required_list",
        ),
    }
}

/// The strings under `key`, none when the key is absent: the manifest says
/// nothing, and the command needs nothing, so there is nothing to refuse.
fn optional_list(table: &Table, key: &str) -> Vec<String> {
    match list(table, key) {
        Some(found) => found.to_vec(),
        None => Vec::new(),
    }
}

/// The whole number under `key`, or a refusal when it is absent or is not one.
fn number(table: &Table, key: &str) -> Result<u64, BenchError> {
    match raw(table, key).and_then(|written| written.parse::<u64>().ok()) {
        Some(found) if found > 0 => Ok(found),
        Some(_) | None => fail(
            BenchError::Manifest(format!(
                "[{}] needs {key} as a whole number above zero",
                table.path
            )),
            "number",
        ),
    }
}

/// `number` as a count that fits `u32`.
fn count(table: &Table, key: &str) -> Result<u32, BenchError> {
    match u32::try_from(number(table, key)?) {
        Ok(found) => Ok(found),
        Err(_) => fail(
            BenchError::Manifest(format!("[{}] {key} does not fit 32 bits", table.path)),
            "count",
        ),
    }
}

/// The one table named `path`, or a refusal.
fn only<'tables>(tables: &'tables [Table], path: &str) -> Result<&'tables Table, BenchError> {
    match tables
        .iter()
        .find(|table| table.path == path && !table.array)
    {
        Some(found) => Ok(found),
        None => fail(
            BenchError::Manifest(format!("the manifest has no [{path}] table")),
            "only",
        ),
    }
}

/// Every `[[path]]` element.
fn elements<'tables>(tables: &'tables [Table], path: &str) -> Vec<&'tables Table> {
    tables
        .iter()
        .filter(|table| table.path == path && table.array)
        .collect()
}

/// Every `[[path]]` element, which the manifest must have at least one of.
fn each<'tables>(tables: &'tables [Table], path: &str) -> Result<Vec<&'tables Table>, BenchError> {
    let found = elements(tables, path);
    if found.is_empty() {
        return fail(
            BenchError::Manifest(format!("the manifest has no [[{path}]] element")),
            "each",
        );
    }
    Ok(found)
}

/// A refusal when a name is unsafe in a path, or occurs twice among `names`.
///
/// Names become directory names, package names and file names, so a name with a
/// separator or a dot-dot would let a manifest reach outside its work root.
fn distinct(kind: &str, names: &[&str]) -> Result<(), BenchError> {
    for (at, name) in names.iter().enumerate() {
        let safe = !name.is_empty()
            && *name != "."
            && *name != ".."
            && name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'));
        if !safe {
            return fail(
                BenchError::Manifest(format!(
                    "the {kind} {name:?} must be non-empty and use only [A-Za-z0-9_.-]"
                )),
                "distinct",
            );
        }
        if names
            .iter()
            .skip(at.saturating_add(1))
            .any(|other| other == name)
        {
            return fail(
                BenchError::Manifest(format!("the {kind} {name:?} is declared twice")),
                "distinct",
            );
        }
    }
    Ok(())
}

/// The statements of `[protocol]`, words and flags.
fn settings(table: &Table) -> Vec<(String, Setting)> {
    table
        .entries
        .iter()
        .filter_map(|entry| {
            let key = entry.key.clone();
            match text(table, &key) {
                Some(words) => Some((key, Setting::Words(words.to_owned()))),
                None => match raw(table, &key) {
                    Some("true") => Some((key, Setting::Flag(true))),
                    Some("false") => Some((key, Setting::Flag(false))),
                    Some(_) | None => None,
                },
            }
        })
        .collect()
}

/// `pattern` with each `{key}` replaced by its value.
pub(super) fn fill(pattern: &str, pairs: &[(&str, &str)]) -> String {
    pairs.iter().fold(pattern.to_owned(), |filled, pair| {
        filled.replace(&format!("{{{}}}", pair.0), pair.1)
    })
}

/// The `[layout]` table.
fn layout_of(table: &Table) -> Result<Layout, BenchError> {
    let layout = Layout {
        template: required(table, "template")?,
        lock: required(table, "lock")?,
        prompt: required(table, "prompt")?,
        oracle: required(table, "oracle")?,
        reference: required(table, "reference")?,
        mutant: required(table, "mutant")?,
        runs: required(table, "runs")?,
    };
    Ok(layout)
}

/// The `[model]` table.
fn model_of(table: &Table) -> Result<ModelCli, BenchError> {
    let model = ModelCli {
        command: required(table, "command")?,
        sandbox: required(table, "sandbox")?,
        args: required_list(table, "args")?,
        denied: required_list(table, "denied")?,
        config_dir: required(table, "config_dir")?,
        withheld: required_list(table, "withheld")?,
        allowed: optional_list(table, "allowed"),
        env: optional_list(table, "env"),
    };
    Ok(model)
}

/// The `[[arm]]` tables, each with its reference surface (its own name when none
/// is written).
fn arms_of(tables: &[Table]) -> Result<Vec<Arm>, BenchError> {
    let mut arms = Vec::new();
    for table in each(tables, "arm")? {
        let name = required(table, "name")?;
        let reference = match text(table, "reference") {
            Some(surface) if !surface.is_empty() => surface.to_owned(),
            Some(_) | None => name.clone(),
        };
        arms.push(Arm {
            name,
            sheet: required(table, "sheet")?,
            reference,
        });
    }
    Ok(arms)
}

/// The `[[task]]` tables.
fn tasks_of(tables: &[Table]) -> Result<Vec<Task>, BenchError> {
    let mut tasks = Vec::new();
    for table in each(tables, "task")? {
        tasks.push(Task {
            name: required(table, "name")?,
            drop_clause: required(table, "drop_clause")?,
            mutant_base: required(table, "mutant_base")?,
        });
    }
    Ok(tasks)
}

/// The `[[borrowed]]` and `[[exclusion]]` tables, which are optional.
fn exceptions_of(tables: &[Table]) -> Result<(Vec<Borrowed>, Vec<Exclusion>), BenchError> {
    let mut borrowed = Vec::new();
    for table in elements(tables, "borrowed") {
        borrowed.push(Borrowed {
            arm: required(table, "arm")?,
            task: required(table, "task")?,
            surface: required(table, "surface")?,
        });
    }
    let mut exclusions = Vec::new();
    for table in elements(tables, "exclusion") {
        exclusions.push(Exclusion {
            task: required(table, "task")?,
            arm: required(table, "arm")?,
            why: required(table, "why")?,
        });
    }
    Ok((borrowed, exclusions))
}

/// The `[limits]` table.
fn limits_of(table: &Table) -> Result<Limits, BenchError> {
    let seconds = |key: &str| number(table, key).map(Duration::from_secs);
    let model_call = seconds("model_call_s")?;
    let build = seconds("build_s")?;
    let oracle = seconds("oracle_s")?;
    let git = seconds("git_s")?;
    let token_budget = number(table, "token_budget")?;
    let call_tokens = number(table, "call_tokens")?;
    Ok(Limits {
        model_call,
        build,
        oracle,
        git,
        token_budget,
        call_tokens,
    })
}

/// The `[defaults]` table; `parallel` must fit this target's `usize`, and the
/// refusal carries the conversion's own cause.
fn defaults_of(table: &Table) -> Result<Defaults, BenchError> {
    let parallel = match usize::try_from(number(table, "parallel")?) {
        Ok(workers) => workers,
        Err(cause) => {
            return fail(
                BenchError::Manifest(format!("[defaults] parallel does not fit: {cause}")),
                "defaults_of",
            );
        }
    };
    Ok(Defaults {
        models: required_list(table, "models")?,
        trials: count(table, "trials")?,
        parallel,
        max_repairs: count(table, "max_repairs")?,
    })
}

/// The names of `items`, as `name` reads them.
fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<&str> {
    items.iter().map(name).collect()
}

impl Bench {
    /// Reads `bench.toml` in `root`.
    pub(super) fn load(root: &Path) -> Result<Self, BenchError> {
        Self::parse(root, &read(&root.join(MANIFEST))?)
    }

    /// The benchmark a manifest's text declares, rooted at `root`.
    pub(super) fn parse(root: &Path, manifest: &str) -> Result<Self, BenchError> {
        let tables = read_tables(manifest);
        let top = only(&tables, "")?;
        let template = only(&tables, "template")?;
        let limits = only(&tables, "limits")?;
        let defaults = only(&tables, "defaults")?;
        let mut paths = Vec::new();
        for table in each(&tables, "template.path")? {
            paths.push(TemplatePath {
                token: required(table, "token")?,
                target: required(table, "target")?,
            });
        }
        let mut profiles = Vec::new();
        for table in each(&tables, "profile")? {
            profiles.push(Profile {
                name: required(table, "name")?,
                text: required(table, "text")?,
            });
        }
        let arms = arms_of(&tables)?;
        let tasks = tasks_of(&tables)?;
        let (borrowed, exclusions) = exceptions_of(&tables)?;
        let bench = Self {
            root: root.to_path_buf(),
            framing: required(top, "framing")?,
            oracle_token: required(top, "oracle_crate_token")?,
            package_prefix: required(top, "package_prefix")?,
            tokens: required_list(top, "orchestration_tokens")?,
            mutant_arm: required(top, "mutant_arm")?,
            lockfile_note: required(top, "lockfile_note")?,
            layout: layout_of(only(&tables, "layout")?)?,
            template: Template {
                name_token: required(template, "name_token")?,
                paths,
            },
            model: model_of(only(&tables, "model")?)?,
            limits: limits_of(limits)?,
            defaults: defaults_of(defaults)?,
            profiles,
            arms,
            borrowed,
            tasks,
            exclusions,
            protocol: settings(only(&tables, "protocol")?),
        };
        bench.validate()?;
        Ok(bench)
    }

    /// Refuses a manifest whose names collide, are unsafe, or point at nothing it declares.
    fn validate(&self) -> Result<(), BenchError> {
        distinct("profile", &names(&self.profiles, |item| item.name.as_str()))?;
        distinct("arm", &names(&self.arms, |item| item.name.as_str()))?;
        distinct("task", &names(&self.tasks, |item| item.name.as_str()))?;
        let pairs = self
            .exclusions
            .iter()
            .map(|item| (item.arm.as_str(), item.task.as_str()))
            .chain(
                self.borrowed
                    .iter()
                    .map(|item| (item.arm.as_str(), item.task.as_str())),
            );
        for (arm, task) in pairs {
            if self.task(task).is_none() || self.arm(arm).is_none() {
                return fail(
                    BenchError::Manifest(format!(
                        "{arm}/{task} names an arm or task the manifest does not declare"
                    )),
                    "validate",
                );
            }
        }
        if self.arm(&self.mutant_arm).is_none() {
            return fail(
                BenchError::Manifest(format!(
                    "mutant_arm {:?} is not a declared arm",
                    self.mutant_arm
                )),
                "validate",
            );
        }
        Ok(())
    }

    /// The task called `name`.
    pub(super) fn task(&self, name: &str) -> Option<&Task> {
        self.tasks.iter().find(|task| task.name == name)
    }

    /// The arm called `name`.
    pub(super) fn arm(&self, name: &str) -> Option<&Arm> {
        self.arms.iter().find(|arm| arm.name == name)
    }

    /// The profile called `name`.
    pub(super) fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    /// The surface whose reference solution `arm`'s dry run compiles for `task`.
    pub(super) fn reference_surface(&self, arm: &Arm, task: &str) -> String {
        match self
            .borrowed
            .iter()
            .find(|lend| lend.arm == arm.name && lend.task == task)
        {
            Some(lend) => lend.surface.clone(),
            None => arm.reference.clone(),
        }
    }

    /// Whether the benchmark declines to run `arm` on `task`.
    pub(super) fn excludes(&self, arm: &str, task: &str) -> bool {
        self.exclusions
            .iter()
            .any(|exclusion| exclusion.arm == arm && exclusion.task == task)
    }

    /// A path inside the benchmark directory.
    pub(super) fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

#[cfg(test)]
mod tests;
