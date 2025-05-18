use std::sync::Arc;
use actix_web::{delete, get, post, put, web, HttpResponse, Responder, Scope};
use actix_web::web::Json;
use crate::common::authenticated_user::AuthenticatedUser;
use crate::common::exceptions::ApiException;
use crate::common::models::{ObjType, PermissionType};
use crate::controllers::virtual_fs_controller::virtual_fs_controller::{create_file, create_folder, get_file, get_file_meta, get_folder, get_stored_parts, save_existing_chunk};
use crate::services::file_storage::model::FolderMeta;
use crate::services::permission_service::permission_service::PermissionService;
use crate::services::user_permission_service::model::PermissionE;
use crate::services::user_permission_service::user_permission_service::UserPermissionService;
use crate::services::virtual_fs::virtual_fs::VirtualFS;

#[get("/user/current_user/{obj_type}/{obj_id}")]
async fn get_user_permissions(user: AuthenticatedUser, path: web::Path<(ObjType, u64)>,
                         permission_service: web::Data<Arc<PermissionService>>) -> Result<Json<Vec<PermissionType>>, ApiException> {
    let obj_type = path.0;
    let obj_id = path.1;

    let permissions = permission_service.get_ref().get_permissions(user.uid, obj_type, obj_id).await?;
    Ok(Json(permissions))
}

#[get("/user/all/{obj_type}/{obj_id}")]
async fn get_permissions_of_all_users(user: AuthenticatedUser, path: web::Path<(ObjType, u64)>,
                                      permission_service: web::Data<Arc<UserPermissionService>>) -> Result<Json<Vec<PermissionE>>, ApiException> {
    let obj_type = path.0;
    let obj_id = path.1;

    let permissions = permission_service.get_ref().get_permissions_of_all_users(user.uid, obj_type, obj_id).await?;
    Ok(Json(permissions))
}

#[put("/user/{target_user}/{obj_type}/{obj_id}/{permission_type}")]
async fn put_permission(user: AuthenticatedUser, path: web::Path<(String, ObjType, u64, PermissionType)>,
                        permission_service: web::Data<Arc<PermissionService>>) -> Result<HttpResponse, ApiException> {
    let target_user = path.0.clone();
    let obj_type = path.1;
    let obj_id = path.2;
    let permission_type = path.3;

    permission_service.get_ref().put_permission(user.uid, target_user,
                                                obj_type, obj_id, permission_type, None).await?;
    Ok(HttpResponse::Ok().finish())
}

#[delete("/user/{target_user}/{obj_type}/{obj_id}/{permission_type}")]
async fn delete_permission(user: AuthenticatedUser, path: web::Path<(String, ObjType, u64, PermissionType)>,
                           permission_service: web::Data<Arc<PermissionService>>) -> Result<HttpResponse, ApiException> {

    let target_user = path.0.clone();
    let obj_type = path.1;
    let obj_id = path.2;
    let permission_type = path.3;

    permission_service.get_ref().delete_permission(user.uid, target_user,
                                                obj_type, obj_id, permission_type, None).await?;
    Ok(HttpResponse::Ok().finish())
}

pub fn config() -> Scope {
    web::scope("permission")
        .service(get_user_permissions)
        .service(get_permissions_of_all_users)
        .service(put_permission)
        .service(delete_permission)
}