//! Token stream cursor and error types for `#[derive(Error)]` (#842).

use proc_macro2::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};

/// A typed refusal emitted when deriving `Error`.
#[derive(Debug, Clone)]
#[non_exhaustive]
#[doc(hidden)]
pub struct ErrorDeriveError {
    /// Where the error occurred.
    pub(super) span: Span,
    /// The diagnostic refusal message.
    pub(super) message: String,
}

impl ErrorDeriveError {
    /// Create a new refusal at `span` with `message`.
    #[must_use]
    #[doc(hidden)]
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }

    /// The span where the error occurred.
    #[must_use]
    #[doc(hidden)]
    pub fn span(&self) -> Span {
        self.span
    }

    /// Diagnostic text explaining why macro expansion failed.
    #[must_use]
    #[doc(hidden)]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Convert the refusal into a `::core::compile_error!{ ... }` token stream.
    #[must_use]
    #[doc(hidden)]
    pub fn to_compile_error(&self) -> TokenStream {
        let mut msg_lit = Literal::string(&self.message);
        msg_lit.set_span(self.span);
        let mut braces = Group::new(
            Delimiter::Brace,
            TokenStream::from(TokenTree::Literal(msg_lit)),
        );
        braces.set_span(self.span);
        let path = [
            TokenTree::Punct(Punct::new(':', Spacing::Joint)),
            TokenTree::Punct(Punct::new(':', Spacing::Alone)),
            TokenTree::Ident(Ident::new("core", self.span)),
            TokenTree::Punct(Punct::new(':', Spacing::Joint)),
            TokenTree::Punct(Punct::new(':', Spacing::Alone)),
            TokenTree::Ident(Ident::new("compile_error", self.span)),
            TokenTree::Punct(Punct::new('!', Spacing::Alone)),
        ];
        path.into_iter()
            .map(|mut token| {
                token.set_span(self.span);
                token
            })
            .chain([TokenTree::Group(braces)])
            .collect()
    }
}

impl core::fmt::Display for ErrorDeriveError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl core::error::Error for ErrorDeriveError {}

/// Constructs a refusal error.
pub(super) fn refuse<T>(span: Span, msg: impl Into<String>) -> Result<T, ErrorDeriveError> {
    Err(ErrorDeriveError::new(span, msg))
}

/// Token stream cursor for zero-syn parsing.
#[derive(Clone)]
pub(super) struct TokenCursor {
    pub(super) tokens: Vec<TokenTree>,
    pub(super) pos: usize,
}

impl TokenCursor {
    pub(super) fn new(stream: TokenStream) -> Self {
        Self {
            tokens: stream.into_iter().collect(),
            pos: 0,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    pub(super) fn peek(&self) -> Option<TokenTree> {
        self.tokens.get(self.pos).cloned()
    }

    pub(super) fn next(&mut self) -> Option<TokenTree> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos = self.pos.saturating_add(1);
        }
        tok
    }

    /// Current span, defaulting to call site if at end of stream.
    pub(super) fn current_span(&self) -> Span {
        if let Some(tok_tree) = self.peek() {
            tok_tree.span()
        } else {
            Span::call_site()
        }
    }

    /// Skips any optional `pub` or `pub(...)` qualifier.
    pub(super) fn skip_visibility(&mut self) {
        if let Some(TokenTree::Ident(ref ident_tok)) = self.peek()
            && ident_tok == "pub"
        {
            let _ = self.next();
            if let Some(TokenTree::Group(ref grp)) = self.peek()
                && grp.delimiter() == Delimiter::Parenthesis
            {
                let _ = self.next();
            }
        }
    }
}
