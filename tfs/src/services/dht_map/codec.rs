use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use prost::{Enumeration, Message};

use crate::common::models::{ObjType, PermissionType};
use crate::services::dht_map::error::DhtCodecError;
use crate::services::dht_map::model::DhtNodeId;
use crate::services::rebac::models::{Permission, RebacKey};
use crate::services::virtual_fs::models::{FileKeeper, FsNodeType, GlobalFileInfo, StoredFile};

pub(super) const MAX_DHT_VALUE_SIZE: usize = 1000;
const MAX_DECOMPRESSED_RECORD_SIZE: usize = 64 * 1024;
const FORMAT_VERSION: u32 = 1;
const ZSTD_LEVEL: i32 = 3;

// The version stays outside zstd so incompatible formats can be rejected before decoding records.
#[derive(Clone, PartialEq, Message)]
struct DhtRecordEnvelope {
    #[prost(uint32, tag = "1")]
    version: u32,
    #[prost(bytes = "vec", tag = "2")]
    compressed_record: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct DhtRecord {
    #[prost(message, repeated, tag = "1")]
    pub(super) pairs: Vec<DistributedMapVal>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct DistributedMapVal {
    #[prost(string, tag = "1")]
    pub(super) key: String,
    #[prost(oneof = "distributed_map_val::Value", tags = "2, 3, 4, 5, 6")]
    pub(super) value: Option<distributed_map_val::Value>,
}

pub(super) mod distributed_map_val {
    use super::{ProtoDhtNodeList, ProtoGlobalFileInfo, ProtoPermissionList};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub(in crate::services::dht_map) enum Value {
        #[prost(string, tag = "2")]
        Text(String),
        #[prost(uint64, tag = "3")]
        UnsignedInteger(u64),
        #[prost(message, tag = "4")]
        DhtNodeList(ProtoDhtNodeList),
        #[prost(message, tag = "5")]
        GlobalFileInfo(ProtoGlobalFileInfo),
        #[prost(message, tag = "6")]
        PermissionList(ProtoPermissionList),
    }
}

#[derive(Clone, PartialEq, Message)]
struct ProtoDhtNodeId {
    #[prost(bytes = "vec", tag = "1")]
    ip: Vec<u8>,
    #[prost(uint32, tag = "2")]
    port: u32,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct ProtoDhtNodeList {
    #[prost(message, repeated, tag = "1")]
    nodes: Vec<ProtoDhtNodeId>,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoStoredFile {
    #[prost(uint64, tag = "1")]
    version: u64,
    #[prost(bool, tag = "2")]
    is_keeper: bool,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoFileKeeper {
    #[prost(message, optional, tag = "1")]
    id: Option<ProtoDhtNodeId>,
    #[prost(message, repeated, tag = "2")]
    data: Vec<ProtoStoredFile>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct ProtoGlobalFileInfo {
    #[prost(string, tag = "1")]
    filename: String,
    #[prost(enumeration = "ProtoFsNodeType", tag = "2")]
    file_type: i32,
    #[prost(uint64, tag = "3")]
    id: u64,
    #[prost(message, repeated, tag = "4")]
    keepers: Vec<ProtoFileKeeper>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
enum ProtoFsNodeType {
    Unspecified = 0,
    Folder = 1,
    File = 2,
    DataMeta = 3,
    Data = 4,
    Rebac = 5,
    User = 6,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRebacKey {
    #[prost(enumeration = "ProtoObjType", tag = "1")]
    obj_type: i32,
    #[prost(uint64, tag = "2")]
    obj_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
enum ProtoObjType {
    Unspecified = 0,
    File = 1,
    Folder = 2,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoPermission {
    #[prost(uint64, tag = "1")]
    subject: u64,
    #[prost(message, optional, tag = "2")]
    object: Option<ProtoRebacKey>,
    #[prost(enumeration = "ProtoPermissionType", tag = "3")]
    permission_type: i32,
    #[prost(uint64, tag = "4")]
    version: u64,
    #[prost(bool, tag = "5")]
    deleted: bool,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct ProtoPermissionList {
    #[prost(message, repeated, tag = "1")]
    permissions: Vec<ProtoPermission>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
enum ProtoPermissionType {
    Unspecified = 0,
    Read = 1,
    Write = 2,
    ReadRecursively = 3,
    WriteRecursively = 4,
    GrantRead = 5,
    GrantWrite = 6,
    GrantReadRecursively = 7,
    GrantWriteRecursively = 8,
    ReadPermissionsOfOtherUsers = 9,
    ReadPermissionsOfOtherUsersRecursively = 10,
    All = 11,
}

#[derive(Clone, Debug)]
pub(crate) enum DhtValue {
    Text(String),
    UnsignedInteger(u64),
    DhtNodeList(Vec<DhtNodeId>),
    GlobalFileInfo(GlobalFileInfo),
    PermissionList(Vec<Permission>),
}

impl DhtValue {
    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            DhtValue::Text(_) => "String",
            DhtValue::UnsignedInteger(_) => "u64",
            DhtValue::DhtNodeList(_) => "Vec<DhtNodeId>",
            DhtValue::GlobalFileInfo(_) => "GlobalFileInfo",
            DhtValue::PermissionList(_) => "Vec<Permission>",
        }
    }

    pub(super) fn into_proto(self) -> distributed_map_val::Value {
        match self {
            DhtValue::Text(value) => distributed_map_val::Value::Text(value),
            DhtValue::UnsignedInteger(value) => distributed_map_val::Value::UnsignedInteger(value),
            DhtValue::DhtNodeList(nodes) => {
                distributed_map_val::Value::DhtNodeList(ProtoDhtNodeList {
                    nodes: nodes.iter().map(ProtoDhtNodeId::from).collect(),
                })
            }
            DhtValue::GlobalFileInfo(info) => {
                distributed_map_val::Value::GlobalFileInfo(ProtoGlobalFileInfo::from(&info))
            }
            DhtValue::PermissionList(permissions) => {
                distributed_map_val::Value::PermissionList(ProtoPermissionList {
                    permissions: permissions.iter().map(ProtoPermission::from).collect(),
                })
            }
        }
    }

    pub(super) fn from_proto(value: distributed_map_val::Value) -> Result<Self, DhtCodecError> {
        match value {
            distributed_map_val::Value::Text(value) => Ok(DhtValue::Text(value)),
            distributed_map_val::Value::UnsignedInteger(value) => {
                Ok(DhtValue::UnsignedInteger(value))
            }
            distributed_map_val::Value::DhtNodeList(list) => Ok(DhtValue::DhtNodeList(
                list.nodes
                    .into_iter()
                    .map(DhtNodeId::try_from)
                    .collect::<Result<_, _>>()?,
            )),
            distributed_map_val::Value::GlobalFileInfo(info) => {
                Ok(DhtValue::GlobalFileInfo(GlobalFileInfo::try_from(info)?))
            }
            distributed_map_val::Value::PermissionList(list) => Ok(DhtValue::PermissionList(
                list.permissions
                    .into_iter()
                    .map(Permission::try_from)
                    .collect::<Result<_, _>>()?,
            )),
        }
    }
}

pub(crate) trait DhtMapValue: Sized + Sync {
    const TYPE_NAME: &'static str;

    fn to_dht_value(&self) -> DhtValue;
    fn from_dht_value(value: DhtValue) -> Result<Self, DhtCodecError>;
}

macro_rules! impl_dht_value {
    ($type:ty, $variant:ident, $name:literal) => {
        impl DhtMapValue for $type {
            const TYPE_NAME: &'static str = $name;

            fn to_dht_value(&self) -> DhtValue {
                DhtValue::$variant(self.clone())
            }

            fn from_dht_value(value: DhtValue) -> Result<Self, DhtCodecError> {
                let actual = value.kind_name();
                match value {
                    DhtValue::$variant(value) => Ok(value),
                    _ => Err(DhtCodecError::UnexpectedValueType {
                        expected: Self::TYPE_NAME,
                        actual,
                    }),
                }
            }
        }
    };
}

impl_dht_value!(String, Text, "String");
impl_dht_value!(u64, UnsignedInteger, "u64");
impl_dht_value!(Vec<DhtNodeId>, DhtNodeList, "Vec<DhtNodeId>");
impl_dht_value!(GlobalFileInfo, GlobalFileInfo, "GlobalFileInfo");
impl_dht_value!(Vec<Permission>, PermissionList, "Vec<Permission>");

pub(super) fn encode_record(record: &DhtRecord) -> Result<Vec<u8>, DhtCodecError> {
    let record = record.encode_to_vec();
    if record.len() > MAX_DECOMPRESSED_RECORD_SIZE {
        return Err(DhtCodecError::UncompressedValueTooLarge {
            size: record.len(),
            max: MAX_DECOMPRESSED_RECORD_SIZE,
        });
    }
    let compressed_record = zstd::bulk::compress(&record, ZSTD_LEVEL)?;
    Ok(DhtRecordEnvelope {
        version: FORMAT_VERSION,
        compressed_record,
    }
    .encode_to_vec())
}

pub(super) fn decode_record(value: &[u8]) -> Result<DhtRecord, DhtCodecError> {
    if value.len() > MAX_DHT_VALUE_SIZE {
        return Err(DhtCodecError::RecordEnvelopeTooLarge {
            size: value.len(),
            max: MAX_DHT_VALUE_SIZE,
        });
    }
    let envelope = DhtRecordEnvelope::decode(value)?;
    if envelope.version != FORMAT_VERSION {
        return Err(DhtCodecError::UnsupportedVersion(envelope.version));
    }
    if envelope.compressed_record.is_empty() {
        return Err(DhtCodecError::MissingField("compressed_record"));
    }

    let record = zstd::bulk::decompress(
        &envelope.compressed_record,
        MAX_DECOMPRESSED_RECORD_SIZE,
    )?;
    Ok(DhtRecord::decode(record.as_slice())?)
}

impl From<&DhtNodeId> for ProtoDhtNodeId {
    fn from(value: &DhtNodeId) -> Self {
        let ip = match value.ip {
            IpAddr::V4(ip) => ip.octets().to_vec(),
            IpAddr::V6(ip) => ip.octets().to_vec(),
        };
        Self {
            ip,
            port: u32::from(value.port),
        }
    }
}

impl TryFrom<ProtoDhtNodeId> for DhtNodeId {
    type Error = DhtCodecError;

    fn try_from(value: ProtoDhtNodeId) -> Result<Self, Self::Error> {
        let ip = match value.ip.len() {
            4 => {
                let octets: [u8; 4] = value.ip.try_into().map_err(|_| {
                    DhtCodecError::InvalidValue("invalid IPv4 address length".to_string())
                })?;
                IpAddr::V4(Ipv4Addr::from(octets))
            }
            16 => {
                let octets: [u8; 16] = value.ip.try_into().map_err(|_| {
                    DhtCodecError::InvalidValue("invalid IPv6 address length".to_string())
                })?;
                IpAddr::V6(Ipv6Addr::from(octets))
            }
            size => {
                return Err(DhtCodecError::InvalidValue(format!(
                    "invalid IP address length: {size}"
                )))
            }
        };
        let port = u16::try_from(value.port)
            .map_err(|_| DhtCodecError::InvalidValue(format!("invalid port: {}", value.port)))?;
        Ok(DhtNodeId { ip, port })
    }
}

impl From<&StoredFile> for ProtoStoredFile {
    fn from(value: &StoredFile) -> Self {
        Self {
            version: value.version,
            is_keeper: value.is_keeper,
        }
    }
}

impl From<ProtoStoredFile> for StoredFile {
    fn from(value: ProtoStoredFile) -> Self {
        Self {
            version: value.version,
            is_keeper: value.is_keeper,
        }
    }
}

impl From<&FileKeeper> for ProtoFileKeeper {
    fn from(value: &FileKeeper) -> Self {
        Self {
            id: Some(ProtoDhtNodeId::from(&value.id)),
            data: value.data.iter().map(ProtoStoredFile::from).collect(),
        }
    }
}

impl TryFrom<ProtoFileKeeper> for FileKeeper {
    type Error = DhtCodecError;

    fn try_from(value: ProtoFileKeeper) -> Result<Self, Self::Error> {
        Ok(Self {
            id: DhtNodeId::try_from(
                value
                    .id
                    .ok_or(DhtCodecError::MissingField("file_keeper.id"))?,
            )?,
            data: value.data.into_iter().map(StoredFile::from).collect(),
        })
    }
}

impl From<FsNodeType> for ProtoFsNodeType {
    fn from(value: FsNodeType) -> Self {
        match value {
            FsNodeType::Folder => ProtoFsNodeType::Folder,
            FsNodeType::File => ProtoFsNodeType::File,
            FsNodeType::DataMeta => ProtoFsNodeType::DataMeta,
            FsNodeType::Data => ProtoFsNodeType::Data,
            FsNodeType::Rebac => ProtoFsNodeType::Rebac,
            FsNodeType::User => ProtoFsNodeType::User,
        }
    }
}

impl TryFrom<ProtoFsNodeType> for FsNodeType {
    type Error = DhtCodecError;

    fn try_from(value: ProtoFsNodeType) -> Result<Self, Self::Error> {
        match value {
            ProtoFsNodeType::Folder => Ok(FsNodeType::Folder),
            ProtoFsNodeType::File => Ok(FsNodeType::File),
            ProtoFsNodeType::DataMeta => Ok(FsNodeType::DataMeta),
            ProtoFsNodeType::Data => Ok(FsNodeType::Data),
            ProtoFsNodeType::Rebac => Ok(FsNodeType::Rebac),
            ProtoFsNodeType::User => Ok(FsNodeType::User),
            ProtoFsNodeType::Unspecified => Err(DhtCodecError::InvalidValue(
                "unspecified filesystem node type".to_string(),
            )),
        }
    }
}

impl From<&GlobalFileInfo> for ProtoGlobalFileInfo {
    fn from(value: &GlobalFileInfo) -> Self {
        Self {
            filename: value.filename.clone(),
            file_type: ProtoFsNodeType::from(value.file_type) as i32,
            id: value.id,
            keepers: value.keepers.iter().map(ProtoFileKeeper::from).collect(),
        }
    }
}

impl TryFrom<ProtoGlobalFileInfo> for GlobalFileInfo {
    type Error = DhtCodecError;

    fn try_from(value: ProtoGlobalFileInfo) -> Result<Self, Self::Error> {
        let file_type = ProtoFsNodeType::try_from(value.file_type).map_err(|_| {
            DhtCodecError::InvalidValue(format!(
                "invalid filesystem node type: {}",
                value.file_type
            ))
        })?;
        Ok(Self {
            filename: value.filename,
            file_type: FsNodeType::try_from(file_type)?,
            id: value.id,
            keepers: value
                .keepers
                .into_iter()
                .map(FileKeeper::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<ObjType> for ProtoObjType {
    fn from(value: ObjType) -> Self {
        match value {
            ObjType::File => ProtoObjType::File,
            ObjType::Folder => ProtoObjType::Folder,
        }
    }
}

impl TryFrom<ProtoObjType> for ObjType {
    type Error = DhtCodecError;

    fn try_from(value: ProtoObjType) -> Result<Self, Self::Error> {
        match value {
            ProtoObjType::File => Ok(ObjType::File),
            ProtoObjType::Folder => Ok(ObjType::Folder),
            ProtoObjType::Unspecified => Err(DhtCodecError::InvalidValue(
                "unspecified object type".to_string(),
            )),
        }
    }
}

impl From<&RebacKey> for ProtoRebacKey {
    fn from(value: &RebacKey) -> Self {
        Self {
            obj_type: ProtoObjType::from(value.obj_type) as i32,
            obj_id: value.obj_id,
        }
    }
}

impl TryFrom<ProtoRebacKey> for RebacKey {
    type Error = DhtCodecError;

    fn try_from(value: ProtoRebacKey) -> Result<Self, Self::Error> {
        let obj_type = ProtoObjType::try_from(value.obj_type).map_err(|_| {
            DhtCodecError::InvalidValue(format!("invalid object type: {}", value.obj_type))
        })?;
        Ok(Self {
            obj_type: ObjType::try_from(obj_type)?,
            obj_id: value.obj_id,
        })
    }
}

impl From<PermissionType> for ProtoPermissionType {
    fn from(value: PermissionType) -> Self {
        match value {
            PermissionType::Read => ProtoPermissionType::Read,
            PermissionType::Write => ProtoPermissionType::Write,
            PermissionType::ReadRecursively => ProtoPermissionType::ReadRecursively,
            PermissionType::WriteRecursively => ProtoPermissionType::WriteRecursively,
            PermissionType::GrantRead => ProtoPermissionType::GrantRead,
            PermissionType::GrantWrite => ProtoPermissionType::GrantWrite,
            PermissionType::GrantReadRecursively => ProtoPermissionType::GrantReadRecursively,
            PermissionType::GrantWriteRecursively => ProtoPermissionType::GrantWriteRecursively,
            PermissionType::ReadPermissionsOfOtherUsers => {
                ProtoPermissionType::ReadPermissionsOfOtherUsers
            }
            PermissionType::ReadPermissionsOfOtherUsersRecursively => {
                ProtoPermissionType::ReadPermissionsOfOtherUsersRecursively
            }
            PermissionType::All => ProtoPermissionType::All,
        }
    }
}

impl TryFrom<ProtoPermissionType> for PermissionType {
    type Error = DhtCodecError;

    fn try_from(value: ProtoPermissionType) -> Result<Self, Self::Error> {
        match value {
            ProtoPermissionType::Read => Ok(PermissionType::Read),
            ProtoPermissionType::Write => Ok(PermissionType::Write),
            ProtoPermissionType::ReadRecursively => Ok(PermissionType::ReadRecursively),
            ProtoPermissionType::WriteRecursively => Ok(PermissionType::WriteRecursively),
            ProtoPermissionType::GrantRead => Ok(PermissionType::GrantRead),
            ProtoPermissionType::GrantWrite => Ok(PermissionType::GrantWrite),
            ProtoPermissionType::GrantReadRecursively => Ok(PermissionType::GrantReadRecursively),
            ProtoPermissionType::GrantWriteRecursively => Ok(PermissionType::GrantWriteRecursively),
            ProtoPermissionType::ReadPermissionsOfOtherUsers => {
                Ok(PermissionType::ReadPermissionsOfOtherUsers)
            }
            ProtoPermissionType::ReadPermissionsOfOtherUsersRecursively => {
                Ok(PermissionType::ReadPermissionsOfOtherUsersRecursively)
            }
            ProtoPermissionType::All => Ok(PermissionType::All),
            ProtoPermissionType::Unspecified => Err(DhtCodecError::InvalidValue(
                "unspecified permission type".to_string(),
            )),
        }
    }
}

impl From<&Permission> for ProtoPermission {
    fn from(value: &Permission) -> Self {
        Self {
            subject: value.subject,
            object: Some(ProtoRebacKey::from(&value.object)),
            permission_type: ProtoPermissionType::from(value.permission_type) as i32,
            version: value.version,
            deleted: value.deleted,
        }
    }
}

impl TryFrom<ProtoPermission> for Permission {
    type Error = DhtCodecError;

    fn try_from(value: ProtoPermission) -> Result<Self, Self::Error> {
        let permission_type =
            ProtoPermissionType::try_from(value.permission_type).map_err(|_| {
                DhtCodecError::InvalidValue(format!(
                    "invalid permission type: {}",
                    value.permission_type
                ))
            })?;
        Ok(Self {
            subject: value.subject,
            object: RebacKey::try_from(
                value
                    .object
                    .ok_or(DhtCodecError::MissingField("permission.object"))?,
            )?,
            permission_type: PermissionType::try_from(permission_type)?,
            version: value.version,
            deleted: value.deleted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_value(value: DhtValue) -> Vec<u8> {
        encode_record(&DhtRecord {
            pairs: vec![DistributedMapVal {
                key: "test/key".to_string(),
                value: Some(value.into_proto()),
            }],
        })
        .unwrap()
    }

    fn round_trip(value: DhtValue) -> DhtValue {
        let encoded = encode_value(value);
        let mut record = decode_record(&encoded).unwrap();
        let value = record.pairs.swap_remove(0).value.unwrap();
        DhtValue::from_proto(value).unwrap()
    }

    #[test]
    fn round_trips_scalar_values() {
        match round_trip(DhtValue::Text("hello".to_string())) {
            DhtValue::Text(value) => assert_eq!(value, "hello"),
            value => panic!("unexpected value type: {}", value.kind_name()),
        }

        match round_trip(DhtValue::UnsignedInteger(u64::MAX)) {
            DhtValue::UnsignedInteger(value) => assert_eq!(value, u64::MAX),
            value => panic!("unexpected value type: {}", value.kind_name()),
        }
    }

    #[test]
    fn round_trips_node_lists() {
        let expected = vec![
            DhtNodeId {
                ip: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                port: 8080,
            },
            DhtNodeId {
                ip: IpAddr::V6(Ipv6Addr::LOCALHOST),
                port: u16::MAX,
            },
        ];

        match round_trip(DhtValue::DhtNodeList(expected.clone())) {
            DhtValue::DhtNodeList(value) => assert_eq!(value, expected),
            value => panic!("unexpected value type: {}", value.kind_name()),
        }
    }

    #[test]
    fn round_trips_global_file_info() {
        let expected = GlobalFileInfo {
            filename: "home/user/file.txt".to_string(),
            file_type: FsNodeType::File,
            id: 42,
            keepers: vec![FileKeeper {
                id: DhtNodeId {
                    ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                    port: 8080,
                },
                data: vec![
                    StoredFile {
                        version: 1,
                        is_keeper: true,
                    },
                    StoredFile {
                        version: 2,
                        is_keeper: false,
                    },
                ],
            }],
        };

        match round_trip(DhtValue::GlobalFileInfo(expected.clone())) {
            DhtValue::GlobalFileInfo(value) => {
                assert_eq!(value.filename, expected.filename);
                assert_eq!(value.file_type, expected.file_type);
                assert_eq!(value.id, expected.id);
                assert_eq!(value.keepers, expected.keepers);
            }
            value => panic!("unexpected value type: {}", value.kind_name()),
        }
    }

    #[test]
    fn round_trips_permission_lists() {
        let expected = vec![Permission {
            subject: 11,
            object: RebacKey::new(ObjType::Folder, 42),
            permission_type: PermissionType::GrantWriteRecursively,
            version: 7,
            deleted: true,
        }];

        match round_trip(DhtValue::PermissionList(expected.clone())) {
            DhtValue::PermissionList(value) => {
                assert_eq!(value.len(), expected.len());
                assert_eq!(value[0].subject, expected[0].subject);
                assert_eq!(value[0].object, expected[0].object);
                assert_eq!(value[0].permission_type, expected[0].permission_type);
                assert_eq!(value[0].version, expected[0].version);
                assert_eq!(value[0].deleted, expected[0].deleted);
            }
            value => panic!("unexpected value type: {}", value.kind_name()),
        }
    }

    #[test]
    fn round_trips_all_domain_enum_values() {
        for value in [
            FsNodeType::Folder,
            FsNodeType::File,
            FsNodeType::DataMeta,
            FsNodeType::Data,
            FsNodeType::Rebac,
            FsNodeType::User,
        ] {
            assert_eq!(
                FsNodeType::try_from(ProtoFsNodeType::from(value)).unwrap(),
                value
            );
        }

        for value in [ObjType::File, ObjType::Folder] {
            assert_eq!(ObjType::try_from(ProtoObjType::from(value)).unwrap(), value);
        }

        for value in [
            PermissionType::Read,
            PermissionType::Write,
            PermissionType::ReadRecursively,
            PermissionType::WriteRecursively,
            PermissionType::GrantRead,
            PermissionType::GrantWrite,
            PermissionType::GrantReadRecursively,
            PermissionType::GrantWriteRecursively,
            PermissionType::ReadPermissionsOfOtherUsers,
            PermissionType::ReadPermissionsOfOtherUsersRecursively,
            PermissionType::All,
        ] {
            assert_eq!(
                PermissionType::try_from(ProtoPermissionType::from(value)).unwrap(),
                value
            );
        }
    }

    #[test]
    fn rejects_unknown_format_version() {
        let encoded = DhtRecordEnvelope {
            version: FORMAT_VERSION + 1,
            compressed_record: vec![1],
        }
        .encode_to_vec();

        assert!(matches!(
            decode_record(&encoded),
            Err(DhtCodecError::UnsupportedVersion(_))
        ));
    }

    #[test]
    fn protobuf_wire_tags_are_stable() {
        let record = DhtRecord {
            pairs: vec![DistributedMapVal {
                key: "k".to_string(),
                value: Some(DhtValue::Text("v".to_string()).into_proto()),
            }],
        };
        assert_eq!(
            record.encode_to_vec(),
            vec![0x0a, 0x06, 0x0a, 0x01, b'k', 0x12, 0x01, b'v']
        );

        let envelope = DhtRecordEnvelope {
            version: 1,
            compressed_record: vec![0xaa, 0xbb],
        };
        assert_eq!(
            envelope.encode_to_vec(),
            vec![0x08, 0x01, 0x12, 0x02, 0xaa, 0xbb]
        );
    }

    #[test]
    fn rejects_invalid_compressed_record() {
        let encoded = DhtRecordEnvelope {
            version: FORMAT_VERSION,
            compressed_record: b"not a zstd frame".to_vec(),
        }
        .encode_to_vec();

        assert!(matches!(
            decode_record(&encoded),
            Err(DhtCodecError::Compression(_))
        ));
    }

    #[test]
    fn rejects_oversized_record_envelope() {
        assert!(matches!(
            decode_record(&vec![0; MAX_DHT_VALUE_SIZE + 1]),
            Err(DhtCodecError::RecordEnvelopeTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_unexpected_value_type() {
        assert!(matches!(
            String::from_dht_value(DhtValue::UnsignedInteger(42)),
            Err(DhtCodecError::UnexpectedValueType { .. })
        ));
    }

    #[test]
    fn rejects_oversized_uncompressed_record() {
        let record = DhtRecord {
            pairs: vec![DistributedMapVal {
                key: "test/key".to_string(),
                value: Some(DhtValue::Text("x".repeat(MAX_DECOMPRESSED_RECORD_SIZE)).into_proto()),
            }],
        };

        assert!(matches!(
            encode_record(&record),
            Err(DhtCodecError::UncompressedValueTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_compressed_record_expanding_past_limit() {
        let oversized_record = DhtRecord {
            pairs: vec![DistributedMapVal {
                key: "test/key".to_string(),
                value: Some(DhtValue::Text("x".repeat(MAX_DECOMPRESSED_RECORD_SIZE)).into_proto()),
            }],
        }
        .encode_to_vec();
        let encoded = DhtRecordEnvelope {
            version: FORMAT_VERSION,
            compressed_record: zstd::bulk::compress(&oversized_record, ZSTD_LEVEL).unwrap(),
        }
        .encode_to_vec();

        assert!(matches!(
            decode_record(&encoded),
            Err(DhtCodecError::Compression(_))
        ));
    }

    #[test]
    fn representative_structured_values_fit_dht_limit() {
        let keepers = (0..8)
            .map(|index| FileKeeper {
                id: DhtNodeId {
                    ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, index + 1)),
                    port: 8080 + u16::from(index),
                },
                data: (0..8)
                    .map(|version| StoredFile {
                        version,
                        is_keeper: true,
                    })
                    .collect(),
            })
            .collect();
        let file_info = GlobalFileInfo {
            filename: "home/user/a-representative-file-name.txt".to_string(),
            file_type: FsNodeType::File,
            id: u64::MAX / 2,
            keepers,
        };
        assert!(encode_value(DhtValue::GlobalFileInfo(file_info)).len() <= MAX_DHT_VALUE_SIZE);

        let permissions = (0..32)
            .map(|index| Permission {
                subject: 10_000 + index,
                object: RebacKey::new(ObjType::File, 42),
                permission_type: PermissionType::Read,
                version: index,
                deleted: false,
            })
            .collect();
        assert!(encode_value(DhtValue::PermissionList(permissions)).len() <= MAX_DHT_VALUE_SIZE);
    }
}
