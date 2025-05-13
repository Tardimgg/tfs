use std::sync::Arc;
use futures::future::join_all;
use itertools::Itertools;
use tokio_stream::StreamExt;
use typed_builder::TypedBuilder;
use crate::common::models::ObjType;
use crate::services::permission_service::errors::ReadingUserPermissionsToObjectError;
use crate::services::permission_service::permission_service::PermissionService;
use crate::services::rebac::models::Permission;
use crate::services::user_permission_service::model::PermissionE;
use crate::services::user_service::user_service::UserService;

#[derive(TypedBuilder)]
pub struct UserPermissionService {
    user_service: Arc<UserService>,
    permission_service: Arc<PermissionService>
}

impl UserPermissionService {

    pub async fn get_permissions_of_all_users(&self, user_id: u64, obj_type: ObjType,
                                              obj_id: u64) -> Result<Vec<PermissionE>, ReadingUserPermissionsToObjectError> {

        let permissions = self.permission_service.get_permissions_of_all_users(user_id, obj_type, obj_id)
            .await?;

        let permissions_external_futures: Vec<_> = permissions.into_iter().map(async |permission| {
            let user_r = self.user_service.user_by_id(permission.subject).await;
            if let Ok(user) = user_r {
                if let Some(some_user) = user {
                    Ok(Some(
                        PermissionE {
                            login: some_user.login,
                            permission_type: permission.permission_type
                        }
                    ))
                } else {
                    Err(format!("user was not found, uid: {}", permission.subject))
                }
            } else {
                Err(user_r.err().unwrap())
            }
        }).collect();

        let permissions_external = join_all(permissions_external_futures).await;

        if permissions_external.iter().any(Result::is_err) {
            let errs = permissions_external.into_iter()
                .filter(Result::is_err)
                .map(|v| v.err().unwrap())
                .join(", ");

            return Err(ReadingUserPermissionsToObjectError::InternalError(errs));
        }

        let filtered_permissions_external = permissions_external.into_iter()
            .map(Result::unwrap)
            .filter(Option::is_some)
            .map(Option::unwrap)
            .collect();

        Ok(filtered_permissions_external)
    }
}

