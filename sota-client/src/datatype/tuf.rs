use base64;
use chrono::{DateTime, Utc};
use crypto::digest::Digest;
use crypto::sha2::Sha256;
use json;
use pem;
use serde::de::{Deserialize, Deserializer, Error as SerdeError};
use std::fmt::{self, Display, Formatter};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use datatype::{CanonicalJson, Error, InstallResult, Signature, SignatureType};


#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Default)]
pub struct TufSigned {
    pub signatures: Vec<Signature>,
    pub signed:     json::Value,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct TufImage {
    pub filepath: String,
    pub fileinfo: TufMeta
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct TufMeta {
    pub length: u64,
    pub hashes: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<TufCustom>,
}

impl TufMeta {
    pub fn from(hash_type: String, commit: String) -> Self {
        TufMeta { length: 0, hashes: hashmap!{ hash_type => commit }, custom: None }
    }
}

#[allow(non_snake_case)]
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct TufCustom {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecuIdentifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}


#[derive(Serialize, PartialEq, Eq, Debug, Clone, Copy, Hash)]
#[serde(tag = "_type")]
pub enum RoleName {
    Root,
    Targets,
    Snapshot,
    Timestamp
}

impl FromStr for RoleName {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_ref() {
            "root"      => Ok(RoleName::Root),
            "snapshot"  => Ok(RoleName::Snapshot),
            "targets"   => Ok(RoleName::Targets),
            "timestamp" => Ok(RoleName::Timestamp),
            _           => Err(Error::TufRole(s.into()))
        }
    }
}

impl Display for RoleName {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match *self {
            RoleName::Root      => write!(f, "root"),
            RoleName::Targets   => write!(f, "targets"),
            RoleName::Snapshot  => write!(f, "snapshot"),
            RoleName::Timestamp => write!(f, "timestamp"),
        }
    }
}

impl<'de> Deserialize<'de> for RoleName {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<RoleName, D::Error> {
        let s: String = Deserialize::deserialize(de)?;
        s.parse().map_err(|err| SerdeError::custom(format!("unknown RoleName: {}", err)))
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct RoleData {
    pub _type:   RoleName,
    pub version: u64,
    pub expires: DateTime<Utc>,
    pub keys:    Option<HashMap<String, Key>>,        // root only
    pub roles:   Option<HashMap<RoleName, RoleMeta>>, // root only
    pub targets: Option<HashMap<String, TufMeta>>,    // targets only
    pub meta:    Option<HashMap<String, TufMeta>>,    // timestamp/snapshot only
}

impl RoleData {
    pub fn expired(&self) -> bool {
        self.expires < Utc::now()
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct RoleMeta {
    pub keyids:    HashSet<String>,
    pub threshold: u64,
    #[serde(skip_serializing, skip_deserializing)]
    pub version:   u64,
}


#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Key {
    pub keytype: KeyType,
    pub keyval:  KeyValue,
}

impl Key {
    pub fn key_id(&self) -> Result<String, Error> {
        let mut hasher = Sha256::new();
        match self.keytype {
            KeyType::Ed25519 => hasher.input_str(&format!(r#""{}""#, self.keyval.public)),
            KeyType::Rsa => hasher.input(&pem::parse(self.keyval.public.as_bytes())?.contents)
        }
        Ok(hasher.result_str())
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Hash)]
pub struct KeyValue {
    pub public: String,
}

#[derive(Clone)]
pub struct PrivateKey {
    pub keyid:   String,
    pub der_key: Vec<u8>,
}

impl PrivateKey {
    pub fn sign_data(&self, data: json::Value, sig_type: SignatureType) -> Result<TufSigned, Error> {
        let cjson = CanonicalJson::convert(json::to_value(&data)?)?;
        let signed = TufSigned {
            signatures: vec![Signature {
                keyid:  self.keyid.clone(),
                method: sig_type,
                sig:    base64::encode(&sig_type.sign_msg(&cjson, &self.der_key)?),
            }],
            signed: data,
        };
        Ok(signed)
    }
}

#[derive(Serialize, PartialEq, Eq, Debug, Clone)]
pub enum KeyType {
    Ed25519,
    Rsa,
}

impl<'de> Deserialize<'de> for KeyType {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s: String = Deserialize::deserialize(de)?;
        s.parse().map_err(|err| SerdeError::custom(format!("unknown KeyType: {}", err)))
    }
}

impl FromStr for KeyType {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_ref() {
            "ed25519" => Ok(KeyType::Ed25519),
            "rsa"     => Ok(KeyType::Rsa),
            _         => Err(Error::TufKeyType(s.to_string()))
        }
    }
}


pub type Manifests = HashMap<String, TufSigned>;

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct EcuManifests {
    pub primary_ecu_serial: String,
    pub ecu_version_manifests: Manifests,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct EcuVersion {
    pub attacks_detected:         String,
    pub ecu_serial:               String,
    pub installed_image:          TufImage,
    pub previous_timeserver_time: String,
    pub timeserver_time:          String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<EcuCustom>,
}

impl EcuVersion {
    pub fn from(ecu_serial: String, image: TufImage, custom: Option<EcuCustom>) -> Self {
        EcuVersion {
            attacks_detected: "".into(),
            custom: custom,
            ecu_serial: ecu_serial,
            installed_image: image,
            previous_timeserver_time: "1970-01-01T00:00:00Z".into(),
            timeserver_time: "1970-01-01T00:00:00Z".into(),
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct EcuCustom {
    pub operation_result: InstallResult
}

impl EcuCustom {
    pub fn from_result(operation_result: InstallResult) -> Self {
        EcuCustom { operation_result }
    }
}
