use std::io::{self, BufReader, Read, Seek, Write};

use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};

pub mod cdxj;
pub mod pages;
pub mod warc;
pub mod writer;

#[derive(Serialize)]
pub struct DataPackage {
    pub profile: &'static str,
    pub wacz_version: &'static str,
    pub software: &'static str,
    pub created: String,
    pub resources: Vec<DataPackageEntry>,
}

#[derive(Serialize, Clone, Debug)]
pub struct DataPackageEntry {
    pub name: String,
    pub path: String,
    #[serde(serialize_with = "ser_sha256_as_str")]
    pub hash: [u8; 32],
    pub bytes: u64,
}

pub fn sha256_as_string(hash: &[u8; 32]) -> String {
    let mut out = vec![b'a'; 71]; // 'sha256:' + 64 hex chars
    out[0..7].copy_from_slice(b"sha256:");
    faster_hex::hex_encode(&hash[..], &mut out[7..]).unwrap();

    unsafe { String::from_utf8_unchecked(out) }
}

pub fn ser_sha256_as_str<S>(hash: &[u8; 32], ser: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    ser.serialize_str(&sha256_as_string(hash))
}

pub fn file_digest<R: Read + Seek>(file: &mut R) -> io::Result<[u8; 32]> {
    file.rewind().unwrap();
    let mut out = vec![];
    file.read_to_end(&mut out).unwrap();
    file.rewind().unwrap();

    let mut reader = BufReader::new(file);

    let mut hasher = Sha256::new();

    std::io::copy(&mut reader, &mut hasher).unwrap();

    hasher.flush()?;

    let digest = hasher.finalize();

    Ok(digest.into())
}
