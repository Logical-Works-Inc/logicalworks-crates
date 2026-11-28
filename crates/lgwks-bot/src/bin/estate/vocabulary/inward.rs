//! The smoke's address-rule section (#1170): `lgwks_std::http` under a rule for
//! a URL somebody else supplied.
//!
//! A URL naming loopback and a name answering the cloud metadata address are
//! refused, and an admitted answer completes under its name. The loopback
//! server takes one connection in all, so a refused call opened none. This is
//! the caller of `Options::address_rule` and `Options::lookup` on the shipped
//! binary.

use std::net::{IpAddr, Ipv4Addr};

use lgwks_bot::rt::runtime::Handle;
use lgwks_std::http::{self, Error as HttpError, Options};

use super::super::say;
use super::outside::OnceUp;
use super::{Section, VocabularyError, refuse};

/// A rule as a service writes one for a URL somebody else supplied: no
/// loopback, private or link-local address.
fn public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => !(v4.is_loopback() || v4.is_private() || v4.is_link_local()),
        IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local()),
    }
}

/// Drive the address rule against a loopback server of its own.
///
/// # Errors
///
/// The first failed check, or the error the server or the report returned.
pub(super) fn run(driver: &Handle) -> Result<(), VocabularyError> {
    let server = OnceUp::serve(b"hello".to_vec(), 1)?;
    let guarded = Options::default().address_rule(public);
    if !matches!(
        http::get_with(&server.url(), &guarded),
        Err(HttpError::AddressRefused { address, .. }) if address == Ipv4Addr::LOCALHOST
    ) {
        return refuse(Section::Outside, "a URL naming loopback is refused");
    }
    say("outside: a URL naming loopback is refused by the address rule")?;

    let metadata = IpAddr::from([169, 254, 169, 254]);
    let answering_metadata = guarded.lookup(move |_name| Ok(vec![metadata]));
    if !matches!(
        http::get_with("http://inward.example/", &answering_metadata),
        Err(HttpError::AddressRefused { host, address })
            if host == "inward.example" && address == metadata
    ) {
        return refuse(Section::Outside, "a name answering metadata is refused");
    }
    say("outside: a name answering 169.254.169.254 is refused by the address rule")?;

    let admitted = Options::default()
        .address_rule(|address| address.is_loopback())
        .lookup(|_name| Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]));
    let named = server.url_at("upstream.example");
    match http::get_with(&named, &admitted) {
        Ok(response) if response.status() == 200 && response.body() == b"hello" => {}
        other => {
            lgwks_std::trace::debug!(outcome = ?other, "address rule: the admitted call deviated");
            return refuse(Section::Outside, "an admitted answer completes");
        }
    }
    if server.hits() != 1 {
        return refuse(Section::Outside, "the refused calls opened no connection");
    }
    server.join(driver);
    say("outside: an admitted answer completes under its name, one connection in all")?;
    Ok(())
}
