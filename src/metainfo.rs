use serde::Deserialize;
use serde::Serialize;
use sha1::digest::generic_array::GenericArray;
use sha1::digest::generic_array::typenum::U20;

use std::fs;
use std::io::Error;
use std::io::ErrorKind::InvalidInput;
use std::path::Path;

use crate::bencode::Bencode;
use crate::cryptography::sha1_hash;

#[derive(Default, Deserialize, Debug, Serialize)]
pub struct File {
    /// length of the file in bytes (integer)
    pub length: usize,
    /// List containing one or more string elements that together represent the path and filename.
    ///
    /// Each element in the list corresponds to either a directory name or (in the case of the final element) the filename.
    pub path: Vec<String>,
}

#[derive(Deserialize, Debug, Serialize)]
#[serde(untagged)]
pub enum FileKey {
    SingleFile { length: usize },
    MultiFile { files: Vec<File> },
}

impl Default for FileKey {
    fn default() -> Self {
        Self::SingleFile { length: 0 }
    }
}

#[derive(Default, Deserialize, Debug, Serialize)]
pub struct Info {
    /// UTF-8 encoded string which is suggested name to save the file (for a singlefile case) or directory (for a multifile case) as. It is purely advisory.
    pub name: String,
    /// Number of bytes in each piece the file is split into.
    #[serde(rename = "piece length")]
    pub piece_length: usize,
    /// Length is a multiple of 20. It is to be subdivided into strings of length 20, each of which is the SHA1 hash of the piece at the corresponding index.
    pub pieces: Vec<u8>,
    /// Key for single file and multifile option.
    #[serde(flatten)]
    pub key: FileKey,
}

impl Info {
    pub fn pieces_hashes(&self) -> Vec<String> {
        self.pieces
            .chunks_exact(20)
            .map(|chunk| {
                let mut hash = [0u8; 20];
                hash.copy_from_slice(chunk);

                let hex_hash: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
                hex_hash
            })
            .collect()
    }
}

#[derive(Default, Deserialize, Debug, Serialize)]
pub struct TorrentFile {
    ///  The URL of the tracker.
    pub announce: String,
    /// This maps to a dictionary.
    pub info: Info,
}

impl TorrentFile {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read_file(&self, path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        if path.extension().unwrap() != "torrent" {
            return Err(Box::new(Error::new(
                InvalidInput,
                format!(
                    "Got incorrect file format. Expected: '*.torrent', Got: {:?}",
                    path
                ),
            )));
        }

        let data = fs::read(path)?;
        Ok(data)
    }

    pub fn parse_metafile(&self, encoded_value: &Vec<u8>) -> TorrentFile {
        let bencoded = Bencode::new();

        let output = bencoded.clone().decoder(&encoded_value);
        let finally: TorrentFile = serde_json::from_value(output.0).unwrap();

        return finally;
    }

    pub fn info_hash<'a>(&self, encoded_value: &'a [u8]) -> Option<GenericArray<u8, U20>> {
        let tag = b"4:info";
        if let Some(pos) = encoded_value.windows(tag.len()).position(|b| b == tag) {
            let start = pos + tag.len();
            let after_tag = &encoded_value[start..encoded_value.len() - 1];

            let hashed = sha1_hash(after_tag);
            return Some(hashed);
        };
        None
    }
}

#[cfg(test)]
mod metainfo_tester {

    use super::*;

    #[test]
    fn test_read_file() {
        let meta = TorrentFile::new();

        match meta.read_file(Path::new("./sample.torrent")) {
            Ok(output) => {
                let meta_info: TorrentFile = meta.parse_metafile(&output);

                let (_length, _files) = match &meta_info.info.key {
                    FileKey::SingleFile { length } => (Some(length), None),
                    FileKey::MultiFile { files } => (None, Some(files)),
                };

                let info_hash = meta_info.info_hash(&output).unwrap();
                println!("MetaInfo:{:#?}", meta_info);
                println!("info_hash: {:02x}", info_hash);
                println!("Pieces Hash{:#?}", meta_info.info.pieces_hashes());
            }

            Err(e) => {
                eprintln!("Error occured while reading file.\n{:?}", e)
            }
        }
    }
}
