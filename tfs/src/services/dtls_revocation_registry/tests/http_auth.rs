use super::*;
use super::loopback::open_loopback_map;
use crate::common::authenticated_user::AuthenticatedUser;
use crate::controllers::auth::{CaAccessTokenVerifier, ManageRoles, RequiredUser, SessionUser};
use crate::controllers::{RawDhtAccess, RoleAccess};
use crate::test_support::sign_ps256;
use actix_web::http::header::HeaderValue;
use jwt::SignWithKey;

const TEST_SIGNING_KEY: &str = "http-auth-fixture-key";

pub(super) async fn auth_fixture() -> (Arc<GlobalConfig>, String, RevocationAdmin) {
    let (dht, storage, _) = open_loopback_map().await;
    dht.put(SIGNING_KEY_PATH, &TEST_SIGNING_KEY.to_string()).await.unwrap();
    let config = Arc::new(GlobalConfig::new(dht, Vec::new()).await);
    let token = generate_token(42, u64::MAX, TokenAction::Access, TEST_SIGNING_KEY.as_bytes());
    (config, format!("Bearer {token}"), storage)
}

pub(super) fn with_authorization(
    request: actix_test::TestRequest,
    header: Option<&str>,
) -> actix_test::TestRequest {
    match header {
        Some(header) => request.insert_header(("Authorization", header)),
        None => request,
    }
}

async fn optional_uid(user: AuthenticatedUser) -> web::Json<u64> {
    web::Json(user.uid)
}

async fn required_uid(user: RequiredUser) -> web::Json<u64> {
    web::Json(user.uid)
}

async fn role_allowed(_: RoleAccess<RawDhtAccess>) -> actix_web::HttpResponse {
    actix_web::HttpResponse::Ok().finish()
}

async fn session_uid(user: SessionUser) -> web::Json<u64> {
    web::Json(user.uid)
}

async fn grant_allowed(_: RoleAccess<ManageRoles>) -> actix_web::HttpResponse {
    actix_web::HttpResponse::Ok().finish()
}

fn auth_routes(config: &mut web::ServiceConfig) {
    config.route("/optional", web::get().to(optional_uid))
        .route("/required", web::get().to(required_uid))
        .route("/session", web::get().to(session_uid))
        .route("/grant", web::get().to(grant_allowed))
        .route("/role", web::get().to(role_allowed));
}

#[actix_web::test]
async fn live_auth_routes_enforce_identity_when_http_requests_arrive() {
    let (dht, _storage, _) = open_loopback_map().await;
    let config = Arc::new(GlobalConfig::new(dht.clone(), Vec::new()).await);
    let token = generate_token(42, u64::MAX, TokenAction::Access, TEST_SIGNING_KEY.as_bytes());
    let token = format!("Bearer {token}");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = actix_web::HttpServer::new(move || App::new().app_data(web::Data::new(config.clone())).configure(auth_routes))
        .workers(1).disable_signals().listen(listener).unwrap().run();
    let handle = server.handle();
    let task = actix_web::rt::spawn(server);
    let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap();
    let optional = format!("http://{address}/optional");
    for value in [None, Some(" \t")] {
        if let Some(value) = value {
            dht.put(SIGNING_KEY_PATH, &value.to_string()).await.unwrap();
        }
        let response = client.get(&optional).header("Authorization", &token).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 503, "DHT signing key: {value:?}");
    }
    let response = client.get(&optional).send().await.unwrap();
    assert_eq!(response.status().as_u16(), StatusCode::OK.as_u16());
    assert_eq!(response.json::<u64>().await.unwrap(), 2);
    dht.put(SIGNING_KEY_PATH, &TEST_SIGNING_KEY.to_string()).await.unwrap();
    for (uri, header, expected, uid) in [
        ("/optional", Some(token.as_str()), StatusCode::OK, Some(42)),
        ("/required", Some(token.as_str()), StatusCode::OK, Some(42)),
        ("/required", None, StatusCode::UNAUTHORIZED, None),
        ("/role", None, StatusCode::UNAUTHORIZED, None),
        ("/optional", Some("Bearer invalid"), StatusCode::UNAUTHORIZED, None),
    ] {
        let mut request = client.get(format!("http://{address}{uri}"));
        if let Some(header) = header {
            request = request.header("Authorization", header);
        }

        let response = request.send().await.unwrap();

        assert_eq!(response.status().as_u16(), expected.as_u16(), "{uri}");
        if let Some(uid) = uid {
            assert_eq!(response.json::<u64>().await.unwrap(), uid, "{uri}");
        }
    }
    handle.stop(true).await;
    task.await.unwrap().unwrap();
}

#[actix_web::test]
async fn extractors_reject_invalid_or_ambiguous_authorization() {
    let (config, valid, _storage) = auth_fixture().await;
    let valid = HeaderValue::from_str(&valid).unwrap();
    let expired = generate_token(42, 0, TokenAction::Access, TEST_SIGNING_KEY.as_bytes());
    let refresh = generate_token(42, u64::MAX, TokenAction::Refresh, TEST_SIGNING_KEY.as_bytes());
    let wrong_signature = generate_token(42, u64::MAX, TokenAction::Access, b"different-secret");
    let raw = generate_token(42, u64::MAX, TokenAction::Access, TEST_SIGNING_KEY.as_bytes());
    let headers = [
        HeaderValue::from_static(""),
        HeaderValue::from_static("Bearer"),
        HeaderValue::from_static("Bearer "),
        HeaderValue::from_static("Basic credentials"),
        HeaderValue::from_static("Bearer invalid-jwt"),
        HeaderValue::from_bytes(b"Bearer \xff").unwrap(),
        HeaderValue::from_str(&raw).unwrap(),
        HeaderValue::from_str(&format!("Bearer{raw}")).unwrap(),
        HeaderValue::from_str(&format!("Bearer {wrong_signature}")).unwrap(),
        HeaderValue::from_str(&format!("Bearer {expired}")).unwrap(),
        HeaderValue::from_str(&format!("Bearer {refresh}")).unwrap(),
    ];
    let app = actix_test::init_service(App::new().app_data(web::Data::new(config)).configure(auth_routes)).await;
    for uri in ["/optional", "/required", "/role", "/session"] {
        let cases = headers.iter().map(|header| (header, None)).chain([(&valid, Some(&valid))]);
        for (case, (header, second)) in cases.enumerate() {
            let mut request = actix_test::TestRequest::get().uri(uri).insert_header(("Authorization", header.clone()));
            if let Some(second) = second {
                request = request.append_header(("Authorization", second.clone()));
            }

            let response = actix_test::call_service(&app, request.to_request()).await;

            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}: case {case}");
        }
    }
}

fn ca_bearer(alg: &str, claims: &serde_json::Value, key: &PKey<Private>) -> String {
    let header = serde_json::json!({ "alg": alg, "typ": "JWT" });
    format!("Bearer {}", sign_ps256(&header.to_string(), claims, key))
}

#[actix_web::test]
async fn ca_access_resolves_identity_and_grants_roles_without_stored_roles() {
    let (config, session, _storage) = auth_fixture().await;
    let token = ca_bearer("PS256", &serde_json::json!({
        "uid": 71, "additional_roles": ["roles:management"], "exp": u64::MAX,
    }), &ca().key);
    let app = actix_test::init_service(App::new()
        .app_data(web::Data::new(config))
        .app_data(web::Data::new(CaAccessTokenVerifier::from_root_pem(&ca().pem).unwrap()))
        .configure(auth_routes)).await;

    for (uri, header, expected, uid) in [
        ("/optional", Some(token.as_str()), StatusCode::OK, Some(71)),
        ("/required", Some(token.as_str()), StatusCode::OK, Some(71)),
        ("/grant", Some(token.as_str()), StatusCode::OK, None),
        ("/role", Some(token.as_str()), StatusCode::SERVICE_UNAVAILABLE, None),
        ("/session", Some(token.as_str()), StatusCode::UNAUTHORIZED, None),
        ("/session", None, StatusCode::UNAUTHORIZED, None),
        ("/session", Some(session.as_str()), StatusCode::OK, Some(42)),
    ] {
        let request = with_authorization(actix_test::TestRequest::get().uri(uri), header);
        let response = actix_test::call_service(&app, request.to_request()).await;
        assert_eq!(response.status(), expected, "{uri}");
        if let Some(uid) = uid {
            assert_eq!(actix_test::read_body_json::<u64, _>(response).await, uid, "{uri}");
        }
    }
}

#[actix_web::test]
async fn ca_access_rejects_invalid_authority_algorithm_expiration_and_claims() {
    let other_key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let valid = serde_json::json!({ "uid": 71, "additional_roles": ["roles:management"], "exp": u64::MAX });
    let app = actix_test::init_service(App::new()
        .app_data(web::Data::new(CaAccessTokenVerifier::from_root_pem(&ca().pem).unwrap()))
        .configure(auth_routes)).await;

    for (case, alg, claims, key) in [
        ("wrong authority", "PS256", valid.clone(), &other_key),
        ("wrong algorithm", "RS256", valid, &ca().key),
        ("expired", "PS256", serde_json::json!({ "uid": 71, "additional_roles": [], "exp": 0 }), &ca().key),
        ("unexpected claim", "PS256", serde_json::json!({ "uid": 71, "additional_roles": [], "exp": u64::MAX, "extra": true }), &ca().key),
        ("missing role list", "PS256", serde_json::json!({ "uid": 71, "exp": u64::MAX }), &ca().key),
        ("non-object claims", "PS256", serde_json::json!([71, [], u64::MAX]), &ca().key),
    ] {
        let token = ca_bearer(alg, &claims, key);
        let request = with_authorization(actix_test::TestRequest::get().uri("/grant"), Some(&token));
        let response = actix_test::call_service(&app, request.to_request()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{case}");
    }
}

#[actix_web::test]
async fn session_access_ignores_claimed_roles() {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let (config, _, _storage) = auth_fixture().await;
    let secret = Hmac::<Sha256>::new_from_slice(TEST_SIGNING_KEY.as_bytes()).unwrap();
    let token = serde_json::json!({
        "uid": 42, "expires": u64::MAX, "token_action": "Access", "additional_roles": ["roles:management"],
    }).sign_with_key(&secret).unwrap();
    let roles: Arc<dyn RoleAuthorizer> = Arc::new(TestRoleAuthorizer(Ok(Vec::new())));
    let app = actix_test::init_service(App::new().app_data(web::Data::new(config))
        .app_data(web::Data::new(roles)).configure(auth_routes)).await;
    let request = with_authorization(actix_test::TestRequest::get().uri("/grant"), Some(&format!("Bearer {token}")));

    let response = actix_test::call_service(&app, request.to_request()).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[actix_web::test]
async fn role_access_checks_authorization_when_access_token_is_valid() {
    let (config, token, _storage) = auth_fixture().await;
    for (roles, expected) in [
        (None, StatusCode::SERVICE_UNAVAILABLE),
        (Some(Ok(Vec::new())), StatusCode::FORBIDDEN),
        (Some(Ok(vec![Role::RevocationRegistryPublisher])), StatusCode::FORBIDDEN),
        (Some(Err("lookup unavailable".into())), StatusCode::SERVICE_UNAVAILABLE),
        (Some(Ok(vec![Role::RawDhtAccess])), StatusCode::OK),
    ] {
        let mut app = App::new().app_data(web::Data::new(config.clone())).configure(auth_routes);
        if let Some(roles) = roles {
            app = app.app_data(web::Data::new(Arc::new(TestRoleAuthorizer(roles)) as Arc<dyn RoleAuthorizer>));
        }
        let app = actix_test::init_service(app).await;
        let request = with_authorization(actix_test::TestRequest::get().uri("/role"), Some(&token));

        let response = actix_test::call_service(&app, request.to_request()).await;

        assert_eq!(response.status(), expected);
    }
}
