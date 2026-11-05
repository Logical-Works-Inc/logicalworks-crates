//! OAuth 2.1 authorization code with PKCE: two tenants get distinct state and verifiers.

use lgwks_deps::oauth2::basic::BasicClient;
use lgwks_deps::oauth2::{
    AuthUrl, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope, TokenUrl,
};

/// What one tenant's authorization step hands the browser and keeps back.
struct Authorization {
    /// The URL the user is sent to.
    url: String,
    /// The CSRF state the callback must echo.
    state: String,
    /// The PKCE verifier the token exchange presents.
    verifier: String,
}

fn main() {
    if !journey() {
        std::process::exit(1);
    }
}

fn authorize(tenant: &str) -> Option<Authorization> {
    let client = BasicClient::new(ClientId::new(format!("larry-{tenant}")))
        .set_auth_uri(AuthUrl::new(format!("https://id.example/{tenant}/authorize")).ok()?)
        .set_token_uri(TokenUrl::new(format!("https://id.example/{tenant}/token")).ok()?)
        .set_redirect_uri(RedirectUrl::new("http://127.0.0.1:8765/callback".into()).ok()?);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("tools:call".into()))
        .set_pkce_challenge(challenge)
        .url();
    Some(Authorization {
        url: url.to_string(),
        state: state.secret().clone(),
        verifier: verifier.secret().clone(),
    })
}

fn journey() -> bool {
    let (Some(alpha), Some(beta)) = (authorize("alpha"), authorize("beta")) else {
        return false;
    };
    let own = |grant: &Authorization, tenant: &str| {
        grant
            .url
            .starts_with(&format!("https://id.example/{tenant}/authorize?"))
            && grant.url.contains("code_challenge_method=S256")
            && grant.url.contains(&format!("client_id=larry-{tenant}"))
            && grant.verifier.len() >= 43
    };
    own(&alpha, "alpha")
        && own(&beta, "beta")
        && !alpha.url.contains("larry-beta")
        && alpha.state != beta.state
        && alpha.verifier != beta.verifier
}
