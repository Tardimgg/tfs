use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

#[derive(Serialize, Deserialize, TypedBuilder)]
pub struct FileMeta {
    name: String
}


#[derive(Serialize, Deserialize, TypedBuilder)]
pub struct FolderMeta {

    // #[builder(default=vec![])]
    files: Vec<FileMeta>
}

