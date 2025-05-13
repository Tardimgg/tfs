use std::cmp::Ordering;

#[derive(Copy, Clone, Debug, Hash)]
pub enum EndOfFileRange {
    LastByte,
    ByteIndex(u64)
}

impl Eq for EndOfFileRange {}

impl PartialEq<Self> for EndOfFileRange {
    fn eq(&self, other: &Self) -> bool {
        self.partial_cmp(other).unwrap().is_eq()
    }
}

impl PartialOrd<Self> for EndOfFileRange {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match self {
            EndOfFileRange::LastByte => {
                if let EndOfFileRange::LastByte = other {
                    Some(Ordering::Equal)
                } else {
                    Some(Ordering::Greater)
                }
            }
            EndOfFileRange::ByteIndex(end) => {
                if let EndOfFileRange::ByteIndex(other_end) = other {
                    Some(end.cmp(other_end))
                } else {
                    Some(Ordering::Less)
                }
            }
        }
    }
}

impl Ord for EndOfFileRange {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap()
    }
}

pub type FileRange = (u64, EndOfFileRange);
