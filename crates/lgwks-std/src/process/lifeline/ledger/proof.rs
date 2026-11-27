//! Kani proofs of the ledger's kernels (#1164, ADR 0091): for every value,
//! not only for the seeds a sim draws.
//!
//! `cargo kani -p lgwks_std --no-default-features --features process
//! --harness ledger_`. Kani proves the line codec and the detach rule over
//! every id and every crash cut; the seeded sims (`sim_ledger_*`) drive the
//! whole sweep, whose maps and strings CBMC does not finish in a gate's time.

use super::{owns_device, whole_lines};

/// The image paths a proof draws from, and no image.
fn any_image() -> Option<&'static str> {
    match kani::any::<u8>() {
        0 => Some("ram://8"),
        1 => Some("/i.dmg"),
        _ => None,
    }
}

/// Bytes a text proof draws.
const BYTES: usize = 4;

/// Crash consistency, for every text of up to [`BYTES`] bytes: the ledger
/// folds exactly the text before the last newline, so an owner killed mid-line
/// leaves a line that is never read, and a whole line is never dropped. The
/// sims (`sim_ledger_*`) fold real records cut at every byte above this kernel.
#[kani::proof]
#[kani::unwind(6)]
fn ledger_a_torn_last_line_is_never_read_and_a_whole_one_never_dropped() {
    let len = kani::any::<usize>();
    kani::assume(len <= BYTES);
    let mut bytes = [0_u8; BYTES];
    for byte in bytes.iter_mut().take(len) {
        *byte = match kani::any::<u8>() % 3 {
            0 => b'+',
            1 => b'\n',
            _ => b' ',
        };
    }
    let Some(Ok(text)) = bytes.get(..len).map(std::str::from_utf8) else {
        return;
    };
    let breaks = text.bytes().filter(|byte| *byte == b'\n').count();
    match whole_lines(text) {
        None => assert!(breaks == 0, "a text with a newline folded as none"),
        Some(whole) => {
            let tail = text.get(whole.len()..);
            assert!(text.starts_with(whole), "the fold read past the text");
            assert!(
                tail.is_some_and(|tail| tail.starts_with('\n')),
                "the fold did not end at a newline"
            );
            assert!(
                tail.and_then(|tail| tail.get(1..))
                    .is_some_and(|torn| !torn.contains('\n')),
                "the fold dropped a whole line"
            );
        }
    }
}

/// The detach rule, for every boot, live set and device state: a sweep detaches
/// a dead owner's device only on the boot it was held on, only when no live
/// record holds it, and only while it is still the image it was held as; and
/// it always detaches one that meets all three.
#[kani::proof]
#[kani::unwind(16)]
fn ledger_a_device_is_detached_exactly_when_its_dead_owner_still_owns_it() {
    let same_boot = kani::any::<bool>();
    let held_live = kani::any::<bool>();
    let held = any_image();
    let now = any_image();
    let Some(held) = held else {
        return;
    };
    let detached = owns_device(same_boot, held_live, held, now);
    if detached {
        assert!(same_boot, "a device of another boot was detached");
        assert!(!held_live, "a device a live owner holds was detached");
        assert!(
            now == Some(held),
            "a device now attached from another image was detached"
        );
    }
    if same_boot && !held_live && now == Some(held) {
        assert!(
            detached,
            "a device its dead owner still owns was left attached"
        );
    }
}
