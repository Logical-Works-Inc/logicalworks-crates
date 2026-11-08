//! The array grammar RustSec's advisories use, against what the format says it
//! means: `rustsec/advisory-db` writes commas inside one item as AND.

use std::path::Path;

use super::parse_advisory;
use crate::advisories::AdvisoryError;

/// Catches the fail-open the split-on-comma parser shipped with, where `>= 1.0.1`
/// alone read every later version as patched.
#[test]
fn sim_parse_weird_bytes_a_comma_inside_one_requirement_is_a_conjunction()
-> Result<(), AdvisoryError> {
    let content = r#"```toml
[advisory]
id = "RUSTSEC-2026-0003"
package = "vuln"

[versions]
patched = [">= 1.0.1, < 1.1.0", ">= 1.1.3"]
unaffected = ['< 0.5, >= 0.1']
```
"#;
    let path = Path::new("crates/vuln/RUSTSEC-2026-0003.md");
    let advisory = parse_advisory(path, content)?;
    assert_eq!(
        advisory.patched(),
        &[">= 1.0.1, < 1.1.0".to_owned(), ">= 1.1.3".to_owned()],
        "two alternatives, never three"
    );
    assert_eq!(
        advisory.unaffected(),
        &["< 0.5, >= 0.1".to_owned()],
        "a single-quoted conjunction stays one item"
    );
    Ok(())
}
