//! Streams: FAT and mini-FAT reads, plus the test-image builder.
//!
//! [`CfbFile::stream_names`] lists the stream entries in directory order and
//! [`CfbFile::read_stream`] reads one by name: through the FAT, or through
//! the mini FAT rooted at the root storage when the size is below the header
//! cutoff and a mini FAT exists. [`build_image`] writes a version-3 image
//! holding the given streams: one directory chain however many sectors it
//! spans, and the root's children laid out as the red-black tree in MS-CFB
//! §2.6.4 name order that tree-walking readers follow. Its mini cutoff is
//! zero, so every stream reads back through the FAT path.

use std::collections::BTreeSet;

use super::super::limits::Limits;
use super::{
    CfbError, CfbFile, DIR_ENTRY_LEN, ENDOFCHAIN, FATSECT, FREESECT, HEADER_LEN, MINI_SECTOR_LEN,
    NOSTREAM, WRITER_CUTOFF,
};
use crate::legacy::cfb::fat::{sector_offset, walk_chain};
use crate::legacy::{get_u32_le, push_u16_le, push_u32_le};

impl CfbFile {
    /// Every type-2 directory entry's name, in the order the directory
    /// array holds them; storages and the root are left out.
    #[must_use]
    pub fn stream_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| entry.kind == 2)
            .map(|entry| entry.name.clone())
            .collect()
    }

    /// Reads one stream by name through the FAT or the mini FAT.
    ///
    /// # Errors
    ///
    /// [`CfbError::StreamMissing`] for an unknown name,
    /// [`CfbError::Truncated`] past the file, or [`CfbError::Limit`] past a
    /// ceiling.
    pub fn read_stream(&self, name: &str, limits: &Limits) -> Result<Vec<u8>, CfbError> {
        let Some(entry) = self.entries.iter().find(|entry| entry.name == name) else {
            let refusal = Err(CfbError::StreamMissing {
                name: String::from(name),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing a missing stream");
            return refusal;
        };
        if entry.kind != 2 {
            let refusal = Err(CfbError::StreamMissing {
                name: String::from(name),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing a non-stream entry");
            return refusal;
        }
        let cutoff = u64::from(self.cutoff);
        if entry.size < cutoff && !self.mini_fat.is_empty() {
            let Ok(size) = usize::try_from(entry.size) else {
                let refusal = Err(CfbError::Truncated { offset: 0, need: 1 });
                #[cfg(feature = "trace")]
                crate::trace::debug!("cfb: refusing a stream past the address width");
                return refusal;
            };
            return self.read_mini_stream(entry.start, size, limits);
        }
        let Ok(size) = usize::try_from(entry.size) else {
            let refusal = Err(CfbError::Truncated { offset: 0, need: 1 });
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing a stream past the address width");
            return refusal;
        };
        limits.check_bytes(size)?;
        if entry.start == ENDOFCHAIN || entry.start == FREESECT {
            if size == 0 {
                return Ok(Vec::new());
            }
            let refusal = Err(CfbError::Truncated {
                offset: 0,
                need: size,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing an empty start past a nonzero size");
            return refusal;
        }
        let mut out = self.read_chain(entry.start, limits)?;
        out.truncate(size);
        Ok(out)
    }

    /// Reads the mini FAT and the root storage's mini stream.
    pub(crate) fn read_mini_structures(
        &mut self,
        first_mini_fat: u32,
        mini_fat_sectors: u32,
        limits: &Limits,
    ) -> Result<(), CfbError> {
        if first_mini_fat == ENDOFCHAIN || first_mini_fat == FREESECT || mini_fat_sectors == 0 {
            return Ok(());
        }
        let chain = self.follow_chain(first_mini_fat, limits)?;
        let mut raw: Vec<u32> = Vec::new();
        for sector in chain.iter() {
            let at = sector_offset(*sector, self.sector_len, self.image.len())?;
            for index in 0..self.sector_len.saturating_div(4) {
                let position = at.saturating_add(index.saturating_mul(4));
                let Some(next) = get_u32_le(&self.image, position) else {
                    break;
                };
                raw.push(next);
            }
        }
        self.mini_fat = raw;
        let Some(root) = self.entries.iter().find(|entry| entry.kind == 5) else {
            return Ok(());
        };
        if root.start == ENDOFCHAIN || root.start == FREESECT {
            return Ok(());
        }
        let mut stream = self.read_chain(root.start, limits)?;
        let Ok(size) = usize::try_from(root.size) else {
            return Ok(());
        };
        stream.truncate(size);
        self.mini_stream = stream;
        Ok(())
    }

    /// Reads one small stream through the mini FAT from the root mini stream.
    fn read_mini_stream(
        &self,
        start: u32,
        size: usize,
        limits: &Limits,
    ) -> Result<Vec<u8>, CfbError> {
        if start == ENDOFCHAIN || start == FREESECT {
            if size == 0 {
                return Ok(Vec::new());
            }
            let refusal = Err(CfbError::Truncated {
                offset: 0,
                need: size,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing an empty mini start past a nonzero size");
            return refusal;
        }
        let chain = walk_chain(&self.mini_fat, start, limits)?;
        let mut out: Vec<u8> = Vec::new();
        for sector in chain.iter() {
            let Ok(position) = usize::try_from(*sector) else {
                let refusal = Err(CfbError::BadFat { sector: *sector });
                #[cfg(feature = "trace")]
                crate::trace::debug!("cfb: refusing a mini offset past the address width");
                return refusal;
            };
            let at = position.saturating_mul(MINI_SECTOR_LEN);
            let Some(window) = self.mini_stream.get(at..at.saturating_add(MINI_SECTOR_LEN)) else {
                let refusal = Err(CfbError::Truncated {
                    offset: at,
                    need: at.saturating_add(MINI_SECTOR_LEN),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!("cfb: refusing a mini sector past the mini stream");
                return refusal;
            };
            out.extend_from_slice(window);
        }
        out.truncate(size);
        Ok(out)
    }
}

/// Refuses a name MS-CFB §2.6.1 forbids: empty or past 31 UTF-16 units,
/// holding `/`, `\`, `:` or `!`, or equal to a sibling's when both are
/// uppercased, which is how readers compare them.
fn check_names(streams: &[(&str, &[u8])]) -> Result<(), CfbError> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for &(name, _) in streams {
        let refuse = |why: &str| CfbError::BadName {
            name: String::from(name),
            why: String::from(why),
        };
        let units = name.encode_utf16().count();
        if units == 0 || units > 31 {
            return crate::legacy::refuse(refuse("a name holds 1 to 31 UTF-16 units"));
        }
        if name.contains(['/', '\\', ':', '!']) {
            return crate::legacy::refuse(refuse("a name may not hold /, \\, : or !"));
        }
        if !seen.insert(name.to_uppercase()) {
            return crate::legacy::refuse(refuse("a sibling already holds this name"));
        }
    }
    Ok(())
}

/// Builds a minimal version-3 compound image holding `streams`.
///
/// One root entry, directory sectors, FAT sectors as needed, and every
/// stream chained through the FAT. The mini cutoff is zero, so readers take
/// the FAT path for every stream here.
///
/// # Errors
///
/// [`CfbError::BadName`] for a name MS-CFB forbids, [`CfbError::Limit`]
/// past a ceiling.
pub fn build_image(streams: &[(&str, &[u8])], limits: &Limits) -> Result<Vec<u8>, CfbError> {
    const SECTOR: usize = 512;
    check_names(streams)?;
    let mut total: usize = 0;
    for &(_, data) in streams.iter() {
        total = total.saturating_add(data.len().div_ceil(SECTOR).saturating_mul(SECTOR));
    }
    limits.check_bytes(total.saturating_add(HEADER_LEN))?;
    limits.check_records(streams.len().saturating_add(1))?;
    let dir_entries = streams.len().saturating_add(1);
    let dir_sectors = dir_entries.saturating_mul(DIR_ENTRY_LEN).div_ceil(SECTOR);
    let data_sectors = total.saturating_div(SECTOR);
    let chain_sectors = dir_sectors.saturating_add(data_sectors);
    let fat_entries = chain_sectors.saturating_add(2);
    let fat_sectors = fat_entries.saturating_mul(4).div_ceil(SECTOR);
    let mut image: Vec<u8> = Vec::new();
    image.extend_from_slice(&super::MAGIC);
    image.extend_from_slice(&[0_u8; 16]);
    push_u16_le(&mut image, 0x003e);
    push_u16_le(&mut image, 3);
    push_u16_le(&mut image, 0xfffe);
    push_u16_le(&mut image, 9);
    push_u16_le(&mut image, 6);
    image.extend_from_slice(&[0_u8; 6]);
    push_u32_le(&mut image, 0);
    push_u32_le(&mut image, narrow_sector_count(fat_sectors)?);
    push_u32_le(&mut image, narrow_sector_count(fat_sectors)?);
    push_u32_le(&mut image, 0);
    push_u32_le(&mut image, WRITER_CUTOFF);
    push_u32_le(&mut image, ENDOFCHAIN);
    push_u32_le(&mut image, 0);
    push_u32_le(&mut image, ENDOFCHAIN);
    push_u32_le(&mut image, 0);
    let fat_base = narrow_sector_count(fat_sectors)?;
    for index in 0..109_usize {
        if index < fat_sectors {
            push_u32_le(&mut image, narrow_sector_count(index)?);
        } else {
            push_u32_le(&mut image, FREESECT);
        }
    }
    while image.len() < HEADER_LEN {
        image.push(0);
    }
    let mut fat: Vec<u32> = vec![FATSECT; fat_sectors.max(1)];
    let mut next_sector: u32 = fat_base;
    let mut dir_starts: Vec<u32> = Vec::new();
    for _ in 0..dir_sectors {
        dir_starts.push(next_sector);
        next_sector = next_sector.saturating_add(1);
    }
    let mut stream_starts: Vec<u32> = Vec::new();
    let mut stream_sectors: Vec<Vec<u32>> = Vec::new();
    for &(_, data) in streams.iter() {
        let mut run: Vec<u32> = Vec::new();
        let wide = data.len().div_ceil(SECTOR);
        for _ in 0..wide {
            run.push(next_sector);
            next_sector = next_sector.saturating_add(1);
        }
        let start = match run.first() {
            Some(first) => *first,
            None => ENDOFCHAIN,
        };
        stream_starts.push(start);
        stream_sectors.push(run);
    }
    let total_sectors = match usize::try_from(next_sector) {
        Ok(drawn) => drawn,
        Err(_) => {
            let refusal = Err(CfbError::Truncated { offset: 0, need: 1 });
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing an image past the sector address width");
            return refusal;
        }
    };
    while fat.len() < total_sectors {
        fat.push(FREESECT);
    }
    for run in stream_sectors.iter() {
        let mut previous: Option<u32> = None;
        for sector in run.iter() {
            if let Some(done) = previous {
                fat_set(&mut fat, done, *sector)?;
            }
            previous = Some(*sector);
        }
        if let Some(last) = previous {
            fat_set(&mut fat, last, ENDOFCHAIN)?;
        }
    }
    // The directory is one chain, sector to sector, like a stream: a
    // directory that spills past one sector stays reachable from its head.
    for (sector, next) in dir_starts.iter().zip(dir_starts.iter().skip(1)) {
        fat_set(&mut fat, *sector, *next)?;
    }
    if let Some(last) = dir_starts.last() {
        fat_set(&mut fat, *last, ENDOFCHAIN)?;
    }
    for _ in 0..fat_sectors {
        image.extend_from_slice(&[0xff_u8; 512]);
    }
    let fat_at = image
        .len()
        .saturating_sub(fat_sectors.saturating_mul(SECTOR));
    for (index, next) in fat.iter().enumerate() {
        let at = fat_at.saturating_add(index.saturating_mul(4));
        write_u32_at(&mut image, at, *next);
    }
    let mut links = vec![TreeLinks::LEAF; streams.len()];
    let mut order: Vec<usize> = (0..streams.len()).collect();
    order.sort_by(|&left, &right| cfb_name_order(streams, left, right));
    let root_child = place_tree(&order, 0, red_depth(order.len()), &mut links)?;
    let mut dir_bytes: Vec<u8> = Vec::new();
    let root = TreeLinks {
        left: NOSTREAM,
        right: NOSTREAM,
        red: false,
    };
    write_dir_entry(
        &mut dir_bytes,
        "Root Entry",
        5,
        root,
        root_child,
        ENDOFCHAIN,
        0,
    )?;
    for ((&(name, data), &start), &link) in
        streams.iter().zip(stream_starts.iter()).zip(links.iter())
    {
        write_dir_entry(&mut dir_bytes, name, 2, link, NOSTREAM, start, data.len())?;
    }
    while dir_bytes.len() < dir_sectors.saturating_mul(SECTOR) {
        dir_bytes.push(0);
    }
    image.extend_from_slice(&dir_bytes);
    for (&(_, data), run) in streams.iter().zip(stream_sectors.iter()) {
        let mut padded = data.to_vec();
        while padded.len() < run.len().saturating_mul(SECTOR) {
            padded.push(0);
        }
        image.extend_from_slice(&padded);
    }
    Ok(image)
}

/// Narrows a sector count into the `u32` a header field holds.
fn narrow_sector_count(count: usize) -> Result<u32, CfbError> {
    let Ok(drawn) = u32::try_from(count) else {
        let refusal = Err(CfbError::Truncated { offset: 0, need: 1 });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a sector count past 32 bits");
        return refusal;
    };
    Ok(drawn)
}

/// Links one FAT entry: `sector` chains to `next`.
fn fat_set(fat: &mut [u32], sector: u32, next: u32) -> Result<(), CfbError> {
    let Ok(at) = usize::try_from(sector) else {
        let refusal = Err(CfbError::BadFat { sector });
        #[cfg(feature = "trace")]
        crate::trace::debug!(sector, "cfb: refusing a FAT write past the address width");
        return refusal;
    };
    let Some(slot) = fat.get_mut(at) else {
        let refusal = Err(CfbError::BadFat { sector });
        #[cfg(feature = "trace")]
        crate::trace::debug!(sector, "cfb: refusing a FAT write past the table");
        return refusal;
    };
    *slot = next;
    Ok(())
}

/// Writes a little-endian `u32` into an image at an absolute offset.
fn write_u32_at(image: &mut [u8], at: usize, value: u32) {
    let bytes = value.to_le_bytes();
    for (slot, byte) in bytes.iter().enumerate() {
        if let Some(cell) = image.get_mut(at.saturating_add(slot)) {
            *cell = *byte;
        }
    }
}

/// One directory entry's place in its storage's red-black tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TreeLinks {
    /// The left sibling's entry id, or `NOSTREAM`.
    left: u32,
    /// The right sibling's entry id, or `NOSTREAM`.
    right: u32,
    /// Whether the node is red; black otherwise.
    red: bool,
}

impl TreeLinks {
    /// A black node with no siblings.
    const LEAF: Self = Self {
        left: NOSTREAM,
        right: NOSTREAM,
        red: false,
    };
}

/// MS-CFB §2.6.4 sibling order: the shorter UTF-16 name first, then unit by
/// unit once both are uppercased.
fn cfb_name_order(streams: &[(&str, &[u8])], left: usize, right: usize) -> core::cmp::Ordering {
    let name_of = |index: usize| -> Vec<u16> {
        match streams.get(index) {
            Some(&(name, _)) => name.to_uppercase().encode_utf16().collect(),
            None => Vec::new(),
        }
    };
    let (first, second) = (name_of(left), name_of(right));
    first
        .len()
        .cmp(&second.len())
        .then_with(|| first.cmp(&second))
}

/// The depth whose nodes are red so every root-to-leaf path holds the same
/// black count: the one incomplete bottom level of a midpoint-built tree, or
/// none when the tree is perfect.
fn red_depth(count: usize) -> Option<usize> {
    let mut full_levels = 0_usize;
    let mut full_nodes = 0_usize;
    loop {
        let next = full_nodes.saturating_mul(2).saturating_add(1);
        if next > count {
            break;
        }
        full_nodes = next;
        full_levels = full_levels.saturating_add(1);
    }
    (full_nodes != count).then_some(full_levels)
}

/// Lays `order` (stream indices in sibling order) out as a balanced binary
/// search tree, midpoint at the root, red only at `red_at`; returns the
/// subtree root's entry id. Entry id is the stream index plus one, the root
/// storage holding id zero.
fn place_tree(
    order: &[usize],
    depth: usize,
    red_at: Option<usize>,
    links: &mut [TreeLinks],
) -> Result<u32, CfbError> {
    let (left, rest) = order.split_at(order.len().saturating_div(2));
    let Some((&node, right)) = rest.split_first() else {
        return Ok(NOSTREAM);
    };
    let below = depth.saturating_add(1);
    let left_id = place_tree(left, below, red_at, links)?;
    let right_id = place_tree(right, below, red_at, links)?;
    if let Some(slot) = links.get_mut(node) {
        *slot = TreeLinks {
            left: left_id,
            right: right_id,
            red: red_at == Some(depth),
        };
    }
    narrow_sector_count(node.saturating_add(1))
}

/// Appends one 128-byte directory entry: UTF-16LE name, kind, colour,
/// sibling and child links, span.
fn write_dir_entry(
    out: &mut Vec<u8>,
    name: &str,
    kind: u8,
    links: TreeLinks,
    child: u32,
    start: u32,
    size: usize,
) -> Result<(), CfbError> {
    let mut units: Vec<u16> = Vec::new();
    for drawn in name.encode_utf16() {
        if units.len() >= 31 {
            break;
        }
        units.push(drawn);
    }
    units.push(0);
    for unit in units.iter() {
        push_u16_le(out, *unit);
    }
    while out.len().rem_euclid(super::DIR_ENTRY_LEN) < 64 {
        out.push(0);
    }
    let name_len = units.len().saturating_mul(2);
    let Ok(name_len) = u16::try_from(name_len) else {
        let refusal = Err(CfbError::Truncated { offset: 0, need: 2 });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a directory name past 16 bits");
        return refusal;
    };
    push_u16_le(out, name_len);
    out.push(kind);
    out.push(if links.red { 0 } else { 1 });
    push_u32_le(out, links.left);
    push_u32_le(out, links.right);
    push_u32_le(out, child);
    out.extend_from_slice(&[0_u8; 16]);
    push_u32_le(out, 0);
    out.extend_from_slice(&[0_u8; 16]);
    push_u32_le(out, start);
    let wide = size.to_le_bytes();
    let (Some(low_window), Some(high_window)) = (wide.get(..4), wide.get(4..)) else {
        let refusal = Err(CfbError::Truncated { offset: 0, need: 8 });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a stream size past its window");
        return refusal;
    };
    let mut low_bytes = [0_u8; 4];
    let mut high_bytes = [0_u8; 4];
    low_bytes.copy_from_slice(low_window);
    high_bytes.copy_from_slice(high_window);
    push_u32_le(out, u32::from_le_bytes(low_bytes));
    push_u32_le(out, u32::from_le_bytes(high_bytes));
    while out.len().rem_euclid(super::DIR_ENTRY_LEN) != 0 {
        out.push(0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{CfbFile, stream};
    use crate::legacy::limits::Limits;

    /// The bounds the unit tests run under.
    const BOUNDS: Limits = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);

    /// What every fallible unit test returns.
    type UnitResult = Result<(), super::CfbError>;

    #[test]
    fn a_two_stream_image_round_trips() -> UnitResult {
        let streams = [
            ("Workbook", b"data-bytes".as_slice()),
            ("dir", b"dir!".as_slice()),
        ];
        let image = stream::build_image(&streams, &BOUNDS)?;
        let file = CfbFile::open(&image, &BOUNDS)?;
        assert_eq!(
            file.stream_names(),
            vec![String::from("Workbook"), String::from("dir")],
            "both streams must be listed"
        );
        assert_eq!(
            file.read_stream("Workbook", &BOUNDS)?,
            b"data-bytes",
            "the Workbook bytes must read back"
        );
        assert_eq!(
            file.read_stream("dir", &BOUNDS)?,
            b"dir!",
            "the dir bytes must read back"
        );
        Ok(())
    }

    /// MS-CFB §2.6.1: names are 1-31 UTF-16 units, free of `/ \ : !`, and
    /// unique among siblings once uppercased.
    #[test]
    fn a_name_the_format_forbids_is_refused_by_name() {
        let long = "N".repeat(32);
        for (streams, bad) in [
            (vec![("", b"x".as_slice())], ""),
            (vec![(long.as_str(), b"x".as_slice())], long.as_str()),
            (vec![("VBA/dir", b"x".as_slice())], "VBA/dir"),
            (vec![("Sheet!1", b"x".as_slice())], "Sheet!1"),
            (
                vec![("Data", b"x".as_slice()), ("DATA", b"y".as_slice())],
                "DATA",
            ),
        ] {
            assert!(
                matches!(
                    stream::build_image(&streams, &BOUNDS),
                    Err(super::CfbError::BadName { ref name, .. }) if name == bad
                ),
                "{bad:?} must be refused by name"
            );
        }
    }

    /// The node entry id `id` names, with its index, or `None` for `NOSTREAM`.
    fn node_at(links: &[super::TreeLinks], id: u32) -> Option<(usize, super::TreeLinks)> {
        if id == super::NOSTREAM {
            return None;
        }
        let index = usize::try_from(id).ok()?.checked_sub(1)?;
        links.get(index).map(|node| (index, *node))
    }

    /// Walks the subtree at `id` in order, appending each node and checking
    /// MS-CFB §2.6.4's red-black rules: no red node has a red child, and every
    /// path down holds the same black count. Returns that black count.
    fn walk(links: &[super::TreeLinks], id: u32, out: &mut Vec<usize>) -> usize {
        let Some((index, node)) = node_at(links, id) else {
            assert_eq!(id, super::NOSTREAM, "link {id} must name a node");
            return 1;
        };
        for child in [node.left, node.right] {
            if let Some((_, below)) = node_at(links, child) {
                assert!(
                    !(node.red && below.red),
                    "a red node at {index} must not have a red child"
                );
            }
        }
        let left = walk(links, node.left, out);
        out.push(index);
        let right = walk(links, node.right, out);
        assert_eq!(
            left, right,
            "both sides of node {index} must hold the same black count"
        );
        left.saturating_add(usize::from(!node.red))
    }

    /// Every sibling count from 0 to 300 lays out as a valid red-black tree
    /// whose in-order walk is MS-CFB name order.
    #[test]
    fn every_sibling_count_is_a_valid_red_black_tree_in_name_order() -> UnitResult {
        for count in 0..300_usize {
            let names: Vec<String> = (0..count)
                .map(|index| format!("S{}", count.saturating_sub(index)))
                .collect();
            let streams: Vec<(&str, &[u8])> = names
                .iter()
                .map(|name| (name.as_str(), b"".as_slice()))
                .collect();
            let mut order: Vec<usize> = (0..count).collect();
            order.sort_by(|&left, &right| super::cfb_name_order(&streams, left, right));
            let mut links = vec![super::TreeLinks::LEAF; count];
            let root = super::place_tree(&order, 0, super::red_depth(count), &mut links)?;
            if let Some((_, top)) = node_at(&links, root) {
                assert!(!top.red, "{count} siblings: the tree root must be black");
            }
            let mut visited = Vec::new();
            walk(&links, root, &mut visited);
            assert_eq!(
                visited, order,
                "{count} siblings: the in-order walk must be name order"
            );
        }
        Ok(())
    }

    /// A directory past one sector chains, and the root's child tree reaches
    /// every stream, as an MS-CFB reader walking the tree finds them.
    #[test]
    fn a_multi_sector_directory_reaches_every_stream_through_the_tree() -> UnitResult {
        let names: Vec<String> = (0..40).map(|index| format!("Stream{index}")).collect();
        let datas: Vec<Vec<u8>> = (0..40_u8)
            .map(|index| vec![index; usize::from(index)])
            .collect();
        let streams: Vec<(&str, &[u8])> = names
            .iter()
            .zip(datas.iter())
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect();
        let file = CfbFile::open(&stream::build_image(&streams, &BOUNDS)?, &BOUNDS)?;
        for (name, data) in names.iter().zip(datas.iter()) {
            assert_eq!(
                &file.read_stream(name, &BOUNDS)?,
                data,
                "{name} must read back"
            );
        }
        let mut reached = Vec::new();
        let mut dangling = Vec::new();
        let mut pending: Vec<u32> = file
            .entries
            .first()
            .map(|root| root.child)
            .into_iter()
            .collect();
        while let Some(id) = pending.pop() {
            if id == super::NOSTREAM {
                continue;
            }
            match usize::try_from(id)
                .ok()
                .and_then(|drawn| file.entries.get(drawn))
            {
                Some(entry) => {
                    reached.push(entry.name.clone());
                    pending.push(entry.left);
                    pending.push(entry.right);
                }
                None => dangling.push(id),
            }
        }
        assert!(
            dangling.is_empty(),
            "every tree link must name an entry: {dangling:?}"
        );
        reached.sort_unstable();
        let mut expected = names.clone();
        expected.sort_unstable();
        assert_eq!(
            reached, expected,
            "the root's tree must reach all 40 streams"
        );
        Ok(())
    }

    #[test]
    fn an_encryption_info_stream_marks_encryption() -> UnitResult {
        let streams = [("EncryptionInfo", b"locked".as_slice())];
        let image = stream::build_image(&streams, &BOUNDS)?;
        assert!(
            matches!(
                CfbFile::open(&image, &BOUNDS),
                Err(super::CfbError::Encrypted)
            ),
            "an EncryptionInfo stream must mark an encrypted file"
        );
        Ok(())
    }
}
