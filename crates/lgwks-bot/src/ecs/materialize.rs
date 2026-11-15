//! Materializing a spec chain: the registry's constructors turned into an erased
//! chain, with every unmet need recorded rather than the first.

use crate::cap::Demand;
use crate::gate::GrantSet;
use crate::registry::DomainRegistry;
use crate::spec::{ActionSpec, ChainEntry, ChainSpec, Need};

use super::resources::EcsChain;

/// Resolve one spec chain into an erased chain, or record every need it has.
///
/// Returns `None` when the chain contributed any need, so the caller knows not
/// to build it; the needs are appended to `needs` in declaration order. The
/// walk is deliberately exhaustive: an unknown source stops the chain (nothing
/// downstream of a source this binary cannot build is knowable), but a known
/// source with an unknown action still has its condition and its capability
/// requirements checked, because those are facts about the document rather than
/// about the missing action.
pub(super) fn materialize_chain(
    chain_index: usize,
    chain: &ChainSpec,
    registry: &DomainRegistry,
    grants: &GrantSet,
    needs: &mut Vec<Need>,
) -> Option<EcsChain> {
    let before = needs.len();

    let Some(source_ctor) = registry.source(&chain.source) else {
        needs.push(Need::UnknownSource {
            chain: chain_index,
            domain: chain.source.clone(),
        });
        return None;
    };
    let source = match source_ctor(&chain.target) {
        Ok(source) => source,
        Err(cause) => {
            needs.push(Need::SourceTargetRejected {
                chain: chain_index,
                domain: chain.source.clone(),
                cause: cause.to_string(),
            });
            return None;
        }
    };
    for shortage in grants.uncovered(source.required_caps(), &Demand::new(chain.source.clone())) {
        needs.push(Need::MissingCapability {
            chain: chain_index,
            action: None,
            domain: chain.source.clone(),
            capability: shortage.required().clone(),
        });
    }

    let mut entries: Vec<ChainEntry> = Vec::with_capacity(chain.on.len());
    for (action_index, entry) in chain.on.iter().enumerate() {
        let condition_id: &String = &entry.0;
        let action_spec: &ActionSpec = &entry.1;
        let action = match registry.action(&action_spec.domain) {
            Some(ctor) => match ctor(&action_spec.target) {
                Ok(action) => Some(action),
                Err(cause) => {
                    needs.push(Need::ActionTargetRejected {
                        chain: chain_index,
                        action: action_index,
                        domain: action_spec.domain.clone(),
                        cause: cause.to_string(),
                    });
                    None
                }
            },
            None => {
                needs.push(Need::UnknownAction {
                    chain: chain_index,
                    action: action_index,
                    domain: action_spec.domain.clone(),
                });
                None
            }
        };
        if let Some(ref action) = action {
            for shortage in grants.uncovered(
                action.required_caps(),
                &Demand::new(action_spec.domain.clone()),
            ) {
                needs.push(Need::MissingCapability {
                    chain: chain_index,
                    action: Some(action_index),
                    domain: action_spec.domain.clone(),
                    capability: shortage.required().clone(),
                });
            }
        }
        let condition = match source.condition(condition_id) {
            Ok(condition) => Some(condition),
            Err(_) => {
                needs.push(Need::UnknownCondition {
                    chain: chain_index,
                    action: action_index,
                    condition: condition_id.clone(),
                });
                None
            }
        };
        if let (Some(action), Some(condition)) = (action, condition) {
            entries.push(ChainEntry::erased(
                condition.into_evaluate_any(),
                action.into_execute_any(),
            ));
        }
    }

    if needs.len() == before {
        Some(EcsChain::from_registry(source, entries))
    } else {
        None
    }
}
