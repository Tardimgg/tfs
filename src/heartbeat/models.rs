use crate::common::file_range::EndOfFileRange;
use crate::services::virtual_fs::models::StoredFileRangeEnd;

#[derive(Copy, Clone)]
pub enum RangeEvent {
    Start(u64),
    End(u64),
    StoredStart(u64, usize),
    StoredEnd(StoredFileRangeEnd, usize)
}

impl RangeEvent {

    pub fn get_index(&self) -> (u64, u8) {
        match self {
            RangeEvent::Start(v) => (*v, 0),
            RangeEvent::End(v) => (*v, 3),
            RangeEvent::StoredStart(v, _) => (*v, 1),
            RangeEvent::StoredEnd(v, _) => {
                match v {
                    StoredFileRangeEnd::EnfOfFile(v) => (*v, 2),
                    StoredFileRangeEnd::EndOfRange(v) => (*v, 2)
                }
            }
        }
    }
}