//! A fixture's `git`, deaf to the repository a hook runs in.
//!
//! Source: githooks(5). A pre-commit hook runs with `GIT_DIR` and
//! `GIT_INDEX_FILE` set to the repository being committed. On 2026-11-24 the
//! `ai_authoring` fixture ran `git add -A` and `git update-ref HEAD` under a
//! pre-commit hook in a worktree. Both went through the inherited variables,
//! so the commit's index became the fixture's tree, and the branch moved to a
//! root commit named `fixture`. These are the variables `lgwks_deps::git`
//! removes, from the one file `src/redirecting_env.rs`.

use std::process::Command;

/// `command` with every redirecting variable removed, so it reaches only the
/// repository it names with `-C`. Under nextest the `churn-slot` wrapper has
/// already removed them; a plain `cargo test` has no wrapper.
pub(crate) fn deaf(mut command: Command) -> Command {
    for name in crate::redirecting_env::REDIRECTING_ENV {
        command.env_remove(name);
    }
    command
}

/// Catches a fixture `git` that follows a hook's `GIT_DIR` or `GIT_INDEX_FILE`
/// into the repository being committed: the decoy those variables name keeps
/// its HEAD and its index, and the fixture gets its own.
#[test]
fn git_env_a_fixture_never_writes_through_a_hooks_repository()
-> Result<(), Box<dyn std::error::Error>> {
    let scratch = crate::lock_fixture::Scratch::new("git-env")?;
    let (decoy, fixture) = (scratch.path().join("decoy"), scratch.path().join("fixture"));
    for dir in [&decoy, &fixture] {
        std::fs::create_dir_all(dir)?;
        assert!(deaf(git(dir, &["init", "-q"])).status()?.success());
    }
    std::fs::write(fixture.join("file.txt"), "x")?;
    for step in [
        &["add", "-A", "."][..],
        &["commit", "-q", "-m", "fixture"][..],
    ] {
        let mut command = git(&fixture, step);
        command
            .env("GIT_DIR", decoy.join(".git"))
            .env("GIT_INDEX_FILE", decoy.join(".git/index"));
        assert!(
            deaf(command).status()?.success(),
            "{step:?} runs in the fixture"
        );
    }
    let head =
        |dir: &std::path::Path| deaf(git(dir, &["rev-parse", "-q", "--verify", "HEAD"])).output();
    assert!(!head(&decoy)?.status.success(), "the decoy has no commit");
    assert!(!decoy.join(".git/index").exists(), "the decoy has no index");
    assert!(
        head(&fixture)?.status.success(),
        "the fixture has its commit"
    );
    Ok(())
}

/// `git -C dir args` with a fixed identity and no host configuration.
fn git(dir: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid");
    command
}
