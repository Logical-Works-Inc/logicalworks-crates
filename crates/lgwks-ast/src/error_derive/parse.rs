//! Attribute and token parsing for `#[derive(Error)]` (#842).

use proc_macro2::{Delimiter, Ident, Span, TokenStream, TokenTree};
use quote::quote;

use super::cursor::{ErrorDeriveError, TokenCursor, refuse};
use super::format::{FormatAttr, unescape_string_literal};

/// An attribute parsed from source tokens.
pub(super) enum Attribute {
    /// `#[error("...")]` with optional extra arguments.
    ErrorFormat(FormatAttr),
    /// `#[error(transparent)]`.
    Transparent(Span),
    /// `#[from]`.
    From,
    /// `#[source]`.
    Source,
    /// An ignored attribute (such as `#[doc]`, `#[derive]`, `#[allow]`).
    Other,
}

impl Attribute {
    pub(super) fn error_format(&self) -> Option<&FormatAttr> {
        match *self {
            Self::ErrorFormat(ref fmt) => Some(fmt),
            _ => None,
        }
    }

    pub(super) fn transparent_span(&self) -> Option<Span> {
        match *self {
            Self::Transparent(ref sp) => Some(*sp),
            _ => None,
        }
    }
}

/// A parsed field of a struct or variant.
pub(super) struct ParsedField {
    /// Field name if named field, None if tuple field.
    pub(super) name: Option<Ident>,
    /// Whether field has `#[from]`.
    pub(super) is_from: bool,
    /// Whether field has `#[source]` or is named `source`.
    pub(super) is_source: bool,
    /// Type of the field as tokens.
    pub(super) ty: TokenStream,
    /// Span of the field.
    pub(super) span: Span,
}

/// A parsed enum variant.
pub(super) struct ParsedVariant {
    /// Variant name.
    pub(super) name: Ident,
    /// Format string attribute if present.
    pub(super) error_format: Option<FormatAttr>,
    /// Whether marked `#[error(transparent)]`.
    pub(super) is_transparent: bool,
    /// Fields of the variant.
    pub(super) fields: Vec<ParsedField>,
    /// Whether fields are named.
    pub(super) is_named: bool,
}

/// Parse attributes starting at `cursor`.
pub(super) fn parse_attributes(
    cursor: &mut TokenCursor,
) -> Result<Vec<Attribute>, ErrorDeriveError> {
    let mut attrs = Vec::new();
    while let Some(TokenTree::Punct(punct)) = cursor.peek() {
        if punct.as_char() != '#' {
            break;
        }
        let _hash = cursor.next();
        let Some(TokenTree::Group(grp)) = cursor.next() else {
            break;
        };
        if grp.delimiter() != Delimiter::Bracket {
            continue;
        }
        let mut inner_cursor = TokenCursor::new(grp.stream());
        let Some(TokenTree::Ident(ident)) = inner_cursor.next() else {
            continue;
        };
        let ident_str = ident.to_string();
        if ident_str == "error" {
            let Some(TokenTree::Group(paren_group)) = inner_cursor.next() else {
                return refuse(
                    ident.span(),
                    "expected #[error(\"...\")] or #[error(transparent)]",
                );
            };
            if paren_group.delimiter() != Delimiter::Parenthesis {
                return refuse(ident.span(), "expected parenthesis after #[error ...]");
            }
            let mut paren_cursor = TokenCursor::new(paren_group.stream());
            let Some(first_tok) = paren_cursor.next() else {
                return refuse(ident.span(), "#[error(...)] requires arguments");
            };
            match first_tok {
                TokenTree::Ident(ref inner_id) if inner_id == "transparent" => {
                    attrs.push(Attribute::Transparent(ident.span()));
                }
                TokenTree::Literal(lit) => {
                    let lit_str = lit.to_string();
                    let Some(unescaped) = unescape_string_literal(&lit_str) else {
                        return refuse(lit.span(), "expected string literal inside #[error(...)]");
                    };
                    let extra_args = if let Some(TokenTree::Punct(punct)) = paren_cursor.peek() {
                        if punct.as_char() == ',' {
                            let _ = paren_cursor.next();
                            let mut extra_tokens = Vec::new();
                            while let Some(tok) = paren_cursor.next() {
                                extra_tokens.push(tok);
                            }
                            if extra_tokens.is_empty() {
                                None
                            } else {
                                Some(extra_tokens.into_iter().collect::<TokenStream>())
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    attrs.push(Attribute::ErrorFormat(FormatAttr {
                        format: unescaped,
                        literal: lit,
                        extra_args,
                        span: ident.span(),
                    }));
                }
                other => {
                    return refuse(
                        other.span(),
                        "expected string literal or 'transparent' inside #[error(...)]",
                    );
                }
            }
        } else if ident_str == "from" {
            attrs.push(Attribute::From);
        } else if ident_str == "source" {
            attrs.push(Attribute::Source);
        } else {
            attrs.push(Attribute::Other);
        }
    }
    Ok(attrs)
}

/// Parse fields from a delimited stream (parenthesis or braces).
pub(super) fn parse_fields(
    stream: TokenStream,
    is_named: bool,
) -> Result<Vec<ParsedField>, ErrorDeriveError> {
    let mut cursor = TokenCursor::new(stream);
    let mut fields = Vec::new();

    while !cursor.is_empty() {
        let attrs = parse_attributes(&mut cursor)?;
        let mut is_from = false;
        let mut is_source = false;

        for attr in attrs {
            match attr {
                Attribute::From => is_from = true,
                Attribute::Source => is_source = true,
                _ => {}
            }
        }

        cursor.skip_visibility();

        let name = if is_named {
            let Some(TokenTree::Ident(ident_val)) = cursor.next() else {
                break;
            };
            if let Some(TokenTree::Punct(punct)) = cursor.next() {
                if punct.as_char() != ':' {
                    return refuse(punct.span(), "expected ':' after field name");
                }
            } else {
                return refuse(ident_val.span(), "expected ':' after field name");
            }
            Some(ident_val)
        } else {
            None
        };

        let mut span = cursor.current_span();
        if let Some(ref ident_val) = name {
            span = ident_val.span();
        }

        if let Some(ref ident_val) = name
            && ident_val == "source"
        {
            is_source = true;
        }
        if is_from {
            is_source = true;
        }

        let mut ty_tokens = Vec::new();
        while let Some(tok_ref) = cursor.peek() {
            if let TokenTree::Punct(punct) = tok_ref
                && punct.as_char() == ','
            {
                let _ = cursor.next();
                break;
            }
            if let Some(tok) = cursor.next() {
                ty_tokens.push(tok);
            }
        }

        if ty_tokens.is_empty() && name.is_none() {
            break;
        }

        fields.push(ParsedField {
            name,
            is_from,
            is_source,
            ty: ty_tokens.into_iter().collect(),
            span,
        });
    }

    Ok(fields)
}

/// Parse generics from `cursor` if leading token is `<`.
pub(super) fn parse_generics(
    cursor: &mut TokenCursor,
) -> Result<(Option<TokenStream>, Option<TokenStream>), ErrorDeriveError> {
    let Some(TokenTree::Punct(punct)) = cursor.peek() else {
        return Ok((None, None));
    };
    if punct.as_char() != '<' {
        return Ok((None, None));
    }
    let _ = cursor.next();

    let mut depth: usize = 1;
    let mut generics_tokens = Vec::new();

    while let Some(tok) = cursor.next() {
        match tok {
            TokenTree::Punct(ref punct) if punct.as_char() == '<' => {
                depth = depth.saturating_add(1);
                generics_tokens.push(tok);
            }
            TokenTree::Punct(ref punct) if punct.as_char() == '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
                generics_tokens.push(tok);
            }
            _ => generics_tokens.push(tok),
        }
    }

    if depth != 0 {
        return refuse(cursor.current_span(), "unclosed '<' in generic parameters");
    }

    let impl_generics = Some(quote!( < #(#generics_tokens)* > ));

    let mut param_names: Vec<TokenStream> = Vec::new();
    let mut current_param: Vec<TokenTree> = Vec::new();
    let mut paren_depth: usize = 0;

    for tok in generics_tokens {
        match tok {
            TokenTree::Group(grp) => {
                current_param.push(TokenTree::Group(grp));
            }
            TokenTree::Punct(ref punct) if punct.as_char() == '<' => {
                paren_depth = paren_depth.saturating_add(1);
                current_param.push(tok);
            }
            TokenTree::Punct(ref punct) if punct.as_char() == '>' => {
                paren_depth = paren_depth.saturating_sub(1);
                current_param.push(tok);
            }
            TokenTree::Punct(ref punct) if punct.as_char() == ',' && paren_depth == 0 => {
                if let Some(name_stream) = extract_param_name(&current_param) {
                    param_names.push(name_stream);
                }
                current_param.clear();
            }
            _ => current_param.push(tok),
        }
    }
    if !current_param.is_empty()
        && let Some(name_stream) = extract_param_name(&current_param)
    {
        param_names.push(name_stream);
    }

    let ty_generics = if param_names.is_empty() {
        None
    } else {
        Some(quote!( < #(#param_names),* > ))
    };

    Ok((impl_generics, ty_generics))
}

/// Extract parameter identifier or lifetime from a slice of generic parameter tokens.
fn extract_param_name(tokens: &[TokenTree]) -> Option<TokenStream> {
    let first = tokens.first()?;
    match *first {
        TokenTree::Punct(ref punct) if punct.as_char() == '\'' => {
            let second = tokens.get(1)?;
            Some(quote!( #first #second ))
        }
        TokenTree::Ident(ref ident_tok) if ident_tok == "const" => {
            let second = tokens.get(1)?;
            Some(quote!( #second ))
        }
        TokenTree::Ident(_) => Some(quote!( #first )),
        _ => None,
    }
}

/// Parse where clause from `cursor` if present.
pub(super) fn parse_where_clause(cursor: &mut TokenCursor) -> Option<TokenStream> {
    if let Some(TokenTree::Ident(ident_tok)) = cursor.peek()
        && ident_tok == "where"
    {
        let _ = cursor.next();
        let mut where_tokens = Vec::new();
        while let Some(tok_ref) = cursor.peek() {
            match tok_ref {
                TokenTree::Group(grp)
                    if grp.delimiter() == Delimiter::Brace
                        || grp.delimiter() == Delimiter::Parenthesis =>
                {
                    break;
                }
                TokenTree::Punct(punct) if punct.as_char() == ';' => break,
                _ => {
                    if let Some(tok) = cursor.next() {
                        where_tokens.push(tok);
                    }
                }
            }
        }
        return Some(quote!( where #(#where_tokens)* ));
    }
    None
}
