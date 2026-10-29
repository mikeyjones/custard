//! Signing in end to end, against a fake sign-in service that behaves like
//! Zitadel as The Custard Factory's contract describes it: JWKS at
//! `/oauth/v2/keys`, the token endpoint at `/oauth/v2/token`, RS256 tokens.
//! The app is reached through `Router::handle`; the app reaches the service
//! over HTTP, at `CUSTARD_AUTH_URL`, with the issuer's host in `Host`.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use custard::{Env, Platform, RouterBuilderCustardExt, auth::Auth};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::json;
use topcoat::{
    context::{Cx, app_context},
    router::{
        Body, Method, RouteFn, RouteFuture, Router, StatusCode, request,
        response::{IntoResponse, Response},
    },
};

const ISSUER: &str = "https://auth--test.example.com";
const CLIENT_ID: &str = "client123";
const CLIENT_SECRET: &str = "s3cret:with/odd chars";
const APP_ORIGIN: &str = "http://localhost:3000";
const KEY_ID: &str = "key-1";

// -- The fake sign-in service ---------------------------------------------------

#[derive(Default)]
struct Service {
    nonce: Mutex<Option<String>>,
    hosts: Mutex<Vec<String>>,
    refreshes: AtomicUsize,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn sign(claims: serde_json::Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(KEY_ID.into());
    let key = EncodingKey::from_rsa_pem(include_bytes!("fixtures/test-signing-key.pem")).unwrap();
    jsonwebtoken::encode(&header, &claims, &key).unwrap()
}

fn id_token(audience: &str, nonce: Option<&str>) -> String {
    let mut claims = json!({
        "iss": ISSUER,
        "aud": [audience, "project-1"],
        "sub": "user-1",
        "exp": now() + 900,
        "iat": now(),
        "email": "ana@example.com",
        "email_verified": true,
        "name": "Ana",
        "urn:zitadel:iam:user:resourceowner:id": "org-1",
        "urn:zitadel:iam:user:resourceowner:name": "Acme",
        "urn:zitadel:iam:org:project:roles": { "admin": { "org-1": "acme" } },
    });
    if let Some(nonce) = nonce {
        claims["nonce"] = json!(nonce);
    }
    sign(claims)
}

fn keys(cx: &Cx, _body: Body) -> RouteFuture<'_> {
    Box::pin(async move {
        let service = app_context::<Arc<Service>>(cx);
        service.hosts.lock().unwrap().push(host(cx));
        let n = include_str!("fixtures/test-signing-key.n").trim();
        let jwks = json!({ "keys": [{ "kty": "RSA", "kid": KEY_ID, "alg": "RS256", "use": "sig", "n": n, "e": "AQAB" }] });
        (StatusCode::OK, jwks.to_string()).into_response(cx)
    })
}

fn token(cx: &Cx, body: Body) -> RouteFuture<'_> {
    Box::pin(async move {
        let service = app_context::<Arc<Service>>(cx);
        service.hosts.lock().unwrap().push(host(cx));
        let expected = format!(
            "Basic {}",
            STANDARD.encode(format!("{}:{}", encode(CLIENT_ID), encode(CLIENT_SECRET)))
        );
        let authorization = request::headers(cx)
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        if authorization != Some(expected.as_str()) {
            return (StatusCode::UNAUTHORIZED, "bad client credentials").into_response(cx);
        }
        let bytes = body.collect().await.unwrap().to_bytes();
        let form: HashMap<String, String> =
            url::form_urlencoded::parse(&bytes).into_owned().collect();
        let nonce = match form.get("grant_type").map(String::as_str) {
            Some("authorization_code") => {
                let ok = form.get("code").map(String::as_str) == Some("the-code")
                    && form.get("code_verifier").is_some_and(|v| v.len() >= 43)
                    && form.get("redirect_uri").map(String::as_str)
                        == Some("http://localhost:3000/_custard/auth/callback");
                if !ok {
                    return (StatusCode::BAD_REQUEST, "invalid_grant").into_response(cx);
                }
                service.nonce.lock().unwrap().clone()
            }
            Some("refresh_token")
                if form.get("refresh_token").map(String::as_str) == Some("refresh-1") =>
            {
                service.refreshes.fetch_add(1, Ordering::SeqCst);
                None
            }
            _ => return (StatusCode::BAD_REQUEST, "invalid_grant").into_response(cx),
        };
        let tokens = json!({
            "access_token": "opaque",
            "token_type": "Bearer",
            "expires_in": 900,
            "id_token": id_token(CLIENT_ID, nonce.as_deref()),
            "refresh_token": "refresh-1",
        });
        (StatusCode::OK, tokens.to_string()).into_response(cx)
    })
}

fn host(cx: &Cx) -> String {
    request::headers(cx)
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

async fn start_service() -> (String, Arc<Service>) {
    let service = Arc::new(Service::default());
    let router = Router::builder()
        .route(RouteFn::new(Method::GET, "/oauth/v2/keys", keys))
        .route(RouteFn::new(Method::POST, "/oauth/v2/token", token))
        .app_context(service.clone())
        .build();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { topcoat::serve(listener, router).await.unwrap() });
    (format!("http://{address}"), service)
}

// -- The app ---------------------------------------------------------------------

/// `GET /me`: who is signed in, or what `require_user` answers.
fn me(cx: &Cx, _body: Body) -> RouteFuture<'_> {
    Box::pin(async move {
        let user = custard::auth::require_user(cx).await?;
        let admin = custard::auth::require_role(cx, "admin").await.is_ok();
        (
            StatusCode::OK,
            format!(
                "{} {} {} admin={admin}",
                user.user_id, user.email, user.org_name
            ),
        )
            .into_response(cx)
    })
}

fn app(service_url: &str) -> Router {
    let vars: HashMap<&str, String> = HashMap::from([
        ("APP_ORIGIN", APP_ORIGIN.into()),
        ("CUSTARD_AUTH_ISSUER", ISSUER.into()),
        ("CUSTARD_AUTH_URL", service_url.into()),
        ("CUSTARD_AUTH_CLIENT_ID", CLIENT_ID.into()),
        ("CUSTARD_AUTH_CLIENT_SECRET", CLIENT_SECRET.into()),
    ]);
    let env = Env::from_vars(|name| vars.get(name).cloned());
    let auth = Arc::new(Auth::new(env.auth.clone().unwrap(), &env.app_origin));
    let platform = Platform {
        env,
        db: None,
        auth: Some(auth),
        storage: None,
    };
    Router::builder()
        .route(RouteFn::new(Method::GET, "/me", me))
        .custard(&platform)
        .build()
}

/// A browser: keeps cookies between requests.
struct Browser {
    router: Router,
    cookies: HashMap<String, String>,
}

impl Browser {
    async fn get(&mut self, path: &str, headers: &[(&str, &str)]) -> (StatusCode, String, String) {
        let mut builder = http::Request::builder()
            .uri(path)
            .header("host", "localhost:3000");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        if !self.cookies.is_empty() {
            let cookie: Vec<String> = self
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            builder = builder.header("cookie", cookie.join("; "));
        }
        let response: Response = self
            .router
            .handle(builder.body(Body::empty()).unwrap())
            .await;
        for set in response.headers().get_all("set-cookie") {
            let set = set.to_str().unwrap();
            let pair = set.split(';').next().unwrap();
            let (name, value) = pair.split_once('=').unwrap();
            if set.contains("Max-Age=0") || value.is_empty() {
                self.cookies.remove(name);
            } else {
                self.cookies.insert(name.to_owned(), value.to_owned());
            }
        }
        let status = response.status();
        let location = response
            .headers()
            .get("location")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let body = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        (status, location, body)
    }
}

fn query_param(url: &str, name: &str) -> String {
    let query = url.split_once('?').unwrap().1;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap()
}

#[tokio::test]
async fn signs_in_keeps_the_session_and_renews_it() {
    let (service_url, service) = start_service().await;
    let mut browser = Browser {
        router: app(&service_url),
        cookies: HashMap::new(),
    };

    // Not signed in: a page sends the browser to sign in; an API client gets 401.
    let (status, location, _) = browser.get("/me", &[("accept", "text/html")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, "/_custard/auth/login?return_to=%2Fme");
    let (status, _, _) = browser.get("/me", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Signing in starts at the service, with PKCE and the contract's scopes.
    let (status, authorize, _) = browser.get(&location, &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(authorize.starts_with(&format!("{ISSUER}/oauth/v2/authorize?")));
    assert_eq!(query_param(&authorize, "client_id"), CLIENT_ID);
    assert_eq!(query_param(&authorize, "code_challenge_method"), "S256");
    assert!(query_param(&authorize, "scope").contains("urn:zitadel:iam:org:project:roles"));
    assert!(browser.cookies.contains_key("custard_flow"));
    *service.nonce.lock().unwrap() = Some(query_param(&authorize, "nonce"));
    let state = query_param(&authorize, "state");

    // A callback with another state is refused.
    let mut stranger = Browser {
        router: browser.router.clone(),
        cookies: browser.cookies.clone(),
    };
    let (status, _, _) = stranger
        .get("/_custard/auth/callback?code=the-code&state=forged", &[])
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The service sends the browser back; the code is exchanged and checked.
    let (status, location, body) = browser
        .get(
            &format!("/_custard/auth/callback?code=the-code&state={state}"),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(location, "/me");
    assert!(browser.cookies.contains_key("custard_session"));
    assert!(!browser.cookies.contains_key("custard_flow"));

    // Signed in, from the session cookie.
    let (status, _, body) = browser.get("/me", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "user-1 ana@example.com Acme admin=true");

    // The app reached the service at CUSTARD_AUTH_URL, for the issuer's host.
    assert!(
        service
            .hosts
            .lock()
            .unwrap()
            .iter()
            .all(|host| host == "auth--test.example.com")
    );

    // Signing in again with a session carries it on with the refresh token,
    // without a trip to the service's pages.
    let (status, location, _) = browser.get("/_custard/auth/login?return_to=/me", &[]).await;
    assert_eq!((status, location.as_str()), (StatusCode::SEE_OTHER, "/me"));
    assert_eq!(service.refreshes.load(Ordering::SeqCst), 1);

    // Signing out ends the session here and at the service.
    let (status, location, _) = browser.get("/_custard/auth/logout", &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location.starts_with(&format!("{ISSUER}/oidc/v1/end_session?")));
    assert!(query_param(&location, "id_token_hint").len() > 100);
    assert!(!browser.cookies.contains_key("custard_session"));
    let (status, _, _) = browser.get("/me", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn accepts_bearer_tokens_for_this_app_only() {
    let (service_url, _) = start_service().await;
    let mut client = Browser {
        router: app(&service_url),
        cookies: HashMap::new(),
    };

    let good = format!("Bearer {}", id_token(CLIENT_ID, None));
    let (status, _, body) = client.get("/me", &[("authorization", &good)]).await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "user-1 ana@example.com Acme admin=true")
    );

    let other_app = format!("Bearer {}", id_token("another-client", None));
    let (status, _, _) = client.get("/me", &[("authorization", &other_app)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let expired =
        sign(json!({ "iss": ISSUER, "aud": CLIENT_ID, "sub": "user-1", "exp": now() - 3600 }));
    let (status, _, _) = client
        .get("/me", &[("authorization", &format!("Bearer {expired}"))])
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let other_issuer = sign(
        json!({ "iss": "https://evil.example", "aud": CLIENT_ID, "sub": "x", "exp": now() + 900 }),
    );
    let (status, _, _) = client
        .get(
            "/me",
            &[("authorization", &format!("Bearer {other_issuer}"))],
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mut tampered = id_token(CLIENT_ID, None);
    tampered.replace_range(tampered.len() - 6.., "AAAAAA");
    let (status, _, _) = client
        .get("/me", &[("authorization", &format!("Bearer {tampered}"))])
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
