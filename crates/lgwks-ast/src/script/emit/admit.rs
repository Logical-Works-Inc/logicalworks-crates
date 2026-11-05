//! What a flow admits before its first step, and the runtime map's entries.
//!
//! A flow that names a domain or an outside word asks the runtime to admit
//! those uses once, up front, so a refused use fails before any step runs. The
//! map entries are the `FlowShape` and `StepShape` values `lgwks_bot::script`
//! reports a flow's structure through.

use super::super::{FlowShape, Kind, StepShape};
use super::{Result, line_literal, newer_word, runtime};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

/// The admission a flow runs before its first step when it names any domain.
pub(super) fn domain_admission(steps: &[StepShape]) -> Result<TokenStream> {
    let mut uses: Vec<(Kind, &str)> = Vec::new();
    collect_domains(steps, &mut uses);
    if uses.is_empty() {
        return Ok(TokenStream::new());
    }
    let script = runtime();
    let mut entries = Vec::new();
    for (kind, id) in uses {
        entries.push(match kind {
            Kind::Observe => quote!(#script::DomainUse::observe(#id)),
            Kind::Act => quote!(#script::DomainUse::act(#id)),
            _ => {
                let refusal = newer_word("domain call");
                tracing::debug!(error = %refusal, "domain_admission: returning an error to the caller");
                return Err(refusal);
            }
        });
    }
    Ok(quote!(#script::admit_domains(scope, &[#(#entries),*])?;))
}

/// Every domain step in `steps` and their children, in source order.
fn collect_domains<'shape>(steps: &'shape [StepShape], uses: &mut Vec<(Kind, &'shape str)>) {
    for step in steps {
        let kind = step.kind();
        if matches!(kind, Kind::Observe | Kind::Act) {
            let used = (kind, step.subject());
            if !uses.contains(&used) {
                uses.push(used);
            }
        }
        collect_domains(step.children(), uses);
    }
}

/// The admission a flow runs before its first step when it names any outside word.
pub(super) fn outside_admission(steps: &[StepShape]) -> Result<TokenStream> {
    let mut uses: Vec<Kind> = Vec::new();
    collect_outside(steps, &mut uses);
    if uses.is_empty() {
        return Ok(TokenStream::new());
    }
    let script = runtime();
    let mut entries = Vec::new();
    for kind in uses {
        entries.push(match kind {
            Kind::Fetch => quote!(#script::OutsideWord::Fetch),
            Kind::Ask => quote!(#script::OutsideWord::Ask),
            Kind::Send => quote!(#script::OutsideWord::Send),
            _ => {
                let refusal = newer_word("outside call");
                tracing::debug!(error = %refusal, "outside_admission: returning an error to the caller");
                return Err(refusal);
            }
        });
    }
    Ok(quote!(#script::admit_outside(scope, &[#(#entries),*])?;))
}

/// Every outside step in `steps` and their children, in source order.
fn collect_outside(steps: &[StepShape], uses: &mut Vec<Kind>) {
    for step in steps {
        let kind = step.kind();
        if matches!(kind, Kind::Fetch | Kind::Ask | Kind::Send) && !uses.contains(&kind) {
            uses.push(kind);
        }
        collect_outside(step.children(), uses);
    }
}

/// A flow's `FlowShape::new(..)` entry in the map.
pub(super) fn flow_shape(shape: &FlowShape) -> TokenStream {
    let script = runtime();
    let name = shape.name();
    let signature = shape.signature();
    let line = line_literal(shape.line());
    let steps = shape.steps().iter().map(step_shape);
    let passthrough = shape.passthrough().iter().map(|passed| {
        let line = line_literal(passed.line());
        let text = passed.text();
        quote!(#script::Passthrough::new(#line, #text))
    });
    quote! {
        #script::FlowShape::new(#name, #signature, #line, &[#(#steps),*], &[#(#passthrough),*])
    }
}

/// A step's `StepShape::new(..)` entry in the map.
fn step_shape(shape: &StepShape) -> TokenStream {
    let script = runtime();
    let kind = Ident::new(&format!("{:?}", shape.kind()), Span::call_site());
    let subject = shape.subject();
    let detail = shape.detail();
    let line = line_literal(shape.line());
    let children = shape.children().iter().map(step_shape);
    quote! {
        #script::StepShape::new(#script::StepKind::#kind, #subject, #detail, #line, &[#(#children),*])
    }
}
