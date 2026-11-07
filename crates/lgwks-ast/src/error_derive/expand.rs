//! Code generation and macro expansion for `#[derive(Error)]` (#842).

use proc_macro2::{Delimiter, Ident, Literal, Span, TokenStream, TokenTree};
use quote::quote;

use super::cursor::{ErrorDeriveError, TokenCursor, refuse};
use super::format::{FormatAttr, rewrite_format_string};
use super::parse::{
    Attribute, ParsedVariant, parse_attributes, parse_fields, parse_generics, parse_where_clause,
};

/// Expand a `#[derive(Error)]` input token stream into implementations of
/// [`core::fmt::Display`], [`core::error::Error`], and [`core::convert::From`].
#[doc(hidden)]
pub fn expand(input: TokenStream) -> Result<TokenStream, ErrorDeriveError> {
    let mut cursor = TokenCursor::new(input);
    let outer_attrs = parse_attributes(&mut cursor)?;
    cursor.skip_visibility();

    let Some(TokenTree::Ident(kind_ident)) = cursor.next() else {
        return refuse(
            cursor.current_span(),
            "expected 'enum' or 'struct' in derive input",
        );
    };

    let kind_str = kind_ident.to_string();
    if kind_str == "union" {
        return refuse(
            kind_ident.span(),
            "unions are not supported by #[derive(Error)]",
        );
    }
    if kind_str != "enum" && kind_str != "struct" {
        return refuse(
            kind_ident.span(),
            "#[derive(Error)] can only be derived on enums or structs",
        );
    }

    let Some(TokenTree::Ident(type_ident)) = cursor.next() else {
        return refuse(
            cursor.current_span(),
            "expected type identifier after enum/struct",
        );
    };

    let (impl_generics, ty_generics) = parse_generics(&mut cursor)?;
    let where_clause = parse_where_clause(&mut cursor);

    let parts = if kind_str == "enum" {
        expand_enum(&mut cursor)?
    } else {
        expand_struct(&type_ident, outer_attrs, &mut cursor)?
    };

    let mut from_impls = Vec::with_capacity(parts.from_pairs.len());
    for (ty, body) in parts.from_pairs {
        from_impls.push(quote! {
            impl #impl_generics ::core::convert::From<#ty> for #type_ident #ty_generics #where_clause {
                fn from(source: #ty) -> Self {
                    #body
                }
            }
        });
    }

    let display_body = parts.display;
    let source_body = parts.source;
    Ok(quote! {
        impl #impl_generics ::core::fmt::Display for #type_ident #ty_generics #where_clause {
            fn fmt(&self, __formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                #display_body
            }
        }

        impl #impl_generics ::core::error::Error for #type_ident #ty_generics #where_clause {
            fn source(&self) -> ::core::option::Option<&(dyn ::core::error::Error + 'static)> {
                #source_body
            }
        }

        #(#from_impls)*
    })
}

fn tuple_field_bindings(count: usize) -> Vec<Ident> {
    (0..count)
        .map(|idx| Ident::new(&format!("__field_{idx}"), Span::call_site()))
        .collect()
}

fn find_error_format(
    attrs: &[Attribute],
    target_kind: &str,
    target_name: &str,
) -> Result<(Option<FormatAttr>, bool), ErrorDeriveError> {
    let mut error_format = None;
    let mut is_transparent = false;
    for attr in attrs {
        if let Some(fmt) = attr.error_format() {
            if error_format.is_some() || is_transparent {
                return refuse(
                    fmt.span,
                    format!("duplicate #[error(...)] attribute on {target_kind} '{target_name}'"),
                );
            }
            error_format = Some(fmt.clone());
        } else if let Some(sp) = attr.transparent_span() {
            if error_format.is_some() || is_transparent {
                return refuse(
                    sp,
                    format!("duplicate #[error(...)] attribute on {target_kind} '{target_name}'"),
                );
            }
            is_transparent = true;
        }
    }
    Ok((error_format, is_transparent))
}

struct ErrorParts {
    display: TokenStream,
    source: TokenStream,
    from_pairs: Vec<(TokenStream, TokenStream)>,
}

/// Expand an enum definition into error components.
fn expand_enum(cursor: &mut TokenCursor) -> Result<ErrorParts, ErrorDeriveError> {
    let Some(TokenTree::Group(body_group)) = cursor.next() else {
        return refuse(
            cursor.current_span(),
            "expected enum body enclosed in braces",
        );
    };
    if body_group.delimiter() != Delimiter::Brace {
        return refuse(body_group.span(), "expected enum body enclosed in braces");
    }

    let mut body_cursor = TokenCursor::new(body_group.stream());
    let mut variants = Vec::new();

    while !body_cursor.is_empty() {
        let attrs = parse_attributes(&mut body_cursor)?;
        let Some(TokenTree::Ident(variant_name)) = body_cursor.next() else {
            break;
        };

        let (error_format, is_transparent) =
            find_error_format(&attrs, "variant", &variant_name.to_string())?;

        let (fields, is_named) = if let Some(TokenTree::Group(grp)) = body_cursor.peek() {
            let grp_delim = grp.delimiter();
            let grp_stream = grp.stream();
            if grp_delim == Delimiter::Brace {
                let _ = body_cursor.next();
                (parse_fields(grp_stream, true)?, true)
            } else if grp_delim == Delimiter::Parenthesis {
                let _ = body_cursor.next();
                (parse_fields(grp_stream, false)?, false)
            } else {
                (Vec::new(), false)
            }
        } else {
            (Vec::new(), false)
        };

        if let Some(TokenTree::Punct(punct)) = body_cursor.peek()
            && punct.as_char() == '='
        {
            let _ = body_cursor.next();
            while let Some(tok_ref) = body_cursor.peek() {
                if let TokenTree::Punct(punct) = tok_ref
                    && punct.as_char() == ','
                {
                    break;
                }
                let _ = body_cursor.next();
            }
        }

        if let Some(TokenTree::Punct(punct)) = body_cursor.peek()
            && punct.as_char() == ','
        {
            let _ = body_cursor.next();
        }

        if error_format.is_none() && !is_transparent {
            return refuse(
                variant_name.span(),
                format!("missing #[error(...)] attribute on variant '{variant_name}'"),
            );
        }

        if is_transparent && fields.len() != 1 {
            return refuse(
                variant_name.span(),
                format!(
                    "#[error(transparent)] requires variant '{variant_name}' to have exactly one field"
                ),
            );
        }

        for field_item in &fields {
            if field_item.is_from && fields.len() != 1 {
                return refuse(
                    field_item.span,
                    "#[from] is only supported on variants with a single field",
                );
            }
        }

        variants.push(ParsedVariant {
            name: variant_name,
            error_format,
            is_transparent,
            fields,
            is_named,
        });
    }

    let mut display_arms = Vec::new();
    let mut source_arms = Vec::new();
    let mut from_pairs = Vec::new();

    for variant in &variants {
        let v_name = &variant.name;

        if variant.is_transparent {
            if variant.is_named {
                let field = variant.fields.first().map(|field_item| &field_item.name);
                display_arms.push(quote! {
                    Self::#v_name { ref #field } => ::core::fmt::Display::fmt(#field, __formatter),
                });
                source_arms.push(quote! {
                    Self::#v_name { ref #field } => ::core::error::Error::source(#field),
                });
            } else {
                display_arms.push(quote! {
                    Self::#v_name(ref __field_0) => ::core::fmt::Display::fmt(__field_0, __formatter),
                });
                source_arms.push(quote! {
                    Self::#v_name(ref __field_0) => ::core::error::Error::source(__field_0),
                });
            }
        } else if let Some(ref fmt) = variant.error_format {
            let write_args = if let Some(ref extra) = fmt.extra_args {
                let lit = &fmt.literal;
                quote!(#lit, #extra)
            } else {
                let field_names: Vec<String> = variant
                    .fields
                    .iter()
                    .filter_map(|field_item| {
                        field_item
                            .name
                            .as_ref()
                            .map(|ident_val| ident_val.to_string())
                    })
                    .collect();
                let rewritten_str = match rewrite_format_string(
                    &fmt.format,
                    !variant.is_named,
                    &field_names,
                    variant.fields.len(),
                ) {
                    Ok(res_str) => res_str,
                    Err(err_msg) => return refuse(fmt.span, err_msg),
                };
                let rewritten_lit = Literal::string(&rewritten_str);
                quote!(#rewritten_lit)
            };

            if variant.fields.is_empty() {
                display_arms.push(quote! {
                    Self::#v_name => ::core::write!(__formatter, #write_args),
                });
            } else if variant.is_named {
                let names: Vec<_> = variant
                    .fields
                    .iter()
                    .filter_map(|field_item| field_item.name.as_ref())
                    .collect();
                display_arms.push(quote! {
                    Self::#v_name { #(ref #names),* } => {
                        #( let _ = &#names; )*
                        ::core::write!(__formatter, #write_args)
                    }
                });
            } else {
                let bindings = tuple_field_bindings(variant.fields.len());
                display_arms.push(quote! {
                    Self::#v_name(#(ref #bindings),*) => {
                        #( let _ = &#bindings; )*
                        ::core::write!(__formatter, #write_args)
                    }
                });
            }

            let source_field = variant
                .fields
                .iter()
                .enumerate()
                .find(|&(_, field_item)| field_item.is_source);
            if let Some((idx, field_item)) = source_field {
                if variant.is_named {
                    let f_name = &field_item.name;
                    source_arms.push(quote! {
                        Self::#v_name { ref #f_name, .. } => ::core::option::Option::Some(#f_name),
                    });
                } else {
                    let bindings = (0..variant.fields.len()).map(|field_idx| {
                        if field_idx == idx {
                            quote!(ref __field_source)
                        } else {
                            quote!(_)
                        }
                    });
                    source_arms.push(quote! {
                        Self::#v_name(#(#bindings),*) => ::core::option::Option::Some(__field_source),
                    });
                }
            } else if variant.fields.is_empty() {
                source_arms.push(quote! {
                    Self::#v_name => ::core::option::Option::None,
                });
            } else if variant.is_named {
                source_arms.push(quote! {
                    Self::#v_name { .. } => ::core::option::Option::None,
                });
            } else {
                source_arms.push(quote! {
                    Self::#v_name(..) => ::core::option::Option::None,
                });
            }
        }

        for field_item in &variant.fields {
            if field_item.is_from {
                let body = if variant.is_named {
                    let f_name = &field_item.name;
                    quote! { Self::#v_name { #f_name: source } }
                } else {
                    quote! { Self::#v_name(source) }
                };
                from_pairs.push((field_item.ty.clone(), body));
            }
        }
    }

    let display_body = quote! {
        match *self {
            #(#display_arms)*
        }
    };
    let source_body = quote! {
        match *self {
            #(#source_arms)*
        }
    };

    Ok(ErrorParts {
        display: display_body,
        source: source_body,
        from_pairs,
    })
}

/// Expand a struct definition into error components.
fn expand_struct(
    ident: &Ident,
    outer_attrs: Vec<Attribute>,
    cursor: &mut TokenCursor,
) -> Result<ErrorParts, ErrorDeriveError> {
    let (error_format, is_transparent) =
        find_error_format(&outer_attrs, "struct", &ident.to_string())?;

    if error_format.is_none() && !is_transparent {
        return refuse(
            ident.span(),
            format!("missing #[error(...)] attribute on struct '{ident}'"),
        );
    }

    let (fields, is_named) = if let Some(TokenTree::Group(grp)) = cursor.next() {
        if grp.delimiter() == Delimiter::Brace {
            (parse_fields(grp.stream(), true)?, true)
        } else if grp.delimiter() == Delimiter::Parenthesis {
            if let Some(TokenTree::Punct(punct)) = cursor.peek()
                && punct.as_char() == ';'
            {
                let _ = cursor.next();
            }
            (parse_fields(grp.stream(), false)?, false)
        } else {
            (Vec::new(), false)
        }
    } else {
        (Vec::new(), false)
    };

    if is_transparent && fields.len() != 1 {
        return refuse(
            ident.span(),
            format!("#[error(transparent)] requires struct '{ident}' to have exactly one field"),
        );
    }

    for field_item in &fields {
        if field_item.is_from && fields.len() != 1 {
            return refuse(
                field_item.span,
                "#[from] is only supported on structs with a single field",
            );
        }
    }

    let display_body = if is_transparent {
        if is_named {
            let field = fields.first().map(|field_item| &field_item.name);
            quote! {
                ::core::fmt::Display::fmt(&self.#field, __formatter)
            }
        } else {
            quote! {
                ::core::fmt::Display::fmt(&self.0, __formatter)
            }
        }
    } else if let Some(ref fmt) = error_format {
        let write_args = if let Some(ref extra) = fmt.extra_args {
            let lit = &fmt.literal;
            quote!(#lit, #extra)
        } else {
            let field_names: Vec<String> = fields
                .iter()
                .filter_map(|field_item| {
                    field_item
                        .name
                        .as_ref()
                        .map(|ident_val| ident_val.to_string())
                })
                .collect();
            let rewritten_str =
                match rewrite_format_string(&fmt.format, !is_named, &field_names, fields.len()) {
                    Ok(res_str) => res_str,
                    Err(err_msg) => return refuse(fmt.span, err_msg),
                };
            let rewritten_lit = Literal::string(&rewritten_str);
            quote!(#rewritten_lit)
        };

        if fields.is_empty() {
            quote! {
                ::core::write!(__formatter, #write_args)
            }
        } else if is_named {
            let names: Vec<_> = fields
                .iter()
                .filter_map(|field_item| field_item.name.as_ref())
                .collect();
            quote! {
                let Self { #(ref #names),* } = *self;
                #( let _ = &#names; )*
                ::core::write!(__formatter, #write_args)
            }
        } else {
            let bindings = tuple_field_bindings(fields.len());
            quote! {
                let Self(#(ref #bindings),*) = *self;
                #( let _ = &#bindings; )*
                ::core::write!(__formatter, #write_args)
            }
        }
    } else {
        quote! {
            ::core::write!(__formatter, "")
        }
    };

    let source_body = if is_transparent {
        if is_named {
            let field = fields.first().map(|field_item| &field_item.name);
            quote! {
                ::core::error::Error::source(&self.#field)
            }
        } else {
            quote! {
                ::core::error::Error::source(&self.0)
            }
        }
    } else if let Some((idx, field_item)) = fields
        .iter()
        .enumerate()
        .find(|&(_, field_item)| field_item.is_source)
    {
        if is_named {
            let f_name = &field_item.name;
            quote! {
                ::core::option::Option::Some(&self.#f_name)
            }
        } else {
            let index = Literal::usize_unsuffixed(idx);
            quote! {
                ::core::option::Option::Some(&self.#index)
            }
        }
    } else {
        quote! {
            ::core::option::Option::None
        }
    };

    let mut from_pairs = Vec::new();
    for field_item in &fields {
        if field_item.is_from {
            let body = if is_named {
                let f_name = &field_item.name;
                quote! { Self { #f_name: source } }
            } else {
                quote! { Self(source) }
            };
            from_pairs.push((field_item.ty.clone(), body));
        }
    }

    Ok(ErrorParts {
        display: display_body,
        source: source_body,
        from_pairs,
    })
}
