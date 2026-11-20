//! Code generation and macro expansion for `#[derive(Error)]` (#842).

use proc_macro2::{Delimiter, Ident, Literal, Span, TokenStream, TokenTree};
use quote::quote;

use super::cursor::{ErrorDeriveError, TokenCursor, refuse};
use super::format::{FormatAttr, rewrite_format_string};
use super::parse::{
    Attribute, ParsedField, ParsedVariant, parse_attributes, parse_fields, parse_generics,
    parse_where_clause,
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
        expand_struct(&type_ident, &outer_attrs, &mut cursor)?
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

/// The arguments of the `write!` that renders one variant or struct: the
/// attribute's own literal and extra arguments, or the format string rewritten
/// onto the field bindings.
fn write_arguments(
    fmt: &FormatAttr,
    is_named: bool,
    fields: &[ParsedField],
) -> Result<TokenStream, ErrorDeriveError> {
    if let Some(ref extra) = fmt.extra_args {
        let lit = &fmt.literal;
        return Ok(quote!(#lit, #extra));
    }
    let field_names: Vec<String> = fields
        .iter()
        .filter_map(|field_item| {
            field_item
                .name
                .as_ref()
                .map(|ident_val| ident_val.to_string())
        })
        .collect();
    match rewrite_format_string(&fmt.format, !is_named, &field_names, fields.len()) {
        Ok(rewritten_str) => {
            let rewritten_lit = Literal::string(&rewritten_str);
            Ok(quote!(#rewritten_lit))
        }
        Err(error) => refuse(fmt.span, error.to_string()),
    }
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
    let variants = parse_variants(&mut body_cursor)?;
    enum_parts(&variants)
}

/// Read every variant of an enum body, refusing the first malformed one.
fn parse_variants(body_cursor: &mut TokenCursor) -> Result<Vec<ParsedVariant>, ErrorDeriveError> {
    let mut variants = Vec::new();

    while !body_cursor.is_empty() {
        let attrs = parse_attributes(body_cursor)?;
        let Some(TokenTree::Ident(variant_name)) = body_cursor.next() else {
            break;
        };

        let (error_format, is_transparent) =
            find_error_format(&attrs, "variant", &variant_name.to_string())?;
        let (fields, is_named) = parse_variant_fields(body_cursor)?;
        skip_discriminant_and_comma(body_cursor);
        check_variant(
            &variant_name,
            error_format.as_ref(),
            is_transparent,
            &fields,
        )?;

        variants.push(ParsedVariant {
            name: variant_name,
            error_format,
            is_transparent,
            fields,
            is_named,
        });
    }
    Ok(variants)
}

/// The fields of the variant at `cursor`: braces name them, parentheses
/// number them, and neither means a unit variant.
fn parse_variant_fields(
    cursor: &mut TokenCursor,
) -> Result<(Vec<ParsedField>, bool), ErrorDeriveError> {
    let Some(TokenTree::Group(grp)) = cursor.peek() else {
        return Ok((Vec::new(), false));
    };
    match grp.delimiter() {
        Delimiter::Brace => {
            cursor.advance();
            Ok((parse_fields(grp.stream(), true)?, true))
        }
        Delimiter::Parenthesis => {
            cursor.advance();
            Ok((parse_fields(grp.stream(), false)?, false))
        }
        Delimiter::Bracket | Delimiter::None => Ok((Vec::new(), false)),
    }
}

/// Step past an explicit discriminant (`= 3`) and the comma after the variant.
fn skip_discriminant_and_comma(cursor: &mut TokenCursor) {
    if let Some(TokenTree::Punct(punct)) = cursor.peek()
        && punct.as_char() == '='
    {
        cursor.advance();
        while let Some(tok_ref) = cursor.peek() {
            if let TokenTree::Punct(punct) = tok_ref
                && punct.as_char() == ','
            {
                break;
            }
            cursor.advance();
        }
    }

    if let Some(TokenTree::Punct(punct)) = cursor.peek()
        && punct.as_char() == ','
    {
        cursor.advance();
    }
}

/// Refuse a variant with no `#[error]`, a `transparent` one without exactly one
/// field, or a `#[from]` beside another field.
fn check_variant(
    name: &Ident,
    error_format: Option<&FormatAttr>,
    is_transparent: bool,
    fields: &[ParsedField],
) -> Result<(), ErrorDeriveError> {
    if error_format.is_none() && !is_transparent {
        return refuse(
            name.span(),
            format!("missing #[error(...)] attribute on variant '{name}'"),
        );
    }

    if is_transparent && fields.len() != 1 {
        return refuse(
            name.span(),
            format!("#[error(transparent)] requires variant '{name}' to have exactly one field"),
        );
    }

    for field_item in fields {
        if field_item.is_from && fields.len() != 1 {
            return refuse(
                field_item.span,
                "#[from] is only supported on variants with a single field",
            );
        }
    }
    Ok(())
}

/// Build the `Display` and `source` match bodies and the `From` pairs of an enum.
fn enum_parts(variants: &[ParsedVariant]) -> Result<ErrorParts, ErrorDeriveError> {
    let mut display_arms = Vec::new();
    let mut source_arms = Vec::new();
    let mut from_pairs = Vec::new();

    for variant in variants {
        if variant.is_transparent {
            let (display_arm, source_arm) = transparent_variant_arms(variant);
            display_arms.push(display_arm);
            source_arms.push(source_arm);
        } else if let Some(ref fmt) = variant.error_format {
            display_arms.push(variant_display_arm(variant, fmt)?);
            source_arms.push(variant_source_arm(variant));
        }
        from_pairs.extend(variant_from_pairs(variant));
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

/// The `Display` and `source` arms of a `#[error(transparent)]` variant.
fn transparent_variant_arms(variant: &ParsedVariant) -> (TokenStream, TokenStream) {
    let v_name = &variant.name;
    if variant.is_named {
        let field = variant.fields.first().map(|field_item| &field_item.name);
        (
            quote! {
                Self::#v_name { ref #field } => ::core::fmt::Display::fmt(#field, __formatter),
            },
            quote! {
                Self::#v_name { ref #field } => ::core::error::Error::source(#field),
            },
        )
    } else {
        (
            quote! {
                Self::#v_name(ref __field_0) => ::core::fmt::Display::fmt(__field_0, __formatter),
            },
            quote! {
                Self::#v_name(ref __field_0) => ::core::error::Error::source(__field_0),
            },
        )
    }
}

/// The `Display` arm of a variant that carries an `#[error("...")]` format.
fn variant_display_arm(
    variant: &ParsedVariant,
    fmt: &FormatAttr,
) -> Result<TokenStream, ErrorDeriveError> {
    let v_name = &variant.name;
    let write_args = write_arguments(fmt, variant.is_named, &variant.fields)?;

    Ok(if variant.fields.is_empty() {
        quote! {
            Self::#v_name => ::core::write!(__formatter, #write_args),
        }
    } else if variant.is_named {
        let names: Vec<_> = variant
            .fields
            .iter()
            .filter_map(|field_item| field_item.name.as_ref())
            .collect();
        quote! {
            Self::#v_name { #(ref #names),* } => {
                let __used = (#(#names,)*);
                ::core::write!(__formatter, #write_args)
            }
        }
    } else {
        let bindings = tuple_field_bindings(variant.fields.len());
        quote! {
            Self::#v_name(#(ref #bindings),*) => {
                let __used = (#(#bindings,)*);
                ::core::write!(__formatter, #write_args)
            }
        }
    })
}

/// The reference a `source` arm returns for a field bound by `ref` as `binding`.
///
/// A boxed field is reported as the error inside the box. Coercing `&Box<E>` to
/// `&dyn Error` yields the box itself as the link, so a caller who walks
/// `source()` and downcasts to `E` finds nothing, and `Box<E>::source` then
/// skips `E`'s own layer.
fn borrowed_source(binding: TokenStream, boxed: bool) -> TokenStream {
    if boxed { quote!(&**#binding) } else { binding }
}

/// The reference a `source` body returns for the struct field `place`, with the
/// same boxed rule as [`borrowed_source`].
fn owned_source(place: &TokenStream, boxed: bool) -> TokenStream {
    if boxed {
        quote!(&*#place)
    } else {
        quote!(&#place)
    }
}

/// The `source` arm of a variant: its `#[source]` field, or `None`.
fn variant_source_arm(variant: &ParsedVariant) -> TokenStream {
    let v_name = &variant.name;
    let source_field = variant
        .fields
        .iter()
        .enumerate()
        .find(|&(_, field_item)| field_item.is_source);
    if let Some((idx, field_item)) = source_field {
        let boxed = field_item.is_boxed;
        if variant.is_named {
            let f_name = &field_item.name;
            let source = borrowed_source(quote!(#f_name), boxed);
            quote! {
                Self::#v_name { ref #f_name, .. } => ::core::option::Option::Some(#source),
            }
        } else {
            let bindings = (0..variant.fields.len()).map(|field_idx| {
                if field_idx == idx {
                    quote!(ref __field_source)
                } else {
                    quote!(_)
                }
            });
            let source = borrowed_source(quote!(__field_source), boxed);
            quote! {
                Self::#v_name(#(#bindings),*) => ::core::option::Option::Some(#source),
            }
        }
    } else if variant.fields.is_empty() {
        quote! {
            Self::#v_name => ::core::option::Option::None,
        }
    } else if variant.is_named {
        quote! {
            Self::#v_name { .. } => ::core::option::Option::None,
        }
    } else {
        quote! {
            Self::#v_name(..) => ::core::option::Option::None,
        }
    }
}

/// The `From` impl pieces, one per `#[from]` field of a variant: the source
/// type and the expression that builds the variant from it.
fn variant_from_pairs(variant: &ParsedVariant) -> Vec<(TokenStream, TokenStream)> {
    let v_name = &variant.name;
    variant
        .fields
        .iter()
        .filter(|field_item| field_item.is_from)
        .map(|field_item| {
            let body = if variant.is_named {
                let f_name = &field_item.name;
                quote! { Self::#v_name { #f_name: source } }
            } else {
                quote! { Self::#v_name(source) }
            };
            (field_item.ty.clone(), body)
        })
        .collect()
}

/// Expand a struct definition into error components.
fn expand_struct(
    ident: &Ident,
    outer_attrs: &[Attribute],
    cursor: &mut TokenCursor,
) -> Result<ErrorParts, ErrorDeriveError> {
    let (error_format, is_transparent) =
        find_error_format(outer_attrs, "struct", &ident.to_string())?;

    if error_format.is_none() && !is_transparent {
        return refuse(
            ident.span(),
            format!("missing #[error(...)] attribute on struct '{ident}'"),
        );
    }

    let (fields, is_named) = parse_struct_fields(cursor)?;

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

    let display_body = struct_display(error_format.as_ref(), is_transparent, is_named, &fields)?;
    let source_body = struct_source(is_transparent, is_named, &fields);

    Ok(ErrorParts {
        display: display_body,
        source: source_body,
        from_pairs: struct_from_pairs(is_named, &fields),
    })
}

/// The fields of the struct at `cursor`: braces name them, parentheses number
/// them (and may be followed by `;`), and neither means a unit struct.
fn parse_struct_fields(
    cursor: &mut TokenCursor,
) -> Result<(Vec<ParsedField>, bool), ErrorDeriveError> {
    let Some(TokenTree::Group(grp)) = cursor.next() else {
        return Ok((Vec::new(), false));
    };
    match grp.delimiter() {
        Delimiter::Brace => Ok((parse_fields(grp.stream(), true)?, true)),
        Delimiter::Parenthesis => {
            if let Some(TokenTree::Punct(punct)) = cursor.peek()
                && punct.as_char() == ';'
            {
                cursor.advance();
            }
            Ok((parse_fields(grp.stream(), false)?, false))
        }
        Delimiter::Bracket | Delimiter::None => Ok((Vec::new(), false)),
    }
}

/// The body of the generated `Display::fmt` of a struct.
fn struct_display(
    error_format: Option<&FormatAttr>,
    is_transparent: bool,
    is_named: bool,
    fields: &[ParsedField],
) -> Result<TokenStream, ErrorDeriveError> {
    if is_transparent {
        return Ok(if is_named {
            let field = fields.first().map(|field_item| &field_item.name);
            quote! {
                ::core::fmt::Display::fmt(&self.#field, __formatter)
            }
        } else {
            quote! {
                ::core::fmt::Display::fmt(&self.0, __formatter)
            }
        });
    }
    let Some(fmt) = error_format else {
        return Ok(quote! {
            ::core::write!(__formatter, "")
        });
    };
    let write_args = write_arguments(fmt, is_named, fields)?;

    Ok(if fields.is_empty() {
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
            let __used = (#(#names,)*);
            ::core::write!(__formatter, #write_args)
        }
    } else {
        let bindings = tuple_field_bindings(fields.len());
        quote! {
            let Self(#(ref #bindings),*) = *self;
            let __used = (#(#bindings,)*);
            ::core::write!(__formatter, #write_args)
        }
    })
}

/// The body of the generated `Error::source` of a struct.
fn struct_source(is_transparent: bool, is_named: bool, fields: &[ParsedField]) -> TokenStream {
    if is_transparent {
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
        let boxed = field_item.is_boxed;
        if is_named {
            let f_name = &field_item.name;
            let source = owned_source(&quote!(self.#f_name), boxed);
            quote! {
                ::core::option::Option::Some(#source)
            }
        } else {
            let index = Literal::usize_unsuffixed(idx);
            let source = owned_source(&quote!(self.#index), boxed);
            quote! {
                ::core::option::Option::Some(#source)
            }
        }
    } else {
        quote! {
            ::core::option::Option::None
        }
    }
}

/// The `From` impl pieces, one per `#[from]` field of a struct.
fn struct_from_pairs(is_named: bool, fields: &[ParsedField]) -> Vec<(TokenStream, TokenStream)> {
    fields
        .iter()
        .filter(|field_item| field_item.is_from)
        .map(|field_item| {
            let body = if is_named {
                let f_name = &field_item.name;
                quote! { Self { #f_name: source } }
            } else {
                quote! { Self(source) }
            };
            (field_item.ty.clone(), body)
        })
        .collect()
}
