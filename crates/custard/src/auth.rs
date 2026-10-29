//! Signing people in with the environment's sign-in service, as The Custard
//! Factory's contract describes: OpenID Connect, authorisation code flow with
//! PKCE, tokens checked here against the service's published keys.
//!
//! | Route | Does |
//! | --- | --- |
//! | `GET /_custard/auth/login?return_to=/path` | Sign in, or carry on a session that has expired, then go to the path |
//! | `GET /_custard/auth/callback` | Where the sign-in service sends people back |
//! | `GET /_custard/auth/logout` | Sign out, here and at the service |
//!
//! In a page or route:
//!
//! ```rust,ignore
//! let user = custard::auth::require_user(cx).await?;       // or sends them to sign in
//! let admin = custard::auth::require_role(cx, "admin").await?; // or 403
//! let maybe = custard::auth::user(cx).await;                // Option<&User>
//! ```
//!
//! The session is kept in a cookie encrypted with a key made from the client
//! secret: it holds the ID token, checked on every request, and the refresh
//! token that renews it. API clients send `Authorization: Bearer <token>`.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use topcoat::{
    Result,
    context::{Cx, memoize, try_app_context},
    cookie::{Cookie, Cookies, Key, SameSite, cookies, time},
    router::{
        Body, Method, RouteFn, RouteFuture, StatusCode,
        error::{SeeOther, forbidden, see_other, unauthorized},
        request,
        response::{IntoResponse, Response},
    },
};

use crate::env::AuthEnv;

pub const LOGIN_PATH: &str = "/_custard/auth/login";
pub const CALLBACK_PATH: &str = "/_custard/auth/callback";
pub const LOGOUT_PATH: &str = "/_custard/auth/logout";

const SESSION_COOKIE: &str = "custard_session";
const FLOW_COOKIE: &str = "custard_flow";
const FLOW_PATH: &str = "/_custard/auth";

/// The scopes that ask for every claim a [`User`] has.
const SCOPES: &str = "openid profile email offline_access \
    urn:zitadel:iam:org:project:roles urn:zitadel:iam:user:resourceowner";

/// The person signed in, in the contract's names, whatever the service behind
/// the tokens calls them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct User {
    /// Stable for the life of the environment: store this to refer to them.
    pub user_id: String,
    pub email: String,
    pub email_verified: bool,
    /// For showing.
    pub name: String,
    /// The organisation they belong to: one of the app's customers.
    pub org_id: String,
    pub org_name: String,
    /// Their roles in the app, such as `admin`, sorted.
    pub roles: Vec<String>,
}

impl User {
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|found| found == role)
    }

    /// From a token's claims. Zitadel's names are known here and nowhere else.
    fn from_claims(claims: &serde_json::Value) -> Option<User> {
        let text = |name: &str| {
            claims
                .get(name)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned()
        };
        let mut roles: Vec<String> = claims
            .get("urn:zitadel:iam:org:project:roles")
            .and_then(|roles| roles.as_object())
            .map(|roles| roles.keys().cloned().collect())
            .unwrap_or_default();
        roles.sort();
        Some(User {
            user_id: claims.get("sub")?.as_str()?.to_owned(),
            email: text("email"),
            email_verified: claims
                .get("email_verified")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            name: text("name"),
            org_id: text("urn:zitadel:iam:user:resourceowner:id"),
            org_name: text("urn:zitadel:iam:user:resourceowner:name"),
            roles,
        })
    }
}

/// The signed-in user, if anyone is: from a bearer token, or the session
/// cookie. Checked once per request.
pub async fn user(cx: &Cx) -> Option<&User> {
    current_user(cx).await.as_ref()
}

/// The signed-in user. A browser asking for a page is sent to sign in and
/// brought back; anything else gets 401.
pub async fn require_user(cx: &Cx) -> Result<&User> {
    match user(cx).await {
        Some(user) => Ok(user),
        None if wants_html(cx) => Err(see_other(login_url(cx)).into()),
        None => Err(unauthorized().into()),
    }
}

/// A signed-in user with `role`, or 403.
pub async fn require_role<'a>(cx: &'a Cx, role: &str) -> Result<&'a User> {
    let user = require_user(cx).await?;
    if user.has_role(role) {
        Ok(user)
    } else {
        Err(forbidden().into())
    }
}

/// Where to send someone to sign in and come back to this page.
pub fn login_url(cx: &Cx) -> String {
    let back = if request::method(cx) == "GET" {
        request::uri(cx)
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
            .to_owned()
    } else {
        "/".to_owned()
    };
    format!("{LOGIN_PATH}?{}", query(&[("return_to", &back)]))
}

#[memoize]
async fn current_user(cx: &Cx) -> Option<User> {
    let auth = try_app_context::<Arc<Auth>>(cx)?;
    if let Some(user) = &auth.dev_user {
        return Some(user.clone());
    }
    let token = match bearer(cx) {
        Some(token) => token,
        None => auth.session(cx)?.id_token,
    };
    match auth.verify(&token, None).await {
        Ok(user) => Some(user),
        Err(error) => {
            tracing::debug!(%error, "token not accepted");
            None
        }
    }
}

// -- The service ---------------------------------------------------------------

/// The sign-in service, its keys, and the cookies' key. Shared by every request.
pub struct Auth {
    settings: AuthEnv,
    app_origin: String,
    secure_cookies: bool,
    http: reqwest::Client,
    keys: RwLock<KeyCache>,
    cookie_key: Key,
    dev_user: Option<User>,
}

#[derive(Default)]
struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Option<Instant>,
}

/// Keys are kept this long, and read again at most this often when a token
/// names one not known, as the service rotates them.
const KEEP_KEYS: Duration = Duration::from_secs(600);
const REFETCH_KEYS: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("the token is not one this app can read")]
    Malformed,
    #[error("the token was signed with an unknown key, {0}")]
    UnknownKey(String),
    #[error("the token was not accepted: {0}")]
    Invalid(String),
    #[error("the token is not from the sign-in this app started")]
    WrongNonce,
}

#[derive(Serialize, Deserialize)]
struct Session {
    #[serde(rename = "id")]
    id_token: String,
    #[serde(rename = "refresh")]
    refresh_token: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Flow {
    state: String,
    nonce: String,
    verifier: String,
    return_to: String,
}

impl Auth {
    pub fn new(settings: AuthEnv, app_origin: &str) -> Auth {
        // An internal app has no secret and signs nobody in; its cookie key
        // is never used, but must still be made from something.
        let secret = settings
            .client_secret
            .clone()
            .unwrap_or_else(|| settings.client_id.clone());
        Auth {
            app_origin: app_origin.to_owned(),
            secure_cookies: app_origin.starts_with("https://"),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("an HTTP client"),
            keys: RwLock::new(KeyCache::default()),
            cookie_key: cookie_key(&secret),
            settings,
            dev_user: None,
        }
    }

    /// Everyone is signed in as this user, for working on an app without the
    /// sign-in service. Only ever used in debug builds.
    pub fn dev(user: User, app_origin: &str) -> Auth {
        let mut auth = Auth::new(
            AuthEnv {
                issuer: String::new(),
                url: String::new(),
                client_id: "dev".into(),
                client_secret: None,
            },
            app_origin,
        );
        auth.dev_user = Some(user);
        auth
    }

    /// The user a token is for, once its signature, issuer, audience, expiry
    /// and, when signing in, nonce are checked.
    pub async fn verify(&self, token: &str, nonce: Option<&str>) -> Result<User, TokenError> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| TokenError::Malformed)?;
        if header.alg != Algorithm::RS256 {
            return Err(TokenError::Malformed);
        }
        let kid = header.kid.ok_or(TokenError::Malformed)?;
        let key = self.key(&kid).await.ok_or(TokenError::UnknownKey(kid))?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.settings.issuer]);
        validation.set_audience(&[&self.settings.client_id]);
        validation.leeway = 60;
        let data = jsonwebtoken::decode::<serde_json::Value>(token, &key, &validation)
            .map_err(|error| TokenError::Invalid(error.to_string()))?;

        if let Some(expected) = nonce
            && data.claims.get("nonce").and_then(|n| n.as_str()) != Some(expected)
        {
            return Err(TokenError::WrongNonce);
        }
        User::from_claims(&data.claims).ok_or(TokenError::Malformed)
    }

    async fn key(&self, kid: &str) -> Option<DecodingKey> {
        {
            let cache = self.keys.read().await;
            let fresh = cache.fetched_at.is_some_and(|at| at.elapsed() < KEEP_KEYS);
            if fresh && let Some(key) = cache.keys.get(kid) {
                return Some(key.clone());
            }
        }
        let mut cache = self.keys.write().await;
        let since = cache.fetched_at.map(|at| at.elapsed());
        let stale = since.is_none_or(|since| since >= KEEP_KEYS);
        let unknown = !cache.keys.contains_key(kid);
        if stale || (unknown && since.is_none_or(|since| since >= REFETCH_KEYS)) {
            match self.fetch_keys().await {
                Ok(keys) => {
                    cache.keys = keys;
                    cache.fetched_at = Some(Instant::now());
                }
                // Kept as they were: the service may be back by the next try.
                Err(error) => tracing::warn!(%error, "could not read the sign-in service's keys"),
            }
        }
        cache.keys.get(kid).cloned()
    }

    async fn fetch_keys(&self) -> Result<HashMap<String, DecodingKey>, String> {
        #[derive(Deserialize)]
        struct Jwks {
            keys: Vec<Jwk>,
        }
        #[derive(Deserialize)]
        struct Jwk {
            kty: String,
            kid: Option<String>,
            n: Option<String>,
            e: Option<String>,
        }
        let jwks: Jwks = self
            .service(reqwest::Method::GET, "/oauth/v2/keys")
            .send()
            .await
            .and_then(|res| res.error_for_status())
            .map_err(|error| error.to_string())?
            .json()
            .await
            .map_err(|error| error.to_string())?;
        Ok(jwks
            .keys
            .into_iter()
            .filter(|jwk| jwk.kty == "RSA")
            .filter_map(|jwk| {
                let key =
                    DecodingKey::from_rsa_components(jwk.n.as_deref()?, jwk.e.as_deref()?).ok()?;
                Some((jwk.kid?, key))
            })
            .collect())
    }

    // A request to the sign-in service, at the address this server reaches it
    // at, for the host browsers know it by.
    fn service(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let request = self
            .http
            .request(method, format!("{}{path}", self.settings.url));
        if self.settings.url == self.settings.issuer {
            return request;
        }
        match url::Url::parse(&self.settings.issuer) {
            Ok(issuer) => match (issuer.host_str(), issuer.port()) {
                (Some(host), Some(port)) => request.header("host", format!("{host}:{port}")),
                (Some(host), None) => request.header("host", host),
                _ => request,
            },
            Err(_) => request,
        }
    }

    // Ask the token endpoint for tokens, and keep them once the ID token is
    // checked, against the nonce when signing in.
    async fn exchange(
        &self,
        form: &[(&str, &str)],
        nonce: Option<&str>,
    ) -> Result<Session, String> {
        #[derive(Deserialize)]
        struct Tokens {
            id_token: String,
            refresh_token: Option<String>,
        }
        let secret = self
            .settings
            .client_secret
            .as_deref()
            .ok_or("this app has no client secret, so it cannot sign people in")?;
        let response = self
            .service(reqwest::Method::POST, "/oauth/v2/token")
            .basic_auth(
                form_encode(&self.settings.client_id),
                Some(form_encode(secret)),
            )
            .header("content-type", "application/x-www-form-urlencoded")
            .body(query(form))
            .send()
            .await
            .map_err(|_| "The sign-in service could not be reached.".to_owned())?;
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "The sign-in service refused: {}",
                body.chars().take(200).collect::<String>()
            ));
        }
        let tokens: Tokens = response
            .json()
            .await
            .map_err(|_| "The sign-in service's answer could not be read.".to_owned())?;
        self.verify(&tokens.id_token, nonce)
            .await
            .map_err(|error| format!("The sign-in could not be checked: {error}"))?;
        Ok(Session {
            id_token: tokens.id_token,
            refresh_token: tokens.refresh_token,
        })
    }

    fn jar<'a>(&'a self, cx: &'a Cx) -> impl Cookies + 'a {
        cookies(cx)
            .default_secure(self.secure_cookies)
            .default_http_only(true)
            .default_same_site(SameSite::Lax)
            .default_path("/")
            .private(&self.cookie_key)
    }

    fn session(&self, cx: &Cx) -> Option<Session> {
        let cookie = self.jar(cx).get(SESSION_COOKIE)?;
        serde_json::from_str(cookie.value()).ok()
    }

    fn set_session(&self, cx: &Cx, session: &Session) {
        let value = serde_json::to_string(session).expect("a session serialises");
        let mut cookie = Cookie::new(SESSION_COOKIE, value);
        cookie.set_max_age(time::Duration::days(30));
        self.jar(cx).add(cookie);
    }

    fn flow(&self, cx: &Cx) -> Option<Flow> {
        let cookie = self.jar(cx).get(FLOW_COOKIE)?;
        serde_json::from_str(cookie.value()).ok()
    }

    fn set_flow(&self, cx: &Cx, flow: &Flow) {
        let value = serde_json::to_string(flow).expect("a flow serialises");
        let mut cookie = Cookie::new(FLOW_COOKIE, value);
        cookie.set_path(FLOW_PATH);
        cookie.set_max_age(time::Duration::minutes(10));
        self.jar(cx).add(cookie);
    }

    fn clear_flow(&self, cx: &Cx) {
        let mut cookie = Cookie::new(FLOW_COOKIE, "");
        cookie.set_path(FLOW_PATH);
        self.jar(cx).remove(cookie);
    }

    fn callback_url(&self) -> String {
        format!("{}{CALLBACK_PATH}", self.app_origin)
    }
}

/// The cookies' key, from the client secret: the same on every server of the
/// app, and across deploys.
fn cookie_key(secret: &str) -> Key {
    let master = Sha256::digest(format!("custard/session:{secret}"));
    Key::derive_from(&master)
}

// -- Routes --------------------------------------------------------------------

/// Signing in, coming back, and signing out, under `/_custard/auth`.
pub(crate) fn routes() -> [RouteFn; 3] {
    fn login_route(cx: &Cx, _body: Body) -> RouteFuture<'_> {
        Box::pin(async move { login(cx).await?.into_response(cx) })
    }
    fn callback_route(cx: &Cx, _body: Body) -> RouteFuture<'_> {
        Box::pin(callback(cx))
    }
    fn logout_route(cx: &Cx, _body: Body) -> RouteFuture<'_> {
        Box::pin(async move { logout(cx).await?.into_response(cx) })
    }
    [
        RouteFn::new(Method::GET, LOGIN_PATH, login_route),
        RouteFn::new(Method::GET, CALLBACK_PATH, callback_route),
        RouteFn::new(Method::GET, LOGOUT_PATH, logout_route),
    ]
}

fn auth(cx: &Cx) -> Result<&Arc<Auth>> {
    try_app_context::<Arc<Auth>>(cx)
        .ok_or_else(|| topcoat::router::error::service_unavailable(60).into())
}

async fn login(cx: &Cx) -> Result<SeeOther> {
    let auth = auth(cx)?;
    let return_to = safe_path(query_value(cx, "return_to").as_deref());
    if auth.dev_user.is_some() {
        return Ok(see_other(return_to));
    }
    // Sign in on the address the service sends people back to, or the flow's
    // cookie would be on another host than the callback.
    if let Some(host) = request::headers(cx)
        .get("host")
        .and_then(|h| h.to_str().ok())
        && !auth.app_origin.ends_with(&format!("//{host}"))
    {
        return Ok(see_other(format!(
            "{}{LOGIN_PATH}?{}",
            auth.app_origin,
            query(&[("return_to", &return_to)])
        )));
    }
    // A session that has expired is carried on without a trip to the service.
    if let Some(refresh_token) = auth.session(cx).and_then(|s| s.refresh_token) {
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
            ("scope", SCOPES),
        ];
        match auth.exchange(&form, None).await {
            Ok(session) => {
                auth.set_session(cx, &session);
                return Ok(see_other(return_to));
            }
            Err(error) => tracing::info!(%error, "could not renew a session"),
        }
    }
    let flow = Flow {
        state: random(),
        nonce: random(),
        verifier: random(),
        return_to,
    };
    let authorize = format!(
        "{}/oauth/v2/authorize?{}",
        auth.settings.issuer,
        query(&[
            ("response_type", "code"),
            ("client_id", &auth.settings.client_id),
            ("redirect_uri", &auth.callback_url()),
            ("scope", SCOPES),
            ("state", &flow.state),
            ("nonce", &flow.nonce),
            ("code_challenge", &challenge(&flow.verifier)),
            ("code_challenge_method", "S256"),
        ])
    );
    auth.set_flow(cx, &flow);
    Ok(see_other(authorize))
}

async fn callback(cx: &Cx) -> Result<Response> {
    let auth = auth(cx)?;
    let flow = auth.flow(cx);
    auth.clear_flow(cx);
    let outcome = match (flow, query_value(cx, "error")) {
        (_, Some(error)) => Err(format!(
            "The sign-in service said: {}",
            query_value(cx, "error_description").unwrap_or(error)
        )),
        (None, _) => Err("This sign-in was not started here, or took too long. Try again.".into()),
        (Some(flow), None) => match (query_value(cx, "state"), query_value(cx, "code")) {
            (Some(state), Some(code)) if state == flow.state => {
                let callback = auth.callback_url();
                let form = [
                    ("grant_type", "authorization_code"),
                    ("code", code.as_str()),
                    ("redirect_uri", callback.as_str()),
                    ("code_verifier", flow.verifier.as_str()),
                ];
                auth.exchange(&form, Some(&flow.nonce))
                    .await
                    .map(|session| (session, flow.return_to))
            }
            _ => Err("This sign-in does not match the one started here. Try again.".into()),
        },
    };
    match outcome {
        Ok((session, return_to)) => {
            auth.set_session(cx, &session);
            see_other(return_to).into_response(cx)
        }
        Err(message) => {
            tracing::warn!(%message, "sign-in failed");
            (StatusCode::BAD_REQUEST, message).into_response(cx)
        }
    }
}

async fn logout(cx: &Cx) -> Result<SeeOther> {
    let auth = auth(cx)?;
    if auth.dev_user.is_some() {
        return Ok(see_other("/"));
    }
    // Whose session to end, so the service need not ask.
    let hint = auth.session(cx).map(|s| s.id_token);
    auth.jar(cx).remove(Cookie::new(SESSION_COOKIE, ""));
    let post_logout = format!("{}/", auth.app_origin);
    let mut params = vec![
        ("client_id", auth.settings.client_id.as_str()),
        ("post_logout_redirect_uri", post_logout.as_str()),
    ];
    if let Some(hint) = &hint {
        params.push(("id_token_hint", hint));
    }
    Ok(see_other(format!(
        "{}/oidc/v1/end_session?{}",
        auth.settings.issuer,
        query(&params)
    )))
}

// -- Helpers -------------------------------------------------------------------

fn bearer(cx: &Cx) -> Option<String> {
    let value = request::headers(cx).get("authorization")?.to_str().ok()?;
    value
        .strip_prefix("Bearer ")
        .map(|token| token.trim().to_owned())
}

fn wants_html(cx: &Cx) -> bool {
    request::headers(cx)
        .get("accept")
        .and_then(|accept| accept.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

fn query_value(cx: &Cx, name: &str) -> Option<String> {
    let query = request::uri(cx).query()?;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn query(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

fn form_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// Only a path in this app: never another site.
fn safe_path(path: Option<&str>) -> String {
    match path {
        Some(path)
            if path.starts_with('/') && !path.starts_with("//") && !path.starts_with("/\\") =>
        {
            path.to_owned()
        }
        _ => "/".to_owned(),
    }
}

/// Random text for a state, nonce or PKCE verifier.
fn random() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The PKCE challenge for a verifier: its SHA-256, base64url.
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_the_contract_names_from_zitadel_claims() {
        let user = User::from_claims(&json!({
            "sub": "123",
            "email": "ana@example.com",
            "email_verified": true,
            "name": "Ana",
            "urn:zitadel:iam:user:resourceowner:id": "org1",
            "urn:zitadel:iam:user:resourceowner:name": "Acme",
            "urn:zitadel:iam:org:project:roles": { "editor": {}, "admin": {} },
        }))
        .unwrap();
        assert_eq!(user.user_id, "123");
        assert_eq!(user.org_name, "Acme");
        assert_eq!(user.roles, ["admin", "editor"]);
        assert!(user.has_role("admin"));
    }

    #[test]
    fn an_access_token_lacks_what_it_lacks() {
        let user = User::from_claims(&json!({ "sub": "machine" })).unwrap();
        assert_eq!(user.email, "");
        assert!(user.roles.is_empty());
        assert!(User::from_claims(&json!({ "email": "x" })).is_none());
    }

    #[test]
    fn only_returns_to_paths_here() {
        assert_eq!(safe_path(Some("/notes?page=2")), "/notes?page=2");
        assert_eq!(safe_path(Some("//evil.example")), "/");
        assert_eq!(safe_path(Some("/\\evil.example")), "/");
        assert_eq!(safe_path(Some("https://evil.example")), "/");
        assert_eq!(safe_path(None), "/");
    }

    #[test]
    fn pkce_challenge_is_unpadded_base64url_sha256() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mJ92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "ngF5GsXcbwljx6u133FFr3Xht9xooA_DuaX_3QwODtc"
        );
    }
}
