use super::*;
use crate::common::common::generate_token;
use crate::common::models::TokenAction;
use crate::config::global_config::SIGNING_KEY_PATH;
use crate::controllers::auth::CaAccessTokenVerifier;
use crate::controllers::user_controller::user_controller;
use crate::services::dht_map::model::PrevSeq;
use crate::services::file_storage::file_storage::FileStorage;
use crate::services::permission_service::permission_service::PermissionService;
use crate::services::user_service::user_service::{RoleAuthorizer, UserService};
use crate::test_support::sign_ps256;
use actix_web::{http::StatusCode, test, web, App};
use openssl::{pkey::PKey, rsa::Rsa};
use serde_json::json;
use std::sync::Arc;

mod support;

#[actix_web::test]
async fn http_grant_makes_an_existing_session_a_persistent_grantor() {
    let mut fixture = support::Fixture::new(false).await;
    let secret = "role-grant-fixture-session-key".to_string();
    fixture.map.put_with_seq(SIGNING_KEY_PATH, &secret, PrevSeq::Seq(None)).await.unwrap();
    fixture.vfs.lock_user_uid_mapping(42, "alice").await.unwrap();
    let permissions = PermissionService::new(fixture.map.clone(), fixture.vfs.clone(), fixture.config.clone()).await;
    let users = Arc::new(UserService::builder()
        .virtual_fs(fixture.vfs.clone())
        .permission_service(Arc::new(permissions))
        .config(fixture.config.clone())
        .build());
    let authorizer: Arc<dyn RoleAuthorizer> = users.clone();
    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let root = support::test_root(&key).to_pem().unwrap();
    let app = test::init_service(App::new()
        .app_data(web::Data::new(fixture.config.clone()))
        .app_data(web::Data::new(users))
        .app_data(web::Data::new(authorizer))
        .app_data(web::Data::new(CaAccessTokenVerifier::from_root_pem(&root).unwrap()))
        .service(user_controller::config())).await;

    let session = generate_token(42, u64::MAX, TokenAction::Access, secret.as_bytes());
    let ca_token = |roles: &[&str]| {
        let claims = json!({"uid": 42, "additional_roles": roles, "exp": u64::MAX});
        sign_ps256(r#"{"alg":"PS256"}"#, &claims, &key)
    };
    let ca_without_grant = ca_token(&["RawDhtAccess"]);
    let ca_with_grant = ca_token(&["roles:management"]);

    let grant_request = |uid, token: &str, role: &str| {
        test::TestRequest::post()
            .uri(&format!("/user/{uid}/roles"))
            .insert_header(("Authorization", format!("Bearer {token}")))
            .set_json(json!({"roles": [role]}))
            .to_request()
    };
    let read_metadata = || fixture.map.get_strict::<GlobalFileInfo>("users/42");
    let mut snapshot = read_metadata().await.unwrap().unwrap();
    // Each actual grant reserves and publishes one version, advancing the DHT sequence twice.
    for (uid, token, role, status, commit) in [
        (42, &session, "roles:management", StatusCode::FORBIDDEN, None),
        (42, &ca_without_grant, "roles:management", StatusCode::FORBIDDEN, None),
        (43, &ca_with_grant, "roles:management", StatusCode::NOT_FOUND, None),
        (42, &ca_with_grant, "roles:management", StatusCode::NO_CONTENT, Some((2, 1))),
        (42, &session, "RevocationRegistryPublisher", StatusCode::NO_CONTENT, Some((4, 2))),
    ] {
        let response = test::call_service(&app, grant_request(uid, token, role)).await;
        assert_eq!(response.status(), status, "grant {role} to {uid}");
        if let Some((sequence, version)) = commit {
            snapshot = read_metadata().await.unwrap().unwrap();
            assert_eq!(snapshot.1, sequence, "DHT sequence after granting {role}");
            assert_eq!(committed_user_version("users/42", &snapshot.0).unwrap(), version);
            assert!(matches!(fixture.notifications.try_recv(), Ok(StateUpdate::NewUser(path)) if path == "users/42"));
        }
        assert!(fixture.notifications.try_recv().is_err(), "unexpected notification after requesting {role}");
    }

    let version = committed_user_version("users/42", &snapshot.0).unwrap();
    let user = fixture.vfs.read_committed_user("users/42", &snapshot.0, version).await.unwrap();
    assert_eq!(user, UserInfo {
        uid: 42,
        login: "alice".into(),
        hashed_password: "original-hash".into(),
        roles: vec![Role::RawDhtAccess, Role::ManageRoles, Role::RevocationRegistryPublisher],
    });

    let assert_metadata_unchanged = || async {
        assert_eq!(read_metadata().await.unwrap().as_ref(), Some(&snapshot));
    };
    let blob_count = fixture.local.get_file_meta("users/42").await.unwrap().len();
    assert_eq!(blob_count, 3);
    let repeat = grant_request(42, &session, "RevocationRegistryPublisher");
    assert_eq!(test::call_service(&app, repeat).await.status(), StatusCode::NO_CONTENT);
    assert_metadata_unchanged().await;
    assert_eq!(fixture.local.get_file_meta("users/42").await.unwrap().len(), blob_count);
    assert!(fixture.notifications.try_recv().is_err());
    assert!(matches!(
        fixture.vfs.add_user_roles("users/42", 43, "alice", &[]).await,
        Err(RoleGrantError::IdentityMismatch)
    ));

    fixture.local.delete_chunk("users/42", ChunkVersion(version)).await.unwrap();
    for token in [&session, &ca_with_grant] {
        let request = grant_request(42, token, "roles:management");
        assert_eq!(test::call_service(&app, request).await.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
    assert_metadata_unchanged().await;
}

#[tokio::test]
async fn role_lookup_requires_a_local_keeper_of_the_selected_version() {
    let fixture = support::Fixture::new(false).await;
    let (metadata, sequence) = fixture.map.get_strict::<GlobalFileInfo>("users/42").await.unwrap().unwrap();
    let mut remote_latest = metadata.clone();
    let mut remote_id = fixture.vfs.id;
    remote_id.port += 1;
    remote_latest.keepers.push(FileKeeper {
        id: remote_id,
        data: vec![StoredFile { version: 1, is_keeper: true }],
    });
    fixture.map.compare_exchange_strict("users/42", (&metadata, sequence), &remote_latest).await.unwrap();

    assert!(matches!(fixture.vfs.user_for_role_check("users/42").await,
        Err(RoleGrantError::LocalVersionUnavailable(1))));
}

#[tokio::test]
async fn denies_success_and_retains_referenced_blob_when_publication_readback_fails() {
    let mut fixture = support::Fixture::new(true).await;

    let result = fixture
        .vfs
        .add_user_roles(
            "users/42",
            42,
            "alice",
            &[Role::RevocationRegistryPublisher],
        )
        .await;

    assert!(matches!(result, Err(RoleGrantError::Read(_))));
    let (published, sequence) = fixture
        .map
        .get_strict::<GlobalFileInfo>("users/42")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sequence, 2);
    assert_eq!(committed_user_version("users/42", &published).unwrap(), 1);
    assert!(fixture
        .local
        .get_file("users/42", None, Some(ChunkVersion(1)))
        .await
        .is_ok());
    assert!(fixture.notifications.try_recv().is_err());
}

#[tokio::test]
async fn role_lookup_rejects_uncommitted_user_version() {
    let fixture = support::Fixture::new(false).await;
    let (metadata, sequence) = fixture.map.get_strict::<GlobalFileInfo>("users/42").await.unwrap().unwrap();
    let mut reserved = metadata.clone();
    reserved.keepers[0].data.push(StoredFile { version: 1, is_keeper: false });
    fixture.map.compare_exchange_strict("users/42", (&metadata, sequence), &reserved).await.unwrap();
    let blob = serde_json::to_string(&FsNode::UserInfo(UserInfo {
        uid: 42,
        login: "alice".into(),
        hashed_password: "original-hash".into(),
        roles: vec![Role::RawDhtAccess, Role::RevocationRegistryPublisher],
    })).unwrap();
    fixture.local.save_chunk("users/42", FileStream::StringData(Some(blob)),
        FileSaveOptions::builder().version(ChunkVersion(1)).build()).await.unwrap();

    assert!(matches!(fixture.vfs.user_for_role_check("users/42").await, Err(RoleGrantError::InvalidMetadata)));
}
