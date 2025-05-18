use thiserror::Error;
use typed_builder::TypedBuilder;
use crate::common::file_range::{EndOfFileRange, FileRange};
use crate::services::file_storage::model::{ChunkVersion, ChunkVersionParseError};


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

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct ChunkFilename {
    pub name: String,
    pub version: ChunkVersion,
}

impl ChunkFilename {
    pub fn from(filename: &str, version: ChunkVersion) -> Self {
        ChunkFilename {
            name: filename.to_string(),
            version
        }
    }
}

impl From<ChunkFilename> for String {
    fn from(value: ChunkFilename) -> Self {
        format!("{}_v{}", value.name, value.version.0)
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

        Ok(ChunkFilename::from(value, version))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_chunk_filename() {
        assert_eq!(0, 0);
    }
}