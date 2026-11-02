//! From the script tree to Rust: every word becomes one call into
//! `lgwks_bot::script`.
//!
//! The emitted code is meant to be read. A flow expands to an `async fn` whose
//! body is a sequence of `each(..)`, `within(..)`, `retry(..)` and
//! `try_join!(..)` calls with ordinary Rust between them, so `cargo expand`
//! shows the orchestration the script declared and nothing it did not.
//!
//! Nothing here reads the language. [`super::parse`] has already
//! read every word, decided every refusal and chosen every step label (labels
//! are structural and never carry a line number; see that module), so this
//! module only writes Rust for a tree it is handed. The one thing it can still
//! refuse is a word its version does not generate: a tree from a newer
//! `lgwks_ast` than this crate was released with.
//!
//! This is the one emitter of the language. `lgwks_macros::script!` is a thin
//! shell over [`super::parse`] then [`script`], and `lgwks expand` prints what
//! [`script`] answers, formatted, so the macro and the tool never become two
//! code paths.

use super::{
    Block, Bound, BranchValue, Call, Code, Construct, Domain, Duration, Each, Flow, FlowShape, For,
    Fragment, IfChain, Kind, Outside, Retry, Script, Site, Statement, Step, StepShape, Together,
    Within,
};
use super::{Refusal, Result};
use proc_macro2::{Group, Ident, Literal, Span, TokenStream, TokenTree};
use quote::{ToTokens, quote};

mod serve;
mod vocab;

/// The runtime every expansion calls into.
fn runtime() -> TokenStream {
    quote!(::lgwks_bot::script)
}

/// The refusal for a word this crate does not generate: the tree came from a
/// newer `lgwks_ast` than the one this crate was released with.
fn newer_word(what: &str) -> Refusal {
    Refusal::new(
        Span::call_site(),
        format!(
            "this script uses a {what} that this `lgwks_ast` reads but does not generate; \
             use the `lgwks_macros` released with the `lgwks_ast` that wrote it"
        ),
    )
}

/// `Result<T, FlowError>` spelled out, so the expansion names nothing the
/// caller must import.
///
/// A value that ends in `?` is bound first: `Ok(x?)` is what a reader (and
/// `clippy::needless_question_mark`) flags, yet the `?` must stay, because it
/// is what converts a verb's `BotError` or an `io::Error` into a flow failure.
fn ok(value: &TokenStream) -> TokenStream {
    let script = runtime();
    let ends_in_question = value.clone().into_iter().last().is_some_and(
        |token| matches!(token, TokenTree::Punct(ref punct) if punct.as_char() == '?'),
    );
    if ends_in_question {
        quote!({
            let flow_value = #value;
            ::core::result::Result::Ok::<_, #script::FlowError>(flow_value)
        })
    } else {
        quote!(::core::result::Result::Ok::<_, #script::FlowError>(#value))
    }
}

/// The lints every generated flow carries at `forbid`, so the consumer's
/// compiler (`unsafe_code`) and clippy (the rest) refuse by resolved path what
/// the parser's token check can only refuse by spelling. `forbid` rather than
/// `deny` because an `#[allow]` inside the flow cannot lower it.
fn lint_forbids() -> TokenStream {
    quote! {
        #[forbid(
            unsafe_code,
            clippy::unwrap_used,
            clippy::expect_used,
            clippy::panic,
            clippy::todo,
            clippy::unimplemented,
            clippy::unreachable,
            clippy::indexing_slicing,
            clippy::exit,
            clippy::mem_forget,
            clippy::panic_in_result_fn
        )]
    }
}

/// A block's emitted body: its statements and its value.
struct Body {
    /// Every statement but the value, each ending its own statement.
    statements: TokenStream,
    /// The value, and whether it is a flow `Result` rather than the value.
    tail: Option<(TokenStream, bool)>,
    /// Whether the last line leaves the block, so no value follows it.
    diverges: bool,
    /// The flow result a closing `give back` or `fail` answers, held apart
    /// from the statements: the tail of a block that is itself the flow's
    /// result, a `return` everywhere else. A `return` in tail position is
    /// what clippy's `needless_return` refuses in the expanded text.
    exit: Option<TokenStream>,
}

impl Body {
    /// `{ statements; Ok(tail) }` for an async body that returns a flow result.
    fn into_ok_block(self) -> TokenStream {
        let Self {
            statements,
            tail,
            diverges,
            exit,
        } = self;
        let value = match (exit, tail) {
            (Some(exit), _) => exit,
            _ if diverges => TokenStream::new(),
            (None, Some((tokens, true))) => tokens,
            (None, Some((tokens, false))) => ok(&tokens),
            (None, None) => ok(&quote!(())),
        };
        quote!({ #statements #value })
    }

    /// `{ statements; tail }` for a plain Rust block, which a closing
    /// `give back` or `fail` leaves by `return`.
    fn into_block(self) -> TokenStream {
        let Self {
            statements,
            tail,
            exit,
            ..
        } = self;
        let value = match exit {
            Some(exit) => Some(quote!(return #exit;)),
            None => tail.map(|(tokens, fallible)| if fallible { quote!(#tokens?) } else { tokens }),
        };
        quote!({ #statements #value })
    }
}

/// Expand a whole script: every flow, then the architecture map.
///
/// This is the one emitter of the language (SL-2, #787): the `script!`
/// proc macro calls it on the tree [`super::parse`] answers, and `lgwks
/// expand` prints what it answers, formatted, so what the tool prints is
/// byte-for-byte what the macro compiles.
///
/// # Errors
///
/// A word this crate's version does not generate.
pub fn script(script: &Script) -> Result<TokenStream> {
    let mut output = TokenStream::new();
    let mut flows = Vec::new();
    for flow in script.flows() {
        output.extend(flow_code(flow)?);
        flows.push(flow_shape(flow.shape()));
    }
    let runtime = runtime();
    output.extend(quote! {
        /// The orchestration this `script!` block declares, compiled from the
        /// same tokens as its flows. See `lgwks_bot::script::Architecture`.
        pub const ARCHITECTURE: #runtime::Architecture = #runtime::Architecture::new(&[#(#flows),*]);
    });
    Ok(output)
}

/// One flow as an `async fn`.
fn flow_code(flow: &Flow) -> Result<TokenStream> {
    let body_block = body(flow.body())?.into_ok_block();
    let script = runtime();
    let name = flow.name();
    let name_text = name.to_string();
    let inputs = flow.inputs();
    let inputs = if inputs.is_empty() {
        quote!(scope: &#script::Scope)
    } else {
        quote!(scope: &#script::Scope, #inputs)
    };
    let unit = quote!(());
    // A flow that promises no output type returns `()`, which is the declared
    // output of a flow whose signature has no `->`. Matched rather than
    // substituted for a missing type: the two arms are different signatures.
    let output = match flow.output() {
        Some(ty) => ty.clone(),
        None => unit,
    };
    let attributes = flow.attributes();
    let has_doc = attributes.clone().into_iter().any(|token| {
        matches!(token, TokenTree::Group(ref group) if group.stream().into_iter().next().is_some_and(|first| matches!(first, TokenTree::Ident(ref word) if word == "doc")))
    });
    let doc = if has_doc {
        TokenStream::new()
    } else {
        let text = format!(" Flow `{name_text}`, written with `script!`.");
        quote!(#[doc = #text])
    };
    // The helper traits are imported only where a flow calls them, so an
    // expansion never carries an unused import into a crate that denies one.
    let helpers = if mentions(&body_block, &["or_fail", "or_retry"]) {
        quote!(use #script::{OptionExt as _, ResultExt as _};)
    } else {
        TokenStream::new()
    };
    // After the author's own attributes, so an `#[allow(..)]` written above the
    // flow cannot lower what the flow forbids.
    let forbids = lint_forbids();
    let visibility = flow.visibility();
    let header = flow.shape();
    let admission = domain_admission(header.steps())?;
    let outside = outside_admission(header.steps())?;
    let site = site_static(
        Kind::Flow,
        header.line(),
        &format!("flow {}", header.signature()),
    );
    Ok(quote! {
        #attributes
        #forbids
        #doc
        #visibility async fn #name(#inputs) -> ::core::result::Result<#output, #script::FlowError> {
            #helpers
            let scope = &scope.enter_at(#name_text, #site)?;
            let outcome: ::core::result::Result<#output, #script::FlowError> = async {
                #admission
                #outside
                #body_block
            }.await;
            scope.settle(&outcome);
            outcome.map_err(|error| error.located(scope))
        }
    })
}

/// A block's statements, with its last expression set apart as its value when
/// the block wants one.
fn body(block: &Block) -> Result<Body> {
    let mut statements = TokenStream::new();
    let mut tail = None;
    let mut exit = None;
    let last = block.statements().len().saturating_sub(1);
    let value_at = block.tail().map(|_| last);
    for (index, line) in block.statements().iter().enumerate() {
        if index == last
            && let Some(answer) = exit_value(line)?
        {
            exit = Some(answer);
            continue;
        }
        let tokens = statement(line)?;
        if Some(index) == value_at {
            tail = Some((tokens, line.is_fallible()));
        } else if line.is_fallible() {
            statements.extend(quote!(#tokens?;));
        } else if line.is_expression() {
            statements.extend(quote!(#tokens;));
        } else {
            statements.extend(tokens);
        }
    }
    Ok(Body {
        statements,
        tail,
        diverges: block.diverges(),
        exit,
    })
}

/// The flow result a `give back` or `fail` line answers, without the
/// `return` that leaves with it; `None` for every other line.
fn exit_value(line: &Statement) -> Result<Option<TokenStream>> {
    match *line {
        Statement::GiveBack(ref value) => {
            let value = code(value)?;
            Ok(Some(quote!(::core::result::Result::Ok(#value))))
        }
        Statement::Fail {
            transient,
            ref reason,
        } => {
            let script = runtime();
            let constructor = if transient {
                quote!(transient)
            } else {
                quote!(failed)
            };
            let reason = code(reason)?;
            Ok(Some(
                quote!(::core::result::Result::Err(#script::FlowError::#constructor(#reason))),
            ))
        }
        _ => Ok(None),
    }
}

/// One line, or one `if` chain.
fn statement(line: &Statement) -> Result<TokenStream> {
    if let Some(answer) = exit_value(line)? {
        return Ok(quote!(return #answer;));
    }
    match *line {
        Statement::Rust(ref rust) => code(rust),
        Statement::Let(ref rust) => {
            let rust = code(rust)?;
            Ok(quote!(#rust;))
        }
        Statement::Run(ref run) => call(run),
        Statement::Bind {
            ref pattern,
            ref block,
        } => {
            let value = construct(block)?;
            Ok(quote!(let #pattern = #value?;))
        }
        Statement::Construct(ref block) => construct(block),
        Statement::Compensate(ref undo) => vocab::compensate(undo),
        Statement::Together(ref branches) => together(branches),
        Statement::For(ref each) => for_loop(each),
        Statement::If(ref chain) => if_chain(chain),
        // `exit_value` answered both above.
        Statement::GiveBack(_) | Statement::Fail { .. } => Ok(TokenStream::new()),
    }
}

/// A block that yields a value: `each`, `within`, `retry`, `step`, `graph`,
/// `saga`, `whenever`, `justify`, `remember`, `admit`, `ready`, `await approval`, `serve`.
fn construct(block: &Construct) -> Result<TokenStream> {
    match *block {
        Construct::Each(ref fan) => each(fan),
        Construct::Within(ref deadline) => within(deadline),
        Construct::Retry(ref attempts) => retry(attempts),
        Construct::Step(ref named) => step(named),
        Construct::Graph(ref declared) => vocab::graph(declared),
        Construct::Saga(ref named) => vocab::saga(named),
        Construct::Whenever(ref feed) => vocab::whenever(feed),
        Construct::Justify(ref block) => vocab::justify(block),
        Construct::Remember(ref named) => vocab::remember(named),
        Construct::Admit(ref admitted) => vocab::admit(admitted),
        Construct::Ready(ref waited) => vocab::ready(waited),
        Construct::Approval(ref parked) => vocab::approval(parked),
        Construct::Serve(ref served) => serve::serve(served),
    }
}

/// `each <pattern> in <items>:`.
fn each(fan: &Each) -> Result<TokenStream> {
    let script = runtime();
    let limit = match fan.bound() {
        None => quote!(::core::option::Option::None),
        Some(written) => {
            let limit = bound(written)?;
            quote!(::core::option::Option::Some(#script::at_most(#limit)?))
        }
    };
    let label = fan.label();
    let items = code(fan.items())?;
    let pattern = fan.pattern();
    let inner = body(fan.body())?.into_ok_block();
    let site = site_of(fan.site());
    Ok(quote! {
        #script::each_at(scope, #label, #site, #limit, #items, async |scope: #script::Scope, #pattern| {
            let scope = &scope;
            let outcome: ::core::result::Result<_, #script::FlowError> = async #inner.await;
            scope.settle(&outcome);
            outcome
        }).await
    })
}

/// `within <duration>:`.
fn within(deadline: &Within) -> Result<TokenStream> {
    let script = runtime();
    let label = deadline.label();
    let limit = duration(deadline.limit())?;
    let inner = body(deadline.body())?.into_ok_block();
    let site = site_of(deadline.site());
    Ok(quote! {
        #script::within_at(scope, #label, #site, #limit, async #inner).await
    })
}

/// `retry up to <N> times[, waiting <duration>]:`.
fn retry(attempts: &Retry) -> Result<TokenStream> {
    let script = runtime();
    let label = attempts.label();
    let count = bound(attempts.attempts())?;
    let waiting = match attempts.waiting() {
        None => quote!(::core::time::Duration::from_millis(100)),
        Some(pause) => duration(pause)?,
    };
    let inner = body(attempts.body())?.into_ok_block();
    let site = site_of(attempts.site());
    Ok(quote! {
        #script::retry_at(scope, #label, #site, #script::attempts(#count)?, #waiting, async |scope: #script::Scope, attempt: u32| {
            let scope = &scope;
            let _ = attempt;
            #inner
        }).await
    })
}

/// `step <name>:`.
fn step(named: &Step) -> Result<TokenStream> {
    let script = runtime();
    let label = named.label();
    let inner = body(named.body())?.into_ok_block();
    let site = site_of(named.site());
    Ok(quote! {
        {
            let scope = &scope.enter_at(#label, #site)?;
            let outcome: ::core::result::Result<_, #script::FlowError> = async {
                scope.justified(0).await?;
                #inner
            }.await;
            scope.settle(&outcome);
            outcome.map_err(|error| error.located(scope))
        }
    })
}

/// `together:` and its branches, run concurrently on this task.
fn together(branches: &Together) -> Result<TokenStream> {
    let mut patterns = Vec::new();
    let mut futures = Vec::new();
    for branch in branches.branches() {
        let pattern = match branch.pattern() {
            Some(bound) => bound.clone(),
            None => quote!(_),
        };
        patterns.push(pattern);
        let value = match *branch.value() {
            BranchValue::Construct(ref block) => construct(block)?,
            BranchValue::Run(ref run) => call(run)?,
            BranchValue::Rust(ref rust) => ok(&code(rust)?),
        };
        futures.push(quote!(async { #value }));
    }
    Ok(quote!(let (#(#patterns,)*) = ::lgwks_bot::try_join!(#(#futures),*)?;))
}

/// `for <pattern> in <items>:`, sequential, one scope per iteration.
fn for_loop(each: &For) -> Result<TokenStream> {
    let label = each.label();
    let items = code(each.items())?;
    let pattern = each.pattern();
    let script = runtime();
    let site = site_of(each.site());
    let iteration = body(each.body())?;
    // An iteration that reaches its end settles as done. One whose last line
    // leaves the flow has no end to reach, and a settle after it would be code
    // the compiler reports as unreachable; it stays unsettled, and the step
    // that the exit settles says why.
    let settled = if iteration.diverges {
        TokenStream::new()
    } else {
        quote!(scope.settle::<()>(&::core::result::Result::<(), #script::FlowError>::Ok(()));)
    };
    let inner = iteration.into_block();
    Ok(quote! {
        for (lgwks_script_index, #pattern) in ::core::iter::IntoIterator::into_iter(#items).enumerate() {
            let scope = &scope.item_at(#label, lgwks_script_index, #site)?;
            #inner
            #settled
        }
    })
}

/// `if cond:` with its `else if cond:` and `else:` branches.
fn if_chain(chain: &IfChain) -> Result<TokenStream> {
    let mut tokens = TokenStream::new();
    for (position, branch) in chain.branches().iter().enumerate() {
        let inner = body(branch.body())?.into_block();
        let keyword = match (position, branch.condition()) {
            (_, None) => quote!(else),
            (0, Some(condition)) => {
                let condition = code(condition)?;
                quote!(if #condition)
            }
            (_, Some(condition)) => {
                let condition = code(condition)?;
                quote!(else if #condition)
            }
        };
        tokens.extend(quote!(#keyword #inner));
    }
    Ok(tokens)
}

/// A line of Rust with each `run` call written as an awaited call whose
/// failure propagates.
fn code(rust: &Code) -> Result<TokenStream> {
    let mut tokens = TokenStream::new();
    for fragment in rust.fragments() {
        match *fragment {
            Fragment::Token(ref token) => tokens.extend([token.clone()]),
            Fragment::Group {
                delimiter,
                span,
                ref inner,
            } => {
                let mut rebuilt = Group::new(delimiter, code(inner)?);
                rebuilt.set_span(span);
                tokens.extend([TokenTree::Group(rebuilt)]);
            }
            Fragment::Run(ref run) => {
                let run = call(run)?;
                tokens.extend(quote!((#run?)));
            }
            Fragment::Domain(ref used) => {
                let used = domain(used)?;
                tokens.extend(quote!((#used?)));
            }
            Fragment::Outside(ref used) => {
                let used = outside(used)?;
                tokens.extend(quote!((#used?)));
            }
        }
    }
    Ok(tokens)
}

/// A `run` call, awaited, as a flow `Result`.
fn call(run: &Call) -> Result<TokenStream> {
    let path = run.path();
    let arguments = if run.arguments().is_empty() {
        TokenStream::new()
    } else {
        let arguments = code(run.arguments())?;
        quote!(, #arguments)
    };
    if run.label() == run.callee() {
        // The callee enters its own name, from its own header line.
        return Ok(quote!(#path(scope #arguments).await));
    }
    let label = run.label();
    let site = site_of(run.site());
    Ok(quote!({
        let called = &scope.enter_at(#label, #site)?;
        let outcome = #path(called #arguments).await;
        called.settle(&outcome);
        outcome
    }))
}

/// `observe` or `act`, awaited, as a flow `Result`.
fn domain(used: &Domain) -> Result<TokenStream> {
    let script = runtime();
    let id = used.id();
    let target = code(used.target())?;
    let target = quote!(::core::convert::AsRef::<str>::as_ref(&(#target)));
    let invoked = match (used.kind(), used.value()) {
        (Kind::Observe, None) => quote!(#script::observe(called, #id, #target)),
        (Kind::Act, Some(value)) => {
            let value = code(value)?;
            quote!(#script::act(called, #id, #target, #value))
        }
        _ => {
            let refusal = newer_word("domain call");
            tracing::debug!(error = %refusal, "domain: returning an error to the caller");
            return Err(refusal);
        }
    };
    let label = used.label();
    let site = site_of(used.site());
    Ok(quote!({
        let called = &scope.enter_at(#label, #site)?;
        let outcome = #invoked.await;
        called.settle(&outcome);
        outcome
    }))
}

/// A `fetch`, `ask` or `send`, awaited, as a flow `Result`.
fn outside(used: &Outside) -> Result<TokenStream> {
    let script = runtime();
    let site = site_of(used.site());
    let label = used.label();
    let target = code(used.target())?;
    let invoked = match used.kind() {
        Kind::Fetch => {
            let options = match used.options() {
                Some(options) => code(options)?,
                None => quote!(#script::FetchOptions::default()),
            };
            quote!(#script::fetch_at(scope, #label, #site, ::core::convert::AsRef::<str>::as_ref(&(#target)), &#options))
        }
        Kind::Ask => {
            let Some(request) = used.options() else {
                let refusal = newer_word("outside call");
                tracing::debug!(error = %refusal, "outside: an ask names no request");
                return Err(refusal);
            };
            let request = code(request)?;
            quote!(#script::ask_at(scope, #label, #site, #target, &#request))
        }
        Kind::Send => {
            let Some(payload) = used.options() else {
                let refusal = newer_word("outside call");
                tracing::debug!(error = %refusal, "outside: a send names no payload");
                return Err(refusal);
            };
            let payload = code(payload)?;
            quote!(#script::send_at(scope, #label, #site, #target, &(#payload)))
        }
        _ => {
            let refusal = newer_word("outside call");
            tracing::debug!(error = %refusal, "outside: returning an error to the caller");
            return Err(refusal);
        }
    };
    Ok(quote!((#invoked).await))
}

/// The admission a flow runs before its first step when it names any domain.
fn domain_admission(steps: &[StepShape]) -> Result<TokenStream> {
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
fn outside_admission(steps: &[StepShape]) -> Result<TokenStream> {
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

/// A `&'static Site` for `site`.
fn site_of(site: &Site) -> TokenStream {
    site_static(site.kind(), site.line(), site.text())
}

/// A `&'static Site` for the line `text`, the word `kind`, on source line `line`.
fn site_static(kind: Kind, line: usize, text: &str) -> TokenStream {
    let script = runtime();
    let kind = Ident::new(&format!("{kind:?}"), Span::call_site());
    let line = line_literal(line);
    quote!({
        static SITE: #script::Site = #script::Site::new(#script::StepKind::#kind, #line, #text);
        &SITE
    })
}

/// A bound as the runtime's argument.
fn bound(written: &Bound) -> Result<TokenStream> {
    match *written {
        Bound::Count(count) => Ok(Literal::u64_unsuffixed(count).into_token_stream()),
        Bound::Expr(ref expr) => Ok(expr.clone()),
    }
}

/// A duration as a `core::time::Duration` expression.
fn duration(written: &Duration) -> Result<TokenStream> {
    match *written {
        Duration::Nanos(nanos) => {
            let nanos = Literal::u64_unsuffixed(nanos);
            Ok(quote!(::core::time::Duration::from_nanos(#nanos)))
        }
        Duration::Expr(ref expr) => Ok(expr.clone()),
    }
}

/// The runtime map's line, a `u32` literal saturating at `u32::MAX`.
fn line_literal(line: usize) -> Literal {
    match u32::try_from(line) {
        Ok(number) => Literal::u32_unsuffixed(number),
        Err(_more_lines_than_a_u32_holds) => Literal::u32_unsuffixed(u32::MAX),
    }
}

/// A flow's `FlowShape::new(..)` entry in the map.
fn flow_shape(shape: &FlowShape) -> TokenStream {
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

/// Whether any identifier in `stream`, at any depth, is one of `words`.
fn mentions(stream: &TokenStream, words: &[&str]) -> bool {
    stream.clone().into_iter().any(|token| match token {
        TokenTree::Ident(ref word) => words.iter().any(|wanted| word == wanted),
        TokenTree::Group(ref inner) => mentions(&inner.stream(), words),
        TokenTree::Punct(_) | TokenTree::Literal(_) => false,
    })
}
