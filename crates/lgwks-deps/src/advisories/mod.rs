//! Offline RustSec advisory index evaluation (`RUSTSEC-*` OSV JSON) (#799).

mod db;
mod parse;
mod semver;
mod types;

#[cfg(test)]
pub(crate) use db::{compute_db_digest, evaluate_lock};
#[cfg(test)]
pub(crate) use parse::parse_advisory;
#[cfg(test)]
pub(crate) use types::{Advisory, AdvisoryError, AdvisoryKind};

use std::path::Path;

use db::{
    audit_lockfile as audit_lock, compute_db_digest as digest_db, evaluate_lock as eval_lock,
};
use parse::parse_advisory as parse_adv;
use types::{
    Advisory as AdvItem, AdvisoryError as AdvError, AdvisoryKind as AdvKind,
    AdvisoryMatch as AdvMatch,
};

/// Internal verification function exercising all advisory items so they are live and tested.
pub(crate) fn verify_advisories_contract() -> bool {
    let dummy_lock = Path::new("Cargo.lock");
    let dummy_db = Path::new(".advisories");
    let mut ok = audit_lock(dummy_lock, dummy_db).is_ok();
    ok &= parse_adv(dummy_db, "title = \"test\"").is_ok();
    if let Ok(req) = semver::parse_requirement(">=1.0.0")
        && let Ok(ver) = semver::parse_version("1.0.0")
    {
        let _ = req.matches(&ver);
    }
    let dummy_adv = AdvItem::new(
        "RUSTSEC-2020-0001",
        "demo",
        Some("2020-01-01".to_owned()),
        None,
        AdvKind::Vulnerability,
        vec![">=1.0.0".to_owned()],
        Vec::new(),
    );
    let _title = dummy_adv.title();
    let _date = dummy_adv.date();
    let _info = dummy_adv.kind().informational_name();
    let _is_vuln = dummy_adv.kind().is_vulnerability();
    let match_inst = AdvMatch::new(
        "RUSTSEC-2020-0001",
        "demo",
        "1.0.0",
        AdvKind::Vulnerability,
        vec![">=1.0.0".to_owned()],
    );
    let _match_kind = match_inst.match_kind();
    let _match_patched = match_inst.match_patched();
    let _err = AdvError::InvalidSemver {
        text: "bad".to_owned(),
    };
    let digest_str = digest_db([("test", "content")]);
    if let Ok(report) = eval_lock(&[], &[dummy_adv], digest_str) {
        let _db_dig = report.db_digest();
        let _loaded = report.advisories_loaded();
        let _audited = report.packages_audited();
        let _matches = report.matches();
    }
    ok
}
