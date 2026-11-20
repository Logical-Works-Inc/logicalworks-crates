//! Metadata and relative paths of the predicate walk's entries.

use super::*;

/// The kind an independent `symlink_metadata` names, by the test's own mapping.
fn independent_kind(metadata: &fs::Metadata) -> FileKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        FileKind::Symlink
    } else if file_type.is_dir() {
        FileKind::Directory
    } else if fs::FileType::is_file(&file_type) {
        // Only a regular file is a `File` here; every other kind is `Unknown`.
        FileKind::File
    } else {
        FileKind::Unknown
    }
}

#[test]
/// Every entry's metadata is the entry's own `lstat`: an independent
/// `symlink_metadata` of its path agrees on type, size, modification time and,
/// on Unix, inode, device, mode and link count — links and entries reached
/// through a followed alias included.
fn sim_every_entry_carries_its_own_lstat() -> TestResult {
    sweep(6, |seed, stream, tree| {
        #[cfg(unix)]
        {
            let file = tree
                .nodes
                .iter()
                .find(|node| !node.dir)
                .map(|node| tree.root.join(&node.rel));
            if let Some(file) = file {
                std::os::unix::fs::symlink(file, tree.root.join("~file-link"))?;
            }
            let target = tree.root.join(tree.some_directory(stream)?);
            std::os::unix::fs::symlink(target, tree.root.join("~dir-alias"))?;
        }
        let walk = options(UNBOUNDED_DEPTH, chance(stream, 500)?, true);
        let entries = walk_dir_entries(&tree.root, &walk, None, enter)?;
        assert!(
            entries.len() >= tree.nodes.len(),
            "seed {seed:#x}: the walk lost grown entries"
        );
        for entry in &entries {
            let independent = fs::symlink_metadata(entry.path())?;
            let carried = entry.metadata();
            let path = entry.path().display();
            assert_eq!(
                carried.file_type(),
                independent.file_type(),
                "seed {seed:#x}: {path} type"
            );
            assert_eq!(
                entry.kind(),
                independent_kind(&independent),
                "seed {seed:#x}: {path} kind"
            );
            assert_eq!(
                carried.len(),
                independent.len(),
                "seed {seed:#x}: {path} size"
            );
            assert_eq!(
                carried.modified()?,
                independent.modified()?,
                "seed {seed:#x}: {path} mtime"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt as _;
                assert_eq!(
                    (
                        carried.ino(),
                        carried.dev(),
                        carried.mode(),
                        carried.nlink()
                    ),
                    (
                        independent.ino(),
                        independent.dev(),
                        independent.mode(),
                        independent.nlink()
                    ),
                    "seed {seed:#x}: {path} inode, device, mode or link count"
                );
            }
        }
        Ok(())
    })
}

#[test]
/// Every relative path is non-empty, relative, and joins back onto the root to
/// give the absolute path; without links the set of relative paths is the
/// grown tree exactly. A followed alias visits its target's directories once
/// (the walk's existing cycle rule), so each grown entry is reached either at
/// its own relative path or at the same suffix below the alias — whichever the
/// walk reached first — and every extra relative path is led by the alias.
fn sim_relative_paths_join_back_to_the_absolute_path() -> TestResult {
    sweep(7, |seed, stream, tree| {
        let aliased = if cfg!(unix) && chance(stream, 500)? {
            Some(tree.some_directory(stream)?)
        } else {
            None
        };
        #[cfg(unix)]
        if let Some(target) = aliased.as_deref() {
            std::os::unix::fs::symlink(tree.root.join(target), tree.root.join("~alias"))?;
        }
        let walk = options(UNBOUNDED_DEPTH, aliased.is_some(), chance(stream, 500)?);
        let entries = walk_dir_entries(&tree.root, &walk, None, enter)?;
        for entry in &entries {
            let relative = entry.relative_path();
            assert!(
                relative.is_relative() && relative.components().next().is_some(),
                "seed {seed:#x}: {} is not a non-empty relative path",
                relative.display()
            );
            assert_eq!(
                tree.root.join(relative),
                entry.path(),
                "seed {seed:#x}: {} does not join back onto the root",
                relative.display()
            );
        }
        let grown: BTreeSet<PathBuf> = tree.nodes.iter().map(|node| node.rel.clone()).collect();
        let walked: BTreeSet<PathBuf> = relatives(&entries).into_iter().collect();
        if let Some(target) = aliased.as_deref() {
            for rel in &grown {
                let via_alias = rel
                    .strip_prefix(target)
                    .ok()
                    .map(|suffix| Path::new("~alias").join(suffix));
                assert!(
                    walked.contains(rel) || via_alias.is_some_and(|alias| walked.contains(&alias)),
                    "seed {seed:#x}: following the alias lost {}",
                    rel.display()
                );
            }
            assert!(
                walked
                    .difference(&grown)
                    .all(|rel| rel.starts_with("~alias")),
                "seed {seed:#x}: an extra relative path is not led by the alias"
            );
        } else {
            assert_eq!(
                walked, grown,
                "seed {seed:#x}: the relative paths are not the grown tree"
            );
        }
        Ok(())
    })
}
