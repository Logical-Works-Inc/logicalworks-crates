//! The nodes of the steps that call out of a flow: `observe` and `act`
//! against a registered domain, and the outside words `fetch`, `ask` and
//! `send`. Each keeps its arguments as written; what they mean is the
//! runtime's, read through the registry or the capability the word declares.

use super::super::lexicon::Kind;
use super::{Code, Site};

/// `observe domain::id of target` or `act domain::id on target with value`: a
/// registry identifier called in this scope, as a step of its own (#388).
///
/// The identifier is kept as written, `::` and all, because it is the key the
/// host's `DomainRegistry` declares and the text a refusal names; it is never
/// resolved here. Resolution is the runtime's, at the flow's entry, against
/// the registry the run was given, so a flow refers to a domain by the same
/// string a `BotSpec` does and the two cannot drift.
#[derive(Debug, Clone)]
pub struct Domain {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// Whether it observes (`observe`) or acts (`act`).
    pub(crate) kind: Kind,
    /// The registry identifier, as written without spaces: `github::pr_status`.
    pub(crate) id: String,
    /// The structural label: the identifier's last segment, numbered among
    /// same-named steps in one scope.
    pub(crate) label: String,
    /// What the domain is built from: after `of` for `observe`, between `on`
    /// and `with` for `act`.
    pub(crate) target: Code,
    /// What an action is handed, after `with`; `None` for `observe`.
    pub(crate) value: Option<Code>,
}

impl Domain {
    /// Whether it observes ([`Kind::Observe`]) or acts ([`Kind::Act`]).
    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.kind
    }

    /// The registry identifier, as written without spaces.
    #[must_use]
    pub const fn id(&self) -> &str {
        self.id.as_str()
    }

    /// The structural label: the identifier's last segment, numbered among
    /// same-named steps in one scope.
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// What the domain is built from.
    #[must_use]
    pub const fn target(&self) -> &Code {
        &self.target
    }

    /// What an action is handed; `None` for an observation.
    #[must_use]
    pub const fn value(&self) -> Option<&Code> {
        self.value.as_ref()
    }
}

/// `fetch <url> [with (options)]`, `ask <model> with (request)` or
/// `send <receiver> with <payload>`: an outside call in this scope, as a step
/// of its own (#783).
///
/// Like [`Domain`], the call's arguments are kept as written: the URL, model
/// or receiver expression and the options, request or payload beside it. What
/// they mean — the deadline, the retry budget, the size bound, the token
/// budget — is the runtime's, which reads the same values through the
/// capability the word declares, so a flow refers to an outside call by the
/// same strings its admission checked.
#[derive(Debug, Clone)]
pub struct Outside {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// Whether it fetches, asks or sends.
    pub(crate) kind: Kind,
    /// The structural label: the word, numbered among same-named steps in one
    /// scope.
    pub(crate) label: String,
    /// What the call reaches: the URL, the model or the receiver.
    pub(crate) target: Code,
    /// The word's declared bounds — the options, request or payload — or
    /// `None` for a `fetch` that inherits them from its enclosing blocks.
    pub(crate) options: Option<Code>,
}

impl Outside {
    /// Whether it fetches, asks or sends.
    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.kind
    }

    /// The structural label: the word, numbered among same-named steps in one
    /// scope.
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// What the call reaches: the URL, the model or the receiver.
    #[must_use]
    pub const fn target(&self) -> &Code {
        &self.target
    }

    /// The word's declared bounds, or `None` for a `fetch` that inherits them.
    #[must_use]
    pub const fn options(&self) -> Option<&Code> {
        self.options.as_ref()
    }
}
