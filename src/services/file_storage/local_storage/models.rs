use thiserror::Error;
use crate::services::file_storage::model::{ChunkVersion, ChunkVersionParseError, EndOfFileRange, FileRange};


/*
      events.sort_unstable_by(|mut f, mut s| {
            let f_matches = matches!(f, Event::EndExist(_, _, _));
            let s_matches = matches!(s, Event::EndExist(_, _, _));
            if !f_matches && !s_matches {
                get_sort_key(f).cmp(&get_sort_key(s))
            } else {
                let with_rev = s_matches;
                let (mut f, mut s) = (f, s);
                if with_rev {
                    (f, s) = (s, f);
                }
                if let Event::EndExist(f_index, f_version, _) = f {
                    let res = match s {
                        Event::StartExist(index, version, _) => {
                            (f_version, cast_end_of_chunk_to_index(f_index)).cmp(&(version, index))
                                .then(Ordering::Less)
                        },
                        Event::EndExist(index, version, _) => {
                            let version_cmp = f_version.cmp(version).reverse();
                            f_index.cmp(&index).then(version_cmp)
                        },
                        Event::StartRequest(from) => {
                            cast_end_of_chunk_to_index(f_index).cmp(&from).then(Ordering::Greater)
                        }
                        Event::EndRequest(to) => {
                            cast_end_of_chunk_to_index(f_index).cmp(&cast_end_of_chunk_to_index(to)).then(Ordering::Greater)
                        }
                    };

                    if with_rev {
                        res.reverse()
                    } else {
                        res
                    }
                } else {
                    panic!("internal logic error");
                }
            }
        });

 */

#[derive(Debug)]
pub enum Event {
    StartExist(u64, ChunkVersion, ChunkFilename),
    EndExist(EndOfFileRange, ChunkVersion, ChunkFilename),
    StartRequest(u64),
    EndRequest(EndOfFileRange),
}

#[derive(Debug, Ord, PartialOrd, PartialEq, Eq, Copy, Clone, Hash)]
pub enum ChunkFilename {
    All(ChunkVersion),
    Range(u64, EndOfFileRange, ChunkVersion)
}

impl ChunkFilename {
    pub fn from(range: FileRange, version: ChunkVersion) -> Self {
        match range.1 {
            EndOfFileRange::LastByte => {
                if range.0 == 0 {
                    ChunkFilename::All(version)
                } else {
                    ChunkFilename::Range(range.0, EndOfFileRange::LastByte, version)
                }
            }
            EndOfFileRange::ByteIndex(last_byte) => ChunkFilename::Range(range.0, EndOfFileRange::ByteIndex(last_byte), version)
        }
    }

    pub fn get_version(&self) -> &ChunkVersion {
        match self {
            ChunkFilename::All(v) => v,
            ChunkFilename::Range(_, _, v) => v
        }
    }

    pub fn get_start(&self) -> &u64 {
        match self {
            ChunkFilename::All(_) => &0,
            ChunkFilename::Range(from, _, _) => from
        }
    }

    pub fn get_end(&self) -> &EndOfFileRange {
        match self {
            ChunkFilename::All(_) => &EndOfFileRange::LastByte,
            ChunkFilename::Range(_, to, _) => to
        }
    }
}

impl From<ChunkFilename> for String {
    fn from(value: ChunkFilename) -> Self {
        match value {
            ChunkFilename::All(v) => format!("ALL_v{}", v.0),
            ChunkFilename::Range(from, to, v) => {
                match to {
                    EndOfFileRange::ByteIndex(last_byte) => format!("{}_{}_v{}", from, last_byte, v.0),
                    EndOfFileRange::LastByte => format!("{}_END_v{}", from, v.0)
                }
            }
        }

    }
}


#[derive(Error, Debug)]
pub enum ParseChunkFilenameError {
    #[error("Invalid filename format")]
    InvalidFormat,
    #[error("Invalid range start")]
    InvalidRangeStart,
    #[error("Invalid end of range")]
    InvalidEndOfRange,
    #[error("Invalid range format")]
    InvalidRange,
    #[error("Invalid chunk version")]
    InvalidChunkVersion(#[from] ChunkVersionParseError)
}

impl TryFrom<&str> for ChunkFilename {
    type Error = ParseChunkFilenameError;


    fn try_from(value: &str) -> Result<Self, Self::Error> {
        const SEPARATOR: &str = "_";

        let version_index = value.rfind("_v").ok_or(ParseChunkFilenameError::InvalidFormat)?;
        let version = ChunkVersion::try_from(&value[version_index + 2..])?;
        let value = &value[..version_index];

        let value = value.to_uppercase();
        if value == "ALL" {
            Ok(ChunkFilename::All(version))
        } else {
            let first = value.find(SEPARATOR);
            let last = value.rfind(SEPARATOR);
            if first.is_none() || first != last || first.unwrap() == value.len() - 1 {
                return Err(ParseChunkFilenameError::InvalidFormat)
            }

            let separator_index = first.unwrap();
            let first = value[..separator_index].parse::<u64>();
            if let Err(_) = first {
                return Err(ParseChunkFilenameError::InvalidRangeStart);
            }
            let first_byte = first.unwrap();

            let last = value[separator_index + 1..].to_uppercase();
            if last == "END" {
                return Ok(ChunkFilename::Range(first_byte, EndOfFileRange::LastByte, version))
            }
            let last_byte = last.parse::<u64>();
            return if let Ok(last_byte) = last_byte {
                if last_byte >= first_byte {
                    Ok(ChunkFilename::Range(first_byte, EndOfFileRange::ByteIndex(last_byte), version))
                } else {
                    Err(ParseChunkFilenameError::InvalidRange)
                }
            } else {
                Err(ParseChunkFilenameError::InvalidEndOfRange)
            }
        }
    }
}
