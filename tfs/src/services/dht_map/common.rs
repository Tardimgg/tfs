pub struct DhtPage(Vec<String>);

impl From<String> for DhtPage {
    fn from(value: String) -> Self {
        todo!()
    }
}

pub struct ListOfIP(Vec<String>);


impl From<DhtPage> for ListOfIP {
    fn from(value: DhtPage) -> Self {
        todo!()
    }
}

pub struct TfsData(String);

impl From<ListOfIP> for TfsData {
    fn from(value: ListOfIP) -> Self {
        todo!()
    }
}