//! A `script!` flow validates a tool call through a storefront engine, with no new script word.

use lgwks_bot::script::{Scope, Tenant};
use lgwks_deps::jsonschema::Validator;
use lgwks_std::json::{Value, json};

lgwks_bot::script! {
    pub flow admits(validator: &Validator, call: &Value) -> bool:
        give back validator.is_valid(&call.clone().into())
}

fn main() {
    if !journey() {
        std::process::exit(1);
    }
}

fn journey() -> bool {
    let schema: Value = json!({
        "type": "object",
        "properties": {"action": {"enum": ["read", "write"]}},
        "required": ["action"]
    });
    let Ok(validator) = lgwks_deps::jsonschema::validator_for(&schema.into()) else {
        return false;
    };
    let Ok(tenant) = Tenant::new("acme") else {
        return false;
    };
    let scope = Scope::root(tenant);
    let good: Value = json!({"action": "read"});
    let bad: Value = json!({"action": "delete"});
    let admitted = lgwks_bot::block_on(admits(&scope, &validator, &good));
    let refused = lgwks_bot::block_on(admits(&scope, &validator, &bad));
    matches!((admitted, refused), (Ok(true), Ok(false)))
}
