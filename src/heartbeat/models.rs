use crate::services::file_storage::model::EndOfFileRange;

pub enum RangeEvent {
    Start(u64),
    End(u64),
    StoredStart(u64, usize),
    StoredEnd(EndOfFileRange, usize)
}

impl RangeEvent {

    pub fn get_index(&self) -> (u64, u8) {
        match self {
            RangeEvent::Start(v) => (*v, 0),
            RangeEvent::End(v) => (*v, 3),
            RangeEvent::StoredStart(v, _) => (*v, 1),
            RangeEvent::StoredEnd(v, _) => {
                match v {
                    EndOfFileRange::LastByte => (u64::MAX, 2),
                    EndOfFileRange::ByteIndex(v) => (*v, 2)
                }
            }
        }
    }
}