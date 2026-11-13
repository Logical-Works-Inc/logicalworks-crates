//! Seeded trees of directories, files and every kind of link, walked by the
//! stream against a non-following `read_dir` recursion and an independent
//! preorder model of the following walk (#945).
//!
//! Source: logical_ci's A06 write detector (logical_ci#248) walks a lane's
//! subject tree to see what the lane planted. A link to `/etc` or to nothing
//! is exactly such a plant, so the walk lists every link whatever it names,
//! and following only ever adds the inside of in-root directory links.
//!
//! The fools: the monkey (seeded trees at every depth bound, follow on and
//! off), wrong-order (links read before their target, into an ancestor and
//! into the root), giant-and-empty (a depth bound of zero), weird-bytes
//! (non-ASCII names with spaces) and the saboteur (the shrunk case where a
//! link read first hid its directory and renamed itself to the target).

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use super::{OmissionPolicy, WalkOptions, walk_dir_iter};
use crate::fs::{FileKind, WalkFailure};

/// What a sim reports when the walk disagreed with the model.
type SimResult = Result<(), Box<dyn std::error::Error>>;

/// Why a sim helper could not build, walk or model a tree.
#[derive(Debug)]
enum SimError {
    /// The scratch tree could not be made or read.
    Io(io::Error),
    /// The stream reported an omission the sim never planted.
    Walk(WalkFailure),
    /// A draw was asked for a value below zero.
    EmptyDraw,
    /// A drawn value does not fit the width it indexes.
    Width(std::num::TryFromIntError),
}

impl std::fmt::Display for SimError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Io(ref error) => write!(formatter, "the scratch tree refused: {error}"),
            Self::Walk(ref failure) => write!(formatter, "the stream reported: {failure}"),
            Self::EmptyDraw => formatter.write_str("a draw needs a nonzero bound"),
            Self::Width(ref error) => write!(formatter, "a drawn index is too wide: {error}"),
        }
    }
}

impl std::error::Error for SimError {}

impl From<io::Error> for SimError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<std::num::TryFromIntError> for SimError {
    fn from(error: std::num::TryFromIntError) -> Self {
        Self::Width(error)
    }
}

/// One walk, in the order it yielded: relative path and kind.
type Trace = Vec<(PathBuf, FileKind)>;

/// Trees per run: each seed is one tree.
const SEEDS: u64 = 96;
/// Entries a generated directory holds at most.
const FAN_OUT: u64 = 5;
/// Directory levels a generated tree has at most.
const LEVELS: usize = 3;
/// Links read before their target, planted per tree at most.
const FORWARD_LINKS: u64 = 4;
/// Every depth bound the model is held to: zero, each generated level, none.
const DEPTH_BOUNDS: [usize; 5] = [0, 1, 2, 3, usize::MAX];

/// A seeded xorshift: one seed, one tree, the same on every run.
struct Draw(u64);

impl Draw {
    /// The next draw below `bound`, refused for a zero bound.
    fn below(&mut self, bound: u64) -> Result<u64, SimError> {
        self.0 ^= self.0.wrapping_shl(13);
        self.0 ^= self.0.wrapping_shr(7);
        self.0 ^= self.0.wrapping_shl(17);
        self.0.checked_rem(bound).ok_or(SimError::EmptyDraw)
    }

    /// One drawn index into a list of `len` items.
    fn index(&mut self, len: usize) -> Result<usize, SimError> {
        let span = u64::try_from(len.max(1))?;
        let drawn = self.below(span)?;
        Ok(usize::try_from(drawn)?)
    }
}

/// A fresh scratch directory unique to this test binary and `name`.
fn scratch(name: &str) -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("lgwks-walk-links-{}-{name}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    dir.canonicalize()
}

/// What the generator made: in-root link targets, and the links the walk
/// must list but never descend.
#[derive(Default)]
struct Grown {
    /// In-root files and directories already made, for in-root links.
    targets: Vec<PathBuf>,
    /// Relative paths of every directory made, the root (empty) first.
    dirs: Vec<PathBuf>,
    /// Relative paths of links to a directory outside the root, or to nothing.
    sealed: Vec<PathBuf>,
}

/// Fills `at` under `root` with a seeded mix of entries, recursing into the
/// directories it makes.
#[cfg(unix)]
fn grow(
    draw: &mut Draw,
    root: &Path,
    outside: &Path,
    at: &Path,
    level: usize,
    grown: &mut Grown,
) -> Result<(), SimError> {
    use std::os::unix::fs::symlink;
    let entries = draw.below(FAN_OUT)?.saturating_add(1);
    for index in 0..entries {
        let stem = if draw.below(4)? == 0 { "é " } else { "e" };
        let name = at.join(format!("{stem}{index}"));
        let path = root.join(&name);
        match draw.below(6)? {
            0 | 1 if level < LEVELS => {
                std::fs::create_dir(&path)?;
                grown.targets.push(path.clone());
                grown.dirs.push(name.clone());
                grow(draw, root, outside, &name, level.saturating_add(1), grown)?;
            }
            2 => {
                let pick = draw.index(grown.targets.len())?;
                match grown.targets.get(pick) {
                    Some(target) => symlink(target, &path)?,
                    None => std::fs::write(&path, b"leaf")?,
                }
            }
            3 => {
                symlink(outside, &path)?;
                grown.sealed.push(name);
            }
            4 => {
                symlink(root.join("never-made"), &path)?;
                grown.sealed.push(name);
            }
            _ => {
                std::fs::write(&path, b"leaf")?;
                grown.targets.push(path);
            }
        }
    }
    Ok(())
}

/// Plants links named `a*`, which sort before every grown name, so the walk
/// reads each one before the directory it names: a later sibling, a deeper
/// cousin, an ancestor or the root itself.
#[cfg(unix)]
fn plant_forward_links(draw: &mut Draw, root: &Path, grown: &Grown) -> Result<(), SimError> {
    for link in 0..draw.below(FORWARD_LINKS.saturating_add(1))? {
        let home = draw.index(grown.dirs.len())?;
        let target = draw.index(grown.dirs.len())?;
        if let (Some(home), Some(target)) = (grown.dirs.get(home), grown.dirs.get(target)) {
            std::os::unix::fs::symlink(
                root.join(target),
                root.join(home).join(format!("a{link}")),
            )?;
        }
    }
    Ok(())
}

/// One seeded tree under a scratch root, with its out-of-root neighbour.
#[cfg(unix)]
fn tree(tag: &str, seed: u64, outside: &Path) -> Result<(PathBuf, Grown), SimError> {
    let root = scratch(&format!("{tag}-{seed}"))?;
    let mut draw = Draw(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut grown = Grown {
        dirs: vec![PathBuf::new()],
        ..Grown::default()
    };
    grow(&mut draw, &root, outside, Path::new(""), 1, &mut grown)?;
    plant_forward_links(&mut draw, &root, &grown)?;
    Ok((root, grown))
}

/// The reference: every entry `read_dir` lists, recursing into real
/// directories only and classifying each by its own `lstat`.
fn listed(root: &Path, at: &Path, out: &mut BTreeMap<PathBuf, FileKind>) -> io::Result<()> {
    for entry in std::fs::read_dir(root.join(at))? {
        let entry = entry?;
        let relative = at.join(entry.file_name());
        let kind = FileKind::of(entry.file_type()?);
        out.insert(relative.clone(), kind);
        if kind == FileKind::Directory {
            listed(root, &relative, out)?;
        }
    }
    Ok(())
}

/// The device and inode a path names, following a final link.
#[cfg(unix)]
fn identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    let stat = std::fs::metadata(path)?;
    Ok((stat.dev(), stat.ino()))
}

/// The model of a following walk, written from its contract and not from the
/// stream: preorder by name; every listed name is an entry under its own
/// logical path; a directory, or an in-root directory link when following, is
/// entered at most once per identity and only within the depth bound.
#[cfg(unix)]
fn model(
    root: &Path,
    at: &Path,
    depth: usize,
    options: &WalkOptions,
    entered: &mut HashSet<(u64, u64)>,
    out: &mut Trace,
) -> io::Result<()> {
    let mut entries = std::fs::read_dir(root.join(at))?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let relative = at.join(entry.file_name());
        let kind = FileKind::of(entry.file_type()?);
        out.push((relative.clone(), kind));
        let below = depth.saturating_add(1);
        if below > options.max_depth {
            continue;
        }
        let enters = match kind {
            FileKind::Directory => entered.insert(identity(&root.join(&relative))?),
            FileKind::Symlink if options.follow_symlinks => {
                match root.join(&relative).canonicalize() {
                    Ok(target) if target.starts_with(root) && target.is_dir() => {
                        entered.insert(identity(&target)?)
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if enters {
            model(root, &relative, below, options, entered, out)?;
        }
    }
    Ok(())
}

/// The model's trace for `root` under `options`.
#[cfg(unix)]
fn modelled(root: &Path, options: &WalkOptions) -> io::Result<Trace> {
    let mut entered = HashSet::from([identity(root)?]);
    let mut out = Vec::new();
    model(root, Path::new(""), 0, options, &mut entered, &mut out)?;
    Ok(out)
}

/// The stream's trace, in the order it yielded.
fn traced(root: &Path, options: &WalkOptions) -> Result<Trace, SimError> {
    let walk = walk_dir_iter(root, options, OmissionPolicy::default())?;
    walk.map(|item| {
        item.map(|entry| (entry.relative_path().to_path_buf(), entry.kind()))
            .map_err(SimError::Walk)
    })
    .collect()
}

/// The stream's entries as relative paths to kinds.
fn streamed(root: &Path, follow: bool) -> Result<BTreeMap<PathBuf, FileKind>, SimError> {
    let options = WalkOptions {
        follow_symlinks: follow,
        ..WalkOptions::default()
    };
    Ok(traced(root, &options)?.into_iter().collect())
}

/// Catches #945: with follow off, an out-of-root or dangling link withheld
/// from the stream; with follow on, a name the tree holds listed under no
/// name or reclassified, or a sealed link descended.
#[test]
#[cfg(unix)]
fn sim_every_link_is_listed_whatever_it_names() -> SimResult {
    let outside = scratch("listed-outside")?;
    std::fs::write(outside.join("passwd"), b"secret")?;
    for seed in 1..=SEEDS {
        let (root, grown) = tree("listed", seed, &outside)?;
        let mut expected = BTreeMap::new();
        listed(&root, Path::new(""), &mut expected)?;
        let unfollowed = streamed(&root, false)?;
        if unfollowed != expected {
            return Err(format!(
                "seed {seed}: unfollowed walk {unfollowed:?} != read_dir {expected:?}"
            )
            .into());
        }
        let followed = streamed(&root, true)?;
        // Following lists a directory a link entered first beneath the link,
        // so a name is held by where it lives: its parent's canonical path.
        let mut held = BTreeMap::new();
        for (path, kind) in &followed {
            let absolute = root.join(path);
            if let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) {
                held.insert(parent.canonicalize()?.join(name), *kind);
            }
        }
        for (path, kind) in &expected {
            if held.get(&root.join(path)) != Some(kind) {
                return Err(format!(
                    "seed {seed}: following listed {path:?} under no name, or reclassified it"
                )
                .into());
            }
        }
        for path in followed.keys() {
            if grown
                .sealed
                .iter()
                .any(|link| path != link && path.starts_with(link))
            {
                return Err(
                    format!("seed {seed}: following descended a sealed link to {path:?}").into(),
                );
            }
        }
        std::fs::remove_dir_all(&root)?;
    }
    std::fs::remove_dir_all(&outside)?;
    Ok(())
}

/// Catches a depth bound off by one for links, a link followed with follow
/// off, a link entered under its target's name and a directory withheld
/// because a link entered it first: the stream must yield exactly the
/// model's trace, in order, at every bound, and replay it.
#[test]
#[cfg(unix)]
fn sim_links_monkey_the_stream_is_the_model_at_every_depth_bound() -> SimResult {
    let outside = scratch("model-outside")?;
    std::fs::write(outside.join("passwd"), b"secret")?;
    for seed in 1..=SEEDS {
        let (root, _) = tree("model", seed, &outside)?;
        for follow_symlinks in [false, true] {
            for max_depth in DEPTH_BOUNDS {
                let options = WalkOptions {
                    follow_symlinks,
                    max_depth,
                    ..WalkOptions::default()
                };
                let expected = modelled(&root, &options)?;
                let first = traced(&root, &options)?;
                if first != expected {
                    return Err(format!(
                        "seed {seed}, follow {follow_symlinks}, depth {max_depth}: \
                         stream {first:?} != model {expected:?}"
                    )
                    .into());
                }
                if traced(&root, &options)? != first {
                    return Err(format!("seed {seed}: the same tree walked twice diverged").into());
                }
            }
        }
        std::fs::remove_dir_all(&root)?;
    }
    std::fs::remove_dir_all(&outside)?;
    Ok(())
}

/// Catches the shrunk #945 follow-up: `a -> e`, read before `e`, entered `e`
/// under the name `e` and then withheld the real `e`. The link keeps its own
/// name, its inside is listed beneath it, and `e` is still listed.
#[test]
#[cfg(unix)]
fn sim_links_saboteur_a_link_read_first_never_hides_or_renames_its_directory() -> SimResult {
    let root = scratch("saboteur")?;
    std::fs::create_dir(root.join("e"))?;
    std::fs::write(root.join("e").join("f"), b"leaf")?;
    std::os::unix::fs::symlink(root.join("e"), root.join("a"))?;
    let options = WalkOptions {
        follow_symlinks: true,
        ..WalkOptions::default()
    };
    let expected: Trace = vec![
        (PathBuf::from("a"), FileKind::Symlink),
        (PathBuf::from("a/f"), FileKind::File),
        (PathBuf::from("e"), FileKind::Directory),
    ];
    let walked = traced(&root, &options)?;
    if walked != expected {
        return Err(format!("the walk yielded {walked:?}, not {expected:?}").into());
    }
    if modelled(&root, &options)? != expected {
        return Err("the model disagrees with its own shrunk case".into());
    }
    std::fs::remove_dir_all(&root)?;
    Ok(())
}
