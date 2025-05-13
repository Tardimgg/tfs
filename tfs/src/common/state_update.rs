pub enum StateUpdate {
    NewFile(String),
    NewFolder(String),
    NewDataMeta(String),
    NewData(String),
    NewRebac(String),
    NewUser(String)
}