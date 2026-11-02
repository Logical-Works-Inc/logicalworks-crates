//! The `serve` word and its `route` lines: the tenant HTTP edge as script.
//!
//! A `serve` names its port (`from env VAR` or an expression) and its
//! in-flight ceiling (`at most (limit) runs in flight`, required), with every
//! line under it a `route METHOD /path (body: Type) -> Output:`.

use lgwks_deps::proc_macro2::TokenTree;

use super::super::lexicon::{self, Kind};
use super::super::lines::{
    Line, Node as LineNode, after, group, ident, is_ident, is_parens, is_punct, text,
};
use super::super::refuse;
use super::super::tree::{
    Block, Code, Construct, Route, RouteMethod, Serve, ServePort, Site, StepShape,
};
use super::super::{Refusal, Result};
use super::words::{bound, shape};
use super::{Labels, block};

/// The most runs one `serve` holds in flight at once.
const MAX_RUNS_IN_FLIGHT: u64 = 65_536;

/// Refused when a `serve` names no in-flight ceiling.
const NO_CEILING: &str = "a `serve` with no in-flight ceiling is refused: write `serve on port from env PORT, at most (limit) runs in flight:`";

/// A line under `serve:` must read as a route.
const ROUTE_FORM: &str =
    "a line under `serve:` reads `route POST /drafts (body: DraftRequest) -> Draft:`";

/// Refused when a `route` names no typed body.
const NO_BODY: &str =
    "a `route` with no typed body is refused: write `route POST /path (body: Type) -> Output:`";

/// The tokens as code: every token passed through as written.
///
/// `rewrite` is the richer door (it reads `run`, `observe` and `act` calls
/// out of the line); a port, a body type and an output type are plain Rust,
/// so they share this one constructor rather than repeating it.
fn code_of(tokens: &[TokenTree]) -> Code {
    Code {
        fragments: tokens
            .iter()
            .cloned()
            .map(crate::script::tree::Fragment::Token)
            .collect(),
    }
}

/// Refuse at `line` with `message`, logged as every refusal is.
fn refuse_at<T>(line: &Line, message: impl Into<String>, at: &str) -> Result<T> {
    let refusal = Err(Refusal::new(line.span, message));
    tracing::debug!(error = ?refusal.as_ref().err(), word = at, "serve: returning an error to the caller");
    refusal
}

/// The body of a `route`: a fresh scope for labels, with the last line as
/// the value.
fn route_body(children: &[LineNode]) -> Result<(Block, Vec<StepShape>)> {
    let mut labels = Labels::default();
    block(
        children,
        &mut labels,
        super::Place {
            nested: false,
            wants_value: true,
            saga: false,
        },
    )
}

/// `serve on port ..., at most N runs in flight:` and its `route`s.
///
/// The port reads `from env VAR` or an expression before `,`; the ceiling
/// reads `at most (limit) runs in flight` and is required, so a `serve`
/// with no in-flight ceiling does not compile. Every line under it must be a
/// `route`.
pub(super) fn serve(
    line: &Line,
    children: &[LineNode],
    labels: &mut Labels,
) -> Result<(Construct, StepShape)> {
    let rest = after(&line.tokens, 1);
    let tail: &[TokenTree] = match *rest {
        [ref on, ref port, ref tail @ ..]
            if is_ident(on, "on") && is_ident(port, "port") && !tail.is_empty() =>
        {
            tail
        }
        _ => {
            return refuse_at(
                line,
                "a `serve` names its port: `serve on port from env PORT, at most (limit) runs in flight:`",
                "serve",
            );
        }
    };
    let (port, after_port) = read_port(line, tail)?;
    let limit_tokens = read_ceiling(line, after_port)?;
    let limit = bound(limit_tokens, line, "at most", MAX_RUNS_IN_FLIGHT)?;
    if children.is_empty() {
        return refuse_at(
            line,
            "a `serve` runs at least one `route ...:` written under it",
            "serve",
        );
    }
    let mut routes = Vec::with_capacity(children.len());
    let mut shapes = Vec::with_capacity(children.len());
    for child in children {
        let (one, map) = route_node(child, labels)?;
        routes.push(one);
        shapes.push(map);
    }
    let label = labels.next("serve");
    let subject = match port {
        ServePort::Env(ref name) => format!("env {name}"),
        ServePort::Expr(_) => String::from("(port)"),
    };
    let map = shape(Kind::Serve, subject, text(&line.tokens), line, shapes);
    let declared = Serve {
        site: Site::of(&map),
        label,
        port,
        bound: limit,
        routes,
    };
    Ok((Construct::Serve(declared), map))
}

/// The port half of a `serve` header: `from env VAR` or a port expression.
///
/// Returns the port and the tokens after it (starting at `,`).
fn read_port<'line>(
    line: &Line,
    tokens: &'line [TokenTree],
) -> Result<(ServePort, &'line [TokenTree])> {
    match *tokens {
        [ref from, ref env, ref var, ref rest @ ..]
            if is_ident(from, "from") && is_ident(env, "env") =>
        {
            let Some(name) = ident(var) else {
                return refuse_at(
                    line,
                    "`serve on port` reads `from env VAR`, where `VAR` is one word",
                    "serve",
                );
            };
            Ok((ServePort::Env(name.to_string()), rest))
        }
        _ => {
            let Some(at_comma) = tokens.iter().position(|token| is_punct(token, ',')) else {
                return refuse_at(line, NO_CEILING, "serve");
            };
            let (port_tokens, rest) = tokens.split_at(at_comma);
            if port_tokens.is_empty() {
                return refuse_at(
                    line,
                    "`serve on port` reads `from env VAR` or a port expression before `,`",
                    "serve",
                );
            }
            let code = code_of(port_tokens);
            Ok((ServePort::Expr(code), rest))
        }
    }
}

/// The ceiling half of a `serve` header: `, at most (limit) runs in flight`.
///
/// Returns the bound tokens between `most` and `runs`.
fn read_ceiling<'line>(line: &Line, tokens: &'line [TokenTree]) -> Result<&'line [TokenTree]> {
    let [ref comma, ref at, ref most, ref tail @ ..] = *tokens else {
        return refuse_at(line, NO_CEILING, "serve");
    };
    if !(is_punct(comma, ',') && is_ident(at, "at") && is_ident(most, "most")) {
        return refuse_at(
            line,
            "`serve` reads `serve on port from env PORT, at most (limit) runs in flight:`",
            "serve",
        );
    }
    if tail.is_empty() {
        return refuse_at(line, NO_CEILING, "serve");
    }
    let Some(at_runs) = tail.iter().position(|token| is_ident(token, "runs")) else {
        return refuse_at(
            line,
            "`serve` reads `..., at most (limit) runs in flight:`",
            "serve",
        );
    };
    let (limit_tokens, after_limit) = tail.split_at(at_runs);
    if limit_tokens.is_empty() {
        return refuse_at(line, NO_CEILING, "serve");
    }
    match *after_limit {
        [ref runs, ref inside, ref flight]
            if is_ident(runs, "runs") && is_ident(inside, "in") && is_ident(flight, "flight") =>
        {
            Ok(limit_tokens)
        }
        _ => refuse_at(
            line,
            "`serve` reads `..., at most (limit) runs in flight:`",
            "serve",
        ),
    }
}

/// One `route METHOD /path (body: Type) -> Output:` under a `serve`.
fn route_node(child: &LineNode, labels: &mut Labels) -> Result<(Route, StepShape)> {
    let line = &child.line;
    refuse::check(&line.tokens)?;
    if lexicon::kind_of(line) != Some(Kind::Route) || !line.opens_block {
        return refuse_at(line, ROUTE_FORM, "route");
    }
    let rest = after(&line.tokens, 1);
    let Some((method, tail)) = rest
        .split_first()
        .and_then(|(first, rest)| ident(first).map(|name| (name.to_string(), rest)))
    else {
        return refuse_at(line, ROUTE_FORM, "route");
    };
    let method = match method.as_str() {
        "GET" => RouteMethod::Get,
        "POST" => RouteMethod::Post,
        _ => {
            return refuse_at(
                line,
                "a `route` answers `GET` or `POST`, naming the method first",
                "route",
            );
        }
    };
    let Some(at_body) = tail.iter().position(is_parens) else {
        return refuse_at(line, NO_BODY, "route");
    };
    let (before, at_group) = tail.split_at(at_body);
    let Some((group_token, after_group)) = at_group.split_first() else {
        return refuse_at(line, ROUTE_FORM, "route");
    };
    let Some(inner) = group(group_token) else {
        return refuse_at(line, ROUTE_FORM, "route");
    };
    let inner_tokens: Vec<TokenTree> = inner.stream().into_iter().collect();
    let (body_ty, output_ty) = match *inner_tokens.as_slice() {
        [ref body_word, ref colon, ref ty @ ..]
            if is_ident(body_word, "body") && is_punct(colon, ':') && !ty.is_empty() =>
        {
            let body_code = code_of(ty);
            let output = read_route_output(line, after_group)?;
            (body_code, output)
        }
        _ => {
            return refuse_at(line, NO_BODY, "route");
        }
    };
    if before.is_empty() {
        return refuse_at(line, ROUTE_FORM, "route");
    }
    let path = text(before);
    if !path.starts_with('/') {
        return refuse_at(
            line,
            "a `route` path is rooted at `/`, as the edge serves it",
            "route",
        );
    }
    let route_label = labels.next(&format!("route:{}", method.as_str().to_lowercase()));
    let (body, inner) = route_body(&child.children)?;
    let map = shape(
        Kind::Route,
        format!("{} {path}", method.as_str()),
        text(&line.tokens),
        line,
        inner,
    );
    let declared = Route {
        site: Site::of(&map),
        label: route_label,
        method,
        path,
        body_ty,
        output_ty,
        body,
    };
    Ok((declared, map))
}

/// The `-> Output` half of a `route` header.
fn read_route_output(line: &Line, tokens: &[TokenTree]) -> Result<Code> {
    match *tokens {
        [ref dash, ref arrow, ref ty @ ..]
            if is_punct(dash, '-') && is_punct(arrow, '>') && !ty.is_empty() =>
        {
            Ok(code_of(ty))
        }
        _ => refuse_at(
            line,
            "a `route` names its output: `route POST /path (body: Type) -> Output:`",
            "route",
        ),
    }
}
