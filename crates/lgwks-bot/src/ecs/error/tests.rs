//! The world layer's error: wrapping, classification and rendering.

#[cfg(test)]
use std::time::Duration;

use super::EcsError;
use crate::error::{DispatchCertainty, KernelError, RetryClass};

/// One payload carrying every class the rendering contract has to handle.
const HOSTILE: &str = "a\nb\rc\td\u{1b}[2J e\"f\\g ünïcode 日本";

fn domain_error(certainty: DispatchCertainty) -> KernelError {
    KernelError::DomainError {
        domain: String::from("gh::merge"),
        certainty,
        cause: String::from("connection closed"),
    }
}

#[test]
fn a_kernel_failure_wraps_into_the_world_layer_unchanged() {
    let wrapped = EcsError::from(domain_error(DispatchCertainty::NotDelivered));

    assert!(
        matches!(
            wrapped,
            EcsError::Kernel(KernelError::DomainError {
                certainty: DispatchCertainty::NotDelivered,
                ..
            })
        ),
        "the kernel failure must ride inside the Kernel arm: {wrapped}"
    );
    assert_eq!(
        wrapped.to_string(),
        domain_error(DispatchCertainty::NotDelivered).to_string(),
        "wrapping must not change the rendered line"
    );
}

#[test]
fn a_wrapped_kernel_failure_keeps_its_retry_class() {
    let cases = [
        (DispatchCertainty::Refused, RetryClass::Never),
        (DispatchCertainty::NotDelivered, RetryClass::Safe),
    ];
    for (certainty, expected) in cases {
        let wrapped = EcsError::from(domain_error(certainty));
        assert_eq!(wrapped.retry_class(), expected, "{wrapped}");
        assert_eq!(wrapped.dispatch_certainty(), certainty, "{wrapped}");
    }
    let indeterminate = EcsError::from(KernelError::EffectIndeterminate {
        domain: String::from("gh::merge"),
        cause: String::from("timed out"),
    });
    assert_eq!(indeterminate.retry_class(), RetryClass::RequiresEvidence);
}

#[test]
fn the_world_layers_own_arms_classify_without_reading_a_cause() {
    assert_eq!(EcsError::TickInsideRuntime.retry_class(), RetryClass::Never);
    assert_eq!(
        EcsError::TickInsideRuntime.dispatch_certainty(),
        DispatchCertainty::Refused
    );
    let stalled = EcsError::PollStalled {
        chain: 0,
        deadline: Duration::from_secs(1),
    };
    assert_eq!(stalled.retry_class(), RetryClass::Safe, "{stalled}");
    let wiring = EcsError::EvaluateError {
        cause: String::from("not a bool"),
    };
    assert_eq!(wiring.retry_class(), RetryClass::Never, "{wiring}");
}

#[test]
fn every_untrusted_world_field_is_escaped() {
    let payload = String::from(HOSTILE);
    let arms = [
        EcsError::MalformedSpec {
            cause: payload.clone(),
        },
        EcsError::IncompleteSpec {
            field: "target",
            cause: payload.clone(),
        },
        EcsError::UnknownCondition {
            condition: payload.clone(),
            argument_parse: payload.clone(),
        },
        EcsError::UnregisteredDomain {
            domain: payload.clone(),
        },
        EcsError::DuplicateDomain {
            domain: payload.clone(),
            role: "source",
            first: 0,
            second: 1,
        },
        EcsError::EvaluateError { cause: payload },
    ];
    for error in arms {
        let rendered = error.to_string();
        assert!(
            !rendered.chars().any(char::is_control),
            "a variant rendered a live control character: {rendered:?}"
        );
    }
}
