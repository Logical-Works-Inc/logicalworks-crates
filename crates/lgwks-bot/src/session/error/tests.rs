//! The session layer's error: wrapping, classification and rendering.

#[cfg(test)]
use super::SessionError;
use crate::error::{DispatchCertainty, KernelError, RetryClass};
use crate::session::Terminal;

/// One payload carrying every class the rendering contract has to handle.
const HOSTILE: &str = "a\nb\rc\td\u{1b}[2J e\"f\\g ünïcode 日本";

#[test]
fn a_kernel_failure_wraps_into_the_session_layer_unchanged() {
    let kernel = KernelError::EffectIndeterminate {
        domain: String::from("gh::merge"),
        cause: String::from("timed out"),
    };
    let rendered = kernel.to_string();
    let wrapped = SessionError::from(kernel);

    assert!(
        matches!(
            wrapped,
            SessionError::Kernel(KernelError::EffectIndeterminate { .. })
        ),
        "the kernel failure must ride inside the Kernel arm: {wrapped}"
    );
    assert_eq!(wrapped.to_string(), rendered);
    assert_eq!(wrapped.retry_class(), RetryClass::RequiresEvidence);
}

#[test]
fn a_session_refusal_is_a_refusal_before_any_effect() {
    let refusals = [
        SessionError::SessionTerminated,
        SessionError::SessionNotAwaitingAnswer,
        SessionError::PredicateTypeMismatch,
        SessionError::MissingTransition {
            node: String::from("start"),
        },
    ];
    for error in refusals {
        assert_eq!(error.dispatch_certainty(), DispatchCertainty::Refused);
        assert_eq!(error.retry_class(), RetryClass::Never, "{error}");
    }
}

/// Every session variant that interpolates a `String` payload.
fn hostile_arms() -> Vec<SessionError> {
    let payload = String::from(HOSTILE);
    vec![
        SessionError::MalformedFlow {
            cause: payload.clone(),
        },
        SessionError::InvalidTransitionTarget {
            from: payload.clone(),
            target: payload.clone(),
        },
        SessionError::MissingAskRoute {
            node: payload.clone(),
            option: payload.clone(),
        },
        SessionError::VariableNeverWritten {
            name: payload.clone(),
        },
        SessionError::VariableReadBeforeInit {
            node: payload.clone(),
            name: payload.clone(),
        },
        SessionError::UndeclaredVariable {
            name: payload.clone(),
        },
        SessionError::UnreachableNode {
            node: payload.clone(),
        },
        SessionError::UnknownNodeKind {
            node: payload.clone(),
            kind: payload.clone(),
        },
        SessionError::MissingTransition {
            node: payload.clone(),
        },
        SessionError::MalformedTemplate {
            node: payload.clone(),
            field: "label",
        },
        SessionError::VariableTypeMismatch {
            name: payload.clone(),
        },
        // One row covers both terminal payloads: `HandedOff` carries the
        // node's target and `Refused` the declared reason, and both reach
        // the rendering site.
        SessionError::ConflictingTerminalDeclaration {
            node: payload.clone(),
            intrinsic: Terminal::HandedOff {
                target: payload.clone(),
            },
            declared: Terminal::Refused {
                reason: payload.clone(),
            },
        },
        SessionError::AskOptionNotAssignable {
            node: payload.clone(),
            variable: payload.clone(),
            option: payload.clone(),
            expected: "boolean",
            cause: payload.clone(),
        },
        SessionError::VariableUnset {
            name: payload.clone(),
        },
        SessionError::InvalidVariableValue {
            variable: payload.clone(),
            value: payload.clone(),
            reason: payload.clone(),
        },
        SessionError::ResolverReturnedInvalidOption { node: payload },
    ]
}

#[test]
fn every_untrusted_session_field_is_escaped() {
    for error in hostile_arms() {
        let rendered = error.to_string();
        assert!(
            !rendered.chars().any(char::is_control),
            "a variant rendered a live control character: {rendered:?}"
        );
        assert!(
            !rendered.contains('\n'),
            "a variant rendered a second physical line: {rendered:?}"
        );
    }
}
