//! What the driver and the Restate worker both read and write: the workflow key,
//! the park notice word and the failure plan file.
//!
//! Both binaries include this one file by path, so the two sides cannot spell a
//! key two ways. Restate has no tenancy: a run's key is `<tenant>-<run>`, and a
//! key reads back to its tenant and run only in that one canonical spelling.

/// The first word of the line the worker prints when a run reaches its approval.
pub(crate) const PARKED_WORD: &str = "parked";

/// The workflow key of run `ordinal` of `tenant`.
pub(crate) fn key_of(tenant: &str, ordinal: u32) -> String {
    format!("{tenant}-{ordinal}")
}

/// The tenant and run of a workflow key, or `None` for anything but the one
/// canonical spelling [`key_of`] writes: no sign, no padding zeros, no tenant
/// left empty.
pub(crate) fn parse_key(key: &str) -> Option<(&str, u32)> {
    let (tenant, run) = key.rsplit_once('-')?;
    let ordinal: u32 = run.parse().ok()?;
    (!tenant.is_empty() && key_of(tenant, ordinal) == key).then_some((tenant, ordinal))
}

/// The failure plan a file holds: line `n` is `<outside> <model>`, the failing
/// attempts of run `n`, or `None` when any line is not exactly that.
pub(crate) fn parse_plan(text: &str) -> Option<Vec<(u32, u32)>> {
    let mut plan = Vec::new();
    for line in text.lines() {
        let mut words = line.split(' ');
        let (outside, model, rest) = (words.next()?, words.next()?, words.next());
        if rest.is_some() {
            return None;
        }
        plan.push((outside.parse().ok()?, model.parse().ok()?));
    }
    Some(plan)
}
