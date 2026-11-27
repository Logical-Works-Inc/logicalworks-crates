//! The ledger sweep against a hostile directory (#1160 mechanism 3).
//!
//! Each seed fills a ledger directory with records the way a crashed fleet
//! leaves them: owners still alive (their lock held), owners dead, a record
//! torn mid-line by a power cut, lines no ledger writes, records from another
//! boot, devices a live owner still holds, devices re-attached from another
//! image, and files that only look like records. The sweep runs against a
//! model host that records each release. The oracle is the rule in the
//! module doc, computed from what the seed drew, never from the sweep's own
//! answer: exactly the dead records' open leases are released, each once, in
//! the order groups, devices, programs, and only the dead records are gone.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use super::{Host, Lease, fold, image_in, sweep};
use crate::process::ProcessIdentity;
use crate::seeded::Seeded;

/// The boot every seed sweeps on.
const BOOT: &str = "kinfo:1795700000.000001";

/// Another boot, which a record may carry.
const OTHER_BOOT: &str = "kinfo:1795600000.000002";

/// A disk image file path, with the space `hdiutil info` prints as is.
const DMG: &str = "/Volumes/images/My Disk.dmg";

/// Another image a device number may be re-attached from.
const OTHER_DMG: &str = "/Volumes/images/Other.dmg";

/// A RAM disk's image path.
const RAM: &str = "ram://8388608";

/// Seeds swept.
const SEEDS: u64 = 256;

/// One release the model host saw.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Call {
    /// A group reaped.
    Reap(String),
    /// A device detached.
    Detach(String),
    /// A program run.
    Run(String, String),
}

/// A host that records each release and answers images from a table.
struct Model {
    /// What was released, in order.
    calls: RefCell<Vec<Call>>,
    /// The image each attached device is attached from now.
    images: BTreeMap<String, String>,
}

impl Host for Model {
    fn reap(&self, leader: &ProcessIdentity) {
        self.calls.borrow_mut().push(Call::Reap(leader.to_string()));
    }

    fn image(&self, device: &str) -> Option<String> {
        self.images.get(device).cloned()
    }

    fn detach(&self, device: &str) {
        self.calls
            .borrow_mut()
            .push(Call::Detach(device.to_owned()));
    }

    fn run(&self, program: &str, argument: &str) {
        self.calls
            .borrow_mut()
            .push(Call::Run(program.to_owned(), argument.to_owned()));
    }
}

/// A draw below `bound`.
fn below(rng: &mut Seeded, bound: u64) -> io::Result<u64> {
    rng.below(bound)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, format!("{error:?}")))
}

/// A directory removed with what it holds when the seed ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let removed = if self.0.is_dir() {
            fs::remove_dir_all(&self.0)
        } else {
            fs::remove_file(&self.0)
        };
        if let Err(error) = removed {
            crate::trace::debug!(%error, "a ledger sim path could not be removed");
        }
    }
}

/// What one seed drew and what the rule says must follow.
struct World {
    /// Locked handles of live owners, held through the sweep.
    live: Vec<File>,
    /// Records a dead owner left.
    dead: BTreeSet<PathBuf>,
    /// Files the sweep must leave.
    kept: BTreeSet<PathBuf>,
    /// The releases the rule demands, in the order it demands them.
    want: Vec<Call>,
    /// The host's attached images.
    images: BTreeMap<String, String>,
}

/// Draw one lease of id `id`.
fn draw_lease(rng: &mut Seeded, id: u64) -> io::Result<Lease> {
    Ok(match below(rng, 3)? {
        0 => {
            let pid = i32::try_from(below(rng, 90_000)?.saturating_add(2))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            let leader = ProcessIdentity::new(pid, &format!("kinfo:{id}.{pid}"))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            Lease::Group(leader)
        }
        1 => {
            let image = match below(rng, 3)? {
                0 => None,
                1 => Some(RAM.to_owned()),
                _ => Some(DMG.to_owned()),
            };
            Lease::Device {
                device: format!("/dev/disk{}", below(rng, 6)?.saturating_add(20)),
                image,
            }
        }
        _ => Lease::Run(
            "/nonexistent-lgwks-sim/rm".to_owned(),
            format!("/tmp/lgwks-sim-{id}.lock"),
        ),
    })
}

/// The releases the rule demands for one dead record, in its order.
fn demanded(
    open: &BTreeMap<u64, Lease>,
    same_boot: bool,
    world: &World,
    live: &BTreeSet<String>,
) -> Vec<Call> {
    let mut groups = Vec::new();
    let mut devices = Vec::new();
    let mut runs = Vec::new();
    for lease in open.values() {
        match *lease {
            Lease::Group(ref leader) => groups.push(Call::Reap(leader.to_string())),
            Lease::Device {
                ref device,
                image: Some(ref image),
            } if same_boot && !live.contains(device) && world.images.get(device) == Some(image) => {
                devices.push(Call::Detach(device.clone()));
            }
            Lease::Device { .. } => {}
            Lease::Run(ref program, ref argument) => {
                runs.push(Call::Run(program.clone(), argument.clone()));
            }
        }
    }
    groups.extend(devices);
    groups.extend(runs);
    groups
}

/// Fill `dir` with drawn records and return the world they make.
fn draw_world(rng: &mut Seeded, dir: &Path) -> io::Result<World> {
    let mut world = World {
        live: Vec::new(),
        dead: BTreeSet::new(),
        kept: BTreeSet::new(),
        want: Vec::new(),
        images: BTreeMap::new(),
    };
    for disk in 20..26_u64 {
        let image = match below(rng, 3)? {
            0 => continue,
            1 => RAM,
            _ => OTHER_DMG,
        };
        world
            .images
            .insert(format!("/dev/disk{disk}"), image.to_owned());
    }
    // Lookalikes: never a record, never touched.
    for name in ["notes.txt", "12-1-0.lease.tmp", ".lease-not"] {
        let path = dir.join(name);
        fs::write(&path, "+ 1 r /bin/rm,/tmp/x\n")?;
        world.kept.insert(path);
    }
    let mut live_devices = BTreeSet::new();
    let mut dead_records = Vec::new();
    for index in 0..below(rng, 6)?.saturating_add(1) {
        let path = dir.join(format!("{index}-0-0.lease"));
        let other_boot = below(rng, 5)? == 0;
        let boot = if other_boot { OTHER_BOOT } else { BOOT };
        let mut text = format!("lgwks-lease 1\nboot {boot}\n");
        let mut open = BTreeMap::new();
        for id in 1..=below(rng, 8)? {
            let lease = draw_lease(rng, id)?;
            text.push_str(&lease.line(id));
            open.insert(id, lease);
            if below(rng, 3)? == 0 {
                text.push_str("- ");
                text.push_str(&id.to_string());
                text.push('\n');
                open.remove(&id);
            }
        }
        if below(rng, 4)? == 0 {
            text.push_str("? 9 z junk\n+ x g 1/kinfo:1\n+ 7 r relative,arg\n");
        }
        if below(rng, 4)? == 0 {
            // A power cut mid-line: a lease the owner never finished writing.
            text.push_str("+ 99 r /nonexistent-lgwks-sim/rm,/tmp/torn");
        }
        let mut file = File::create(&path)?;
        file.write_all(text.as_bytes())?;
        if below(rng, 2)? == 0 {
            file.try_lock()
                .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error))?;
            world.live.push(file);
            world.kept.insert(path);
            live_devices.extend(open.into_values().filter_map(|lease| match lease {
                Lease::Device { device, .. } => Some(device),
                Lease::Group(_) | Lease::Run(..) => None,
            }));
        } else {
            drop(file);
            world.dead.insert(path);
            dead_records.push((!other_boot, open));
        }
    }
    let mut want = Vec::new();
    for &(same_boot, ref open) in &dead_records {
        want.extend(demanded(open, same_boot, &world, &live_devices));
    }
    world.want = want;
    Ok(world)
}

/// Fold a whole trace of calls into one number (FNV-1a).
fn trace_of(calls: &[Call]) -> u64 {
    let text = format!("{calls:?}");
    text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// One seed: draw, sweep, judge. Returns the trace.
fn one_seed(seed: u64) -> io::Result<u64> {
    let mut rng = Seeded::from_seed(seed);
    let dir = Scratch(
        std::env::temp_dir().join(format!("lgwks-ledger-sim-{}-{seed:x}", std::process::id())),
    );
    fs::create_dir_all(&dir.0)?;
    let world = draw_world(&mut rng, &dir.0)?;
    let host = Model {
        calls: RefCell::new(Vec::new()),
        images: world.images.clone(),
    };
    let swept = sweep(&dir.0, Some(BOOT), &host);
    let calls = host.calls.into_inner();
    assert_eq!(
        calls, world.want,
        "seed {seed:#x}: the releases the rule demands"
    );
    assert_eq!(
        swept.records,
        world.dead.len(),
        "seed {seed:#x}: every dead record swept"
    );
    assert_eq!(
        swept.released,
        world.want.len(),
        "seed {seed:#x}: the count"
    );
    for path in &world.dead {
        assert!(
            !path.exists(),
            "seed {seed:#x}: {} outlived its sweep",
            path.display()
        );
    }
    for path in &world.kept {
        assert!(
            path.exists(),
            "seed {seed:#x}: {} was swept",
            path.display()
        );
    }
    drop(world.live);
    Ok(trace_of(&calls))
}

/// The hostile directory, seed by seed, each seed replayed to one trace.
#[test]
fn sim_ledger_sweep_releases_exactly_the_dead_records_leases() -> io::Result<()> {
    for seed in 0..SEEDS {
        let first = one_seed(seed)?;
        assert_eq!(
            first,
            one_seed(seed)?,
            "seed {seed:#x} replays to one trace"
        );
    }
    Ok(())
}

/// A sweep of a directory that is not there, or is a file, releases nothing
/// and never fails the hold that started it.
#[test]
fn sim_ledger_sweep_of_no_directory_is_empty() -> io::Result<()> {
    let host = Model {
        calls: RefCell::new(Vec::new()),
        images: BTreeMap::new(),
    };
    let missing = std::env::temp_dir().join(format!("lgwks-ledger-none-{}", std::process::id()));
    assert_eq!(sweep(&missing, Some(BOOT), &host), super::Swept::default());
    let file = Scratch(missing.with_extension("file"));
    fs::write(&file.0, "x")?;
    assert_eq!(sweep(&file.0, Some(BOOT), &host), super::Swept::default());
    assert!(host.calls.into_inner().is_empty());
    Ok(())
}

/// The fold: a torn last line is ignored, a let-go lease is gone, and a line
/// no ledger writes is skipped.
#[test]
fn sim_ledger_fold_keeps_only_whole_open_leases() {
    let folded = fold(concat!(
        "lgwks-lease 1\n",
        "boot kinfo:1.2\n",
        "+ 1 r /bin/rm,/tmp/a\n",
        "+ 2 d /dev/disk30 ram://64\n",
        "- 1\n",
        "+ 3 d /dev/disk31 -\n",
        "+ 4 r rm,/tmp/relative\n",
        "zz\n",
        "+ 5 r /bin/rm,/tmp/torn",
    ));
    assert_eq!(folded.boot.as_deref(), Some("kinfo:1.2"));
    let ids: Vec<u64> = folded.leases.keys().copied().collect();
    assert_eq!(ids, [2, 3]);
    assert_eq!(fold(""), super::Folded::default());
}

/// A record renamed over by its rewrite: the sweep's open file is no longer
/// the one the path names, though both sit on the same device, so it must not
/// remove the fresh record its live owner just wrote.
#[test]
fn sim_ledger_a_renamed_over_record_is_not_the_one_its_path_names() -> io::Result<()> {
    let old =
        Scratch(std::env::temp_dir().join(format!("lgwks-ledger-old-{}", std::process::id())));
    let fresh = Scratch(old.0.with_extension("fresh"));
    fs::write(&old.0, "old")?;
    let open = File::open(&old.0)?;
    assert!(
        super::still_named(&open, &old.0),
        "an untouched record is the one its path names"
    );
    fs::write(&fresh.0, "fresh")?;
    fs::rename(&fresh.0, &old.0)?;
    assert!(
        !super::still_named(&open, &old.0),
        "a same-device rename over the path was taken for the open record"
    );
    Ok(())
}

/// A release that never ends, here one that stops itself, is cut off at its
/// bound and reads as failed, whatever it would have printed.
#[test]
fn sim_ledger_a_release_past_its_bound_is_killed_and_failed() {
    let stopped = super::bounded_within(
        "/bin/sh",
        &["-c", "echo started; kill -STOP $$"],
        std::time::Duration::from_millis(200),
    );
    assert_eq!(stopped, None, "a release past its bound read as a success");
    let ended = super::bounded_within(
        "/bin/sh",
        &["-c", "echo done"],
        std::time::Duration::from_secs(10),
    );
    assert_eq!(ended.as_deref(), Some("done\n"));
}

/// `hdiutil info` as macOS prints it: each image's block names its path, and
/// the device lines under it.
#[test]
fn sim_ledger_reads_the_image_a_device_is_attached_from() {
    let info = concat!(
        "framework       : 710.60.2\n",
        "================================================\n",
        "image-path      : /Volumes/images/My Disk.dmg\n",
        "image-type      : read-only disk image\n",
        "/dev/disk6          \tGUID_partition_scheme          \t\n",
        "/dev/disk6s1        \tApple_APFS                     \t/Volumes/My Disk\n",
        "================================================\n",
        "image-path      : ram://8388608\n",
        "/dev/disk14\t\t/Volumes/lgwks-test-tmp\n",
    );
    assert_eq!(image_in(info, "/dev/disk6").as_deref(), Some(DMG));
    assert_eq!(image_in(info, "/dev/disk14").as_deref(), Some(RAM));
    assert_eq!(image_in(info, "/dev/disk1"), None);
    assert_eq!(image_in(info, "/dev/disk6s"), None);
}
