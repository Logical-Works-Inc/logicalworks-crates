//! The `serve` word: the tenant HTTP edge as one flow step.

use lgwks_ast::script::{RouteMethod, Serve};
use lgwks_deps::proc_macro2::TokenStream;
use lgwks_deps::quote::quote;

use super::{body, bound, code, runtime, site_of};
use crate::error::Result;

/// `serve on port ..., at most N runs in flight:` and its `route`s.
///
/// Each route becomes a [`TypedRoute`](::lgwks_bot::rt::serve::TypedRoute)
/// whose closure decodes its typed body through `lgwks_std::json` (a failure
/// is `400` naming the field) and runs its block; the serve answers `503`
/// past its ceiling and drains on stop.
pub(super) fn serve(declared: &Serve) -> Result<TokenStream> {
    let script = runtime();
    let site = site_of(declared.site());
    let label = declared.label();
    let max = bound(declared.bound())?;
    let options = match *declared.port() {
        lgwks_ast::script::ServePort::Env(ref name) => {
            quote!(#script::ServeOptions::from_env_ephemeral(#name, #max)?)
        }
        lgwks_ast::script::ServePort::Expr(ref port_code) => {
            let port = code(port_code)?;
            quote!(#script::ServeOptions::new_ephemeral(#port, #max)?)
        }
        _ => {
            lgwks_std::trace::debug!(
                "emit: the serve port is from a newer word than this macro generates"
            );
            return Err(super::newer_word("serve port"));
        }
    };
    let mut routes = TokenStream::new();
    for route in declared.routes() {
        routes.extend(route_tokens(route)?);
    }
    Ok(quote! {
        #script::serve_at(scope, #label, #site, #options, ::std::vec![#routes]).await
    })
}

/// One route as a `TypedRoute` with its typed closure.
fn route_tokens(route: &lgwks_ast::script::Route) -> Result<TokenStream> {
    let script = runtime();
    let method = match route.method() {
        RouteMethod::Get => quote!(::lgwks_bot::rt::serve::RouteMethod::Get),
        RouteMethod::Post => quote!(::lgwks_bot::rt::serve::RouteMethod::Post),
        _ => {
            lgwks_std::trace::debug!(
                "emit: the route method is from a newer word than this macro generates"
            );
            return Err(super::newer_word("route method"));
        }
    };
    let path = route.path();
    let body_ty = code(route.body_ty())?;
    let output_ty = code(route.output_ty())?;
    let site = site_of(route.site());
    let label = route.label();
    let inner = body(route.body())?.into_ok_block();
    Ok(quote! {
        ::lgwks_bot::rt::serve::TypedRoute::new(#method, #path, {
            let site = #site;
            let label = #label;
            move |tenant: ::std::string::String, body_bytes: ::std::vec::Vec<u8>| {
                let site = site;
                let label: &'static str = label;
                ::std::boxed::Box::pin(async move {
                    let raw: &[u8] = if body_bytes.is_empty() { b"null" } else { &body_bytes };
                    let decoded: #body_ty = match ::lgwks_std::json::from_slice(raw) {
                        ::core::result::Result::Ok(value) => value,
                        ::core::result::Result::Err(error) => {
                            let message = ::std::format!(
                                "the body does not decode as this route's input: {error}"
                            );
                            return ::core::result::Result::Err((400u16, message));
                        }
                    };
                    let outcome: ::core::result::Result<#output_ty, #script::FlowError> = async {
                        let scope = &#script::Scope::root(#script::Tenant::new(&tenant).map_err(|error| {
                            #script::FlowError::failed(::std::format!("the tenant is refused: {error}"))
                        })?);
                        let scope = &scope.enter_at(label, site)?;
                        let body = decoded;
                        let _ = &body;
                        #inner
                    }.await;
                    match outcome {
                        ::core::result::Result::Ok(value) => {
                            match ::lgwks_std::json::to_vec(&value) {
                                ::core::result::Result::Ok(bytes) => ::core::result::Result::Ok(bytes),
                                ::core::result::Result::Err(error) => {
                                    ::core::result::Result::Err((500u16, ::std::format!("the route output could not be encoded: {error}")))
                                }
                            }
                        }
                        ::core::result::Result::Err(error) => {
                            ::core::result::Result::Err((500u16, ::std::format!("the route handler failed: {error}")))
                        }
                    }
                })
            }
        }),
    })
}
