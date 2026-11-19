//! Hostile seeded sweeps over the test-code reader of the layer scan (#1096).
//!
//! Each sim checks the reader against a reference model the sim builds, never
//! against the reader's own output: a predicate tree evaluated over every
//! assignment, and a planted file whose shipping imports the generator knows.

use std::error::Error;

use super::edges;
use super::layer_edges::{LAYER_EXCEPTIONS, pick, upward_refusals};
use super::test_cfg::{requires_test, test_lines};
use crate::sim::{Rng, Trace};

/// One generated predicate: an option by bit, or a combinator.
enum Tree {
    /// Bit 0 is `test`; the others are options a build without tests may set.
    Atom(u32),
    /// Every branch holds.
    All(Vec<Tree>),
    /// Some branch holds.
    Any(Vec<Tree>),
    /// The branch does not hold.
    Not(Box<Tree>),
}

/// Every assignment of the four options a generated tree draws from.
const ASSIGNMENTS: u32 = 16;

/// The reference verdict of `tree` under `bits`, where bit 0 is `test`.
fn holds(tree: &Tree, bits: u32) -> bool {
    match *tree {
        Tree::Atom(at) => bits.checked_shr(at).is_some_and(|value| (value & 1) == 1),
        Tree::All(ref items) => items.iter().all(|item| holds(item, bits)),
        Tree::Any(ref items) => items.iter().any(|item| holds(item, bits)),
        Tree::Not(ref item) => !holds(item, bits),
    }
}

/// The reference answer: no assignment with `test` off satisfies `tree`.
fn needs_test(tree: &Tree) -> bool {
    !(0..ASSIGNMENTS)
        .filter(|bits| (bits & 1) == 0)
        .any(|bits| holds(tree, bits))
}

/// A random tree no deeper than `depth`, with a monkey's choice of shape.
fn grow(rng: &mut Rng, depth: u32) -> Tree {
    let shape = if depth == 0 { 0 } else { rng.below(4) };
    let below = depth.saturating_sub(1);
    match shape {
        1 => Tree::All((0..rng.below(4)).map(|_| grow(rng, below)).collect()),
        2 => Tree::Any((0..rng.below(4)).map(|_| grow(rng, below)).collect()),
        3 => Tree::Not(Box::new(grow(rng, below))),
        _ => Tree::Atom(rng.below(4)),
    }
}

/// Weird bytes a feature value may hold: NUL, a right-to-left mark, a zero-width
/// space, brackets, a comma, an equals sign and `test` itself.
const WEIRD: [&str; 8] = ["\u{0}", "\u{202e}", "\u{200b}", "(", ")", ",", "=", "test"];

/// `tree` as cfg text, with random spacing and a weird feature value.
fn render(tree: &Tree, rng: &mut Rng, weird: &str, into: &mut String) {
    let space = if rng.below(2) == 0 { "" } else { "  " };
    match *tree {
        Tree::Atom(0) => into.push_str("test"),
        Tree::Atom(1) => into.push_str("unix"),
        Tree::Atom(2) => {
            for piece in ["feature", space, "=", space, "\"x", weird, "\""] {
                into.push_str(piece);
            }
        }
        Tree::Atom(_) => into.push_str("target_os = \"windows\""),
        Tree::All(ref items) | Tree::Any(ref items) => {
            into.push_str(if matches!(tree, Tree::All(_)) {
                "all("
            } else {
                "any("
            });
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    into.push(',');
                    into.push_str(space);
                }
                render(item, rng, weird, into);
            }
            into.push(')');
        }
        Tree::Not(ref item) => {
            into.push_str("not(");
            render(item, rng, weird, into);
            into.push(')');
        }
    }
}

/// One sweep of `draws` trees on `seed`, as its trace hash.
fn predicate_sweep(seed: u64, draws: u32) -> Result<u64, Box<dyn Error>> {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    let mut verdicts = [0_u32; 2];
    for draw in 0..draws {
        let depth = rng.below(5);
        let tree = grow(&mut rng, depth);
        let weird = pick(&mut rng, &WEIRD)?;
        let mut text = String::new();
        render(&tree, &mut rng, weird, &mut text);
        let expected = needs_test(&tree);
        assert_eq!(
            requires_test(&text),
            expected,
            "seed {seed:#x} draw {draw}: {text:?}"
        );
        if let Some(count) = verdicts.get_mut(usize::from(expected)) {
            *count = count.saturating_add(1);
        }
        trace.record_number(&text, u8::from(expected));
    }
    assert!(
        verdicts.iter().all(|count| *count > 0),
        "seed {seed:#x}: the sweep drew both verdicts, {verdicts:?}"
    );
    Ok(trace.hash())
}

#[test]
/// Monkey, weird-bytes, giant-and-empty and saboteur: every seeded predicate
/// tree gets the verdict of its truth table, a nest deeper than the reader's
/// floor reads as shipped, and the same seed gives the same trace hash.
fn sim_requires_test_is_sound_for_every_assignment() -> Result<(), Box<dyn Error>> {
    for seed in [0x1096_A000_u64, 0x1096_A001, 0xFFFF_1096_A002, 0x0] {
        let first = predicate_sweep(seed, 2_000)?;
        assert_eq!(
            first,
            predicate_sweep(seed, 2_000)?,
            "seed {seed:#x} replays"
        );
    }
    for depth in [1_usize, 2, 31, 32, 33, 1_000] {
        let nested = format!("{}test{}", "not(".repeat(depth), ")".repeat(depth));
        let expected = depth <= 32 && depth.is_multiple_of(2);
        assert_eq!(requires_test(&nested), expected, "not nested {depth} deep");
    }
    let wide: Vec<String> = (0..20).map(|at| format!("feature = \"f{at}\"")).collect();
    assert!(
        !requires_test(&format!("all(test, {})", wide.join(", "))),
        "a predicate past the option bound reads as shipped"
    );
    Ok(())
}

/// The hidden lines a test module may hold: braces inside literals and comments
/// that must never move the module's end.
const HIDDEN: [&str; 7] = [
    "    use crate::script::Hidden;",
    "    fn brace() -> char { '}' }",
    "    const OPEN: &str = \"{{{\";",
    "    // }",
    "    /* } */",
    "    const RAW: &str = r#\"}}\"#;",
    "    fn quote() -> char { '\\'' }",
];

/// Predicates that need `test`.
const TESTED: [&str; 3] = ["test", "all(test, feature = \"script\")", "not(not(test))"];

/// Predicates whose module compiles outside tests.
const SHIPPED: [&str; 2] = ["any(test, unix)", "not(test)"];

/// One planted file: its text and line count, the shipping lines that name an
/// upper module, every line that names one (line, module), and whether it is
/// Rust at all: a module left open makes the whole file a grammar refusal.
pub(super) struct Planted {
    pub(super) text: String,
    pub(super) lines: usize,
    pub(super) edges: Vec<(usize, String)>,
    pub(super) imports: Vec<(usize, String)>,
    pub(super) valid: bool,
}

impl Planted {
    /// What the reference model says the scan reports: the shipping imports of
    /// a file that parses, and every import of one the grammar refuses.
    pub(super) fn expected(&self) -> &[(usize, String)] {
        if self.valid {
            &self.edges
        } else {
            &self.imports
        }
    }
}

/// A planted file that names the modules of `upper`.
pub(super) fn plant(rng: &mut Rng, upper: &[&str]) -> Result<Planted, Box<dyn Error>> {
    let mut lines: Vec<String> = Vec::new();
    let mut edges = Vec::new();
    let mut imports = Vec::new();
    let mut valid = true;
    for _ in 0..rng.between(1, 40) {
        match rng.below(6) {
            0 | 1 => lines.push(String::from("const FILLER: u8 = { 1 };")),
            2 => {
                let name = pick(rng, upper)?;
                lines.push(format!("use crate::{name}::Thing;"));
                edges.push((lines.len(), name.to_owned()));
                imports.push((lines.len(), name.to_owned()));
            }
            3 => lines.push(String::new()),
            _ => {
                let ships = rng.below(4) == 0;
                let predicate = if ships {
                    pick(rng, &SHIPPED)?
                } else {
                    pick(rng, &TESTED)?
                };
                lines.push(format!("#[cfg({predicate})]"));
                lines.push(String::from("mod tests {"));
                for _ in 0..rng.below(6) {
                    let hidden = pick(rng, &HIDDEN)?;
                    lines.push(hidden.to_owned());
                    if hidden.contains("crate::script") {
                        imports.push((lines.len(), String::from("script")));
                        if ships {
                            edges.push((lines.len(), String::from("script")));
                        }
                    }
                }
                // Wrong order and pulled plug: some modules never close.
                if rng.below(5) == 0 {
                    valid = false;
                } else {
                    lines.push(String::from("}"));
                }
            }
        }
    }
    Ok(Planted {
        text: lines.join("\n"),
        lines: lines.len(),
        edges,
        imports,
        valid,
    })
}

/// The refusals `edges` give in `file`, shifted down by `offset` lines.
pub(super) fn refusals(file: &str, edges: &[(usize, String)], offset: usize) -> Vec<String> {
    let layer = if file.starts_with("rt/") {
        "rt"
    } else {
        "world"
    };
    edges
        .iter()
        .map(|&(line, ref name)| {
            format!(
                "{file}:{}: {layer} module names crate::{name}",
                line.saturating_add(offset)
            )
        })
        .collect()
}

/// The rt tenant and the world tenant, each with the upper modules it plants.
pub(super) const RT: (&str, [&str; 3]) = ("rt/tenancy.rs", ["script", "session", "ecs"]);
const WORLD: (&str, [&str; 3]) = ("registry/table.rs", ["session", "task", "script"]);

/// One sweep of planted files on `seed`, as its trace hash.
fn file_sweep(seed: u64, draws: u32) -> Result<u64, Box<dyn Error>> {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for draw in 0..draws {
        let rt = plant(&mut rng, &RT.1)?;
        let world = plant(&mut rng, &WORLD.1)?;
        for (file, planted) in [(RT.0, &rt), (WORLD.0, &world)] {
            let found = upward_refusals(file, &planted.text);
            assert_eq!(
                found,
                refusals(file, planted.expected(), 0),
                "seed {seed:#x} draw {draw} {file}:\n{}",
                planted.text
            );
            match test_lines(&planted.text) {
                Ok(mask) => assert!(
                    planted.valid && mask.len() == planted.text.lines().count(),
                    "seed {seed:#x} draw {draw}: one mask entry per line of a file that parses"
                ),
                Err(refusal) => assert!(
                    !planted.valid,
                    "seed {seed:#x} draw {draw}: a valid file was refused: {refusal}"
                ),
            }
            let named = edges(file, &planted.text)
                .iter()
                .filter(|edge| edge.contains("the Rust grammar refused the file"))
                .count();
            assert_eq!(
                named,
                usize::from(!planted.valid),
                "seed {seed:#x} draw {draw}: a refusal is named exactly when the file is not Rust"
            );
            for edge in &found {
                trace.record(edge);
            }
        }
        // Neighbour: the world tenant's text after the rt tenant's, read as rt.
        // Every world module is above rt, so each world edge is an rt edge; the
        // joined file parses only when both parts do.
        let joined = format!("{}\n{}", rt.text, world.text);
        let mut want = Vec::new();
        if rt.valid && world.valid {
            want.extend(refusals(RT.0, &rt.edges, 0));
            want.extend(refusals(RT.0, &world.edges, rt.lines));
        } else {
            want.extend(refusals(RT.0, &rt.imports, 0));
            want.extend(refusals(RT.0, &world.imports, rt.lines));
        }
        let found = upward_refusals(RT.0, &joined);
        assert_eq!(found, want, "seed {seed:#x} draw {draw}: joined:\n{joined}");
        trace.record_number("joined", found.len());
    }
    Ok(trace.hash())
}

#[test]
/// Neighbour, wrong-order, pulled-plug and saboteur: two tenants' files in two
/// layers, each with planted shipping imports, test modules that ship or not,
/// braces hidden in literals and comments, and modules that never close. A file
/// that parses reports exactly the shipping imports the generator planted, a
/// file with an open module is named as refused and read whole, the joined file
/// follows the same model, and the same seed gives the same trace hash.
fn sim_planted_files_report_only_their_shipping_imports() -> Result<(), Box<dyn Error>> {
    for seed in [
        0x1096_B000_u64,
        0x1096_B001,
        0xFFFF_1096_B002,
        0x1096_0000_B003,
    ] {
        let first = file_sweep(seed, 500)?;
        assert_eq!(first, file_sweep(seed, 500)?, "seed {seed:#x} replays");
    }
    // The shrunk case the lexer got wrong: an rt module never closed hid what
    // followed it; the grammar refuses the file, so it is read whole.
    let open =
        "#[cfg(test)]\nmod tests {\n    fn brace() -> char { '}' }\nuse crate::session::Thing;";
    assert_eq!(
        upward_refusals("rt/tenancy.rs", open),
        vec![String::from(
            "rt/tenancy.rs:4: rt module names crate::session"
        )],
    );
    let closed = format!("{open}\n}}\nuse crate::session::Thing;");
    assert_eq!(
        upward_refusals("rt/tenancy.rs", &closed),
        vec![String::from(
            "rt/tenancy.rs:6: rt module names crate::session"
        )],
    );
    Ok(())
}

/// Attribute spellings a fat finger or a formatter writes, each with the verdict
/// the Reference gives it: whether the item under it exists only in test builds.
const SPELLINGS: [(&str, bool); 14] = [
    ("#[cfg(test)]", true),
    ("#[cfg( test )]", true),
    ("#[ cfg(test) ]", true),
    ("#[cfg(test,)]", true),
    (
        "#[cfg(all(\n    test,\n    feature = \"script\",\n))]",
        true,
    ),
    ("#[cfg(not(any(not(test))))]", true),
    ("#[cfg(all(test, any()))]", true),
    ("#[test]", true),
    ("#[cfg(not(test))]", false),
    ("#[cfg(any(test, unix))]", false),
    ("#[cfg_attr(test, allow(dead_code))]", false),
    ("#[cfg(feature = \"test\")]", false),
    ("#[cfg(tests)]", false),
    ("#[doc = \"#[cfg(test)]\"]", false),
];

/// What a run of attributes may carry around the spelled one.
const NEUTRAL: [&str; 4] = [
    "#[allow(dead_code)]",
    "/// A doc line.",
    "// a comment",
    "/* a block */",
];

/// The 1-based line of the first line of `text` equal to `wanted`.
fn line_of(text: &str, wanted: &str) -> Result<usize, Box<dyn Error>> {
    text.lines()
        .position(|line| line == wanted)
        .map(|at| at.saturating_add(1))
        .ok_or_else(|| Box::<dyn Error>::from("the planted line is not in the text"))
}

#[test]
/// Fat-finger, weird-bytes, wrong-order and saboteur: a test module under a
/// seeded spelling of its attribute, with doc lines, comments and other
/// attributes before and after it, and shipped imports sharing its first and
/// last lines. The module's import is hidden exactly when the Reference says
/// the module exists only in test builds, the imports beside it are always
/// read, both verdicts are drawn, and the same seed replays.
fn sim_attribute_spellings_fat_finger_never_hide_a_shipped_import() -> Result<(), Box<dyn Error>> {
    let hidden = "    use crate::script::Hidden;";
    let shown = "use crate::task::Shown;";
    let mut hashes = Vec::new();
    for seed in [0x1096_C000_u64, 0x1096_C001, 0x1096_C000] {
        let mut rng = Rng::new(seed);
        let mut trace = Trace::new();
        let mut drawn = [0_u32; 2];
        for draw in 0..400_u32 {
            let (spelled, hides) = pick(&mut rng, &SPELLINGS)?;
            let mut pieces: Vec<&str> = Vec::new();
            for _ in 0..rng.below(3) {
                pieces.push(pick(&mut rng, &NEUTRAL)?);
            }
            pieces.push(spelled);
            for _ in 0..rng.below(3) {
                pieces.push(pick(&mut rng, &NEUTRAL)?);
            }
            pieces.extend(["mod tests {", hidden]);
            // Wrong order: the shipped imports may share a line with the
            // attribute before it or the brace after it.
            let ahead = rng.below(2) == 0;
            let beside = rng.below(2) == 0;
            let mut text = pieces.join("\n");
            if ahead {
                text = format!("use crate::session::Ahead; {text}");
            }
            text.push_str(if beside { "\n} " } else { "\n}\n" });
            text.push_str(shown);
            let mut want = Vec::new();
            if ahead {
                want.push(String::from("rt/sync.rs:1: rt module names crate::session"));
            }
            if !hides {
                let at = line_of(&text, hidden)?;
                want.push(format!("rt/sync.rs:{at}: rt module names crate::script"));
            }
            let at = text.lines().count();
            want.push(format!("rt/sync.rs:{at}: rt module names crate::task"));
            assert_eq!(
                upward_refusals("rt/sync.rs", &text),
                want,
                "seed {seed:#x} draw {draw}:\n{text}"
            );
            if let Some(count) = drawn.get_mut(usize::from(hides)) {
                *count = count.saturating_add(1);
            }
            trace.record_number(&text, u8::from(hides));
        }
        assert!(
            drawn.iter().all(|count| *count > 0),
            "seed {seed:#x}: both verdicts drawn, {drawn:?}"
        );
        hashes.push(trace.hash());
    }
    assert_eq!(hashes.first(), hashes.get(2), "the first seed replays");
    assert_ne!(hashes.first(), hashes.get(1), "two seeds draw two traces");
    Ok(())
}

#[test]
/// Monkey, neighbour and saboteur on the shipped tree: a seeded real source file
/// of a layer below the top gets an import of a seeded higher module planted at
/// its end, once inside a `#[cfg(test)]` module and once bare. The hidden plant
/// changes no refusal of that file, the bare one adds exactly one at its line,
/// and the same seed replays.
fn sim_real_sources_monkey_planted_items_change_only_what_they_ship() -> Result<(), Box<dyn Error>>
{
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let tree = super::read_tree(&src)?;
    let mut layered: Vec<(&str, &str, usize)> = Vec::new();
    for source in &tree {
        let (relative, text) = (source.0.as_str(), source.1.as_str());
        let Some(own) = super::layer_index(relative) else {
            continue;
        };
        if !super::is_test_file(relative) && own.saturating_add(1) < super::LAYERS.len() {
            layered.push((relative, text, own));
        }
    }
    assert!(layered.len() > 100, "the walk found the layered sources");
    let indices: Vec<usize> = (0..layered.len()).collect();
    let mut hashes = Vec::new();
    for _ in 0..2 {
        let mut rng = Rng::new(0x1096_D000);
        let mut trace = Trace::new();
        for draw in 0..120_u32 {
            let at = pick(&mut rng, &indices)?;
            let (relative, text, own) = layered
                .get(at)
                .copied()
                .ok_or_else(|| Box::<dyn Error>::from("the draw fell outside the files"))?;
            let layer = super::LAYERS
                .get(own)
                .ok_or_else(|| Box::<dyn Error>::from("the layer is not in the table"))?;
            let higher: Vec<&str> = super::LAYERS
                .iter()
                .skip(own.saturating_add(1))
                .flat_map(|upper| upper.modules.iter().copied())
                .collect();
            let upper = pick(&mut rng, &higher)?;
            let base = if text.ends_with('\n') {
                text.to_owned()
            } else {
                format!("{text}\n")
            };
            let before = upward_refusals(relative, &base);
            // The baseline is held to the reviewed table, not to the reader: the
            // modules a real file names upward are exactly its listed entries.
            let mut seen: Vec<&str> = before
                .iter()
                .filter_map(|edge| edge.rsplit_once("crate::").map(|(_, module)| module))
                .collect();
            seen.sort_unstable();
            seen.dedup();
            let mut listed: Vec<&str> = LAYER_EXCEPTIONS
                .iter()
                .filter(|entry| entry.0 == relative)
                .map(|entry| entry.1)
                .collect();
            listed.sort_unstable();
            assert_eq!(
                seen, listed,
                "draw {draw}: {relative} against LAYER_EXCEPTIONS"
            );
            let tested = format!(
                "{base}#[cfg(test)]\nmod planted_tests {{\n    use crate::{upper}::Planted;\n}}\n"
            );
            assert_eq!(
                upward_refusals(relative, &tested),
                before,
                "draw {draw}: a test plant in {relative} changed its refusals"
            );
            let line = base.lines().count().saturating_add(1);
            let mut want = before;
            want.push(format!(
                "{relative}:{line}: {} module names crate::{upper}",
                layer.name
            ));
            let shipped = format!("{base}use crate::{upper}::Planted;\n");
            assert_eq!(
                upward_refusals(relative, &shipped),
                want,
                "draw {draw}: a shipped plant in {relative}"
            );
            trace.record(relative);
            trace.record(upper);
        }
        hashes.push(trace.hash());
    }
    assert_eq!(hashes.first(), hashes.get(1), "the seed replays");
    Ok(())
}
