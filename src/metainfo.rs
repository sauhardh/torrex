use serde::Deserialize;
use serde::Serialize;
use sha1::digest::generic_array::GenericArray;
use sha1::digest::generic_array::typenum::U20;
use sha1::{Digest, Sha1};

use std::fs;
use std::io::Error;
use std::io::ErrorKind::InvalidInput;
use std::path::Path;

use crate::bencode::Bencode;

#[derive(Default, Deserialize, Debug, Serialize)]
pub struct File {
    // length of the file in bytes (integer)
    pub length: usize,
    // a list containing one or more string elements that together represent the path and filename.
    // Each element in the list corresponds to either a directory name or (in the case of the final element) the filename.
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
    // UTF-8 encoded string which is the suggested name to save the file (for a singlefile case) or directory (for a multifile case) as. It is purely advisory.
    pub name: String,
    // Number of bytes in each piece the file is split into.
    #[serde(rename = "piece length")]
    pub piece_length: usize,
    // Length is a multiple of 20. It is to be subdivided into strings of length 20, each of which is the SHA1 hash of the piece at the corresponding index.
    pub pieces: Option<Vec<u8>>,

    // Key for single file and multifile option.
    #[serde(flatten)]
    pub key: FileKey,
}

impl Info {
    pub fn pieces_hashes(&self) -> Vec<String> {
        self.pieces
            .as_ref()
            .unwrap()
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
    //  The URL of the tracker.
    pub announce: String,
    // This maps to a dictionary.
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

    pub fn parse_pieces<'a>(&self, encoded_value: &'a Vec<u8>) -> Option<(&'a [u8], usize)> {
        let tag = b"6:pieces";
        if let Some(pieces_idx) = encoded_value
            .windows(tag.len())
            .position(|window| window == tag)
        {
            let after_tag = &encoded_value[pieces_idx + tag.len()..];
            let colon_idx = after_tag.iter().position(|b| b == &b':')?;

            let length = std::str::from_utf8(&after_tag[..colon_idx])
                .ok()?
                .parse::<usize>()
                .ok()?;
            let pieces_data = &after_tag[colon_idx + 1..colon_idx + length + 1];

            return Some((pieces_data, pieces_idx));
        }
        None
    }

    pub fn parse_metafile(
        &self,
        encoded_value: &Vec<u8>,
        pieces_data: &Vec<u8>,
        pieces_idx: usize,
    ) -> TorrentFile {
        let bencoded = Bencode::new();

        // This takes the value until the end of pieces value.
        // After that, remaining are ignored
        // NOTE/TODO: The end delimiter 'e' is also cut out with this. Need to handle more gracefully
        let output = bencoded.decoder(&encoded_value[..pieces_idx]).0;

        let mut finally: TorrentFile = serde_json::from_value(output).unwrap();
        finally.info.pieces = Some(pieces_data.to_vec());
        return finally;
    }

    pub fn info_parser<'a>(&self, encoded_value: &'a [u8]) -> Option<&'a [u8]> {
        let tag = b"4:info";
        if let Some(pos) = encoded_value.windows(tag.len()).position(|b| b == tag) {
            let start = pos + tag.len();
            let after_tag = &encoded_value[start..encoded_value.len() - 1];

            return Some(after_tag);
        };
        None
    }

    pub fn sha1_hash(&self, data: &[u8]) -> GenericArray<u8, U20> {
        let mut hasher = Sha1::new();
        hasher.update(data);
        hasher.finalize()
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
                let (pieces_data, pieces_idx) = meta.parse_pieces(&output).unwrap();

                let meta_info: TorrentFile =
                    meta.parse_metafile(&output, &pieces_data.to_vec(), pieces_idx);

                let (_length, _files) = match &meta_info.info.key {
                    FileKey::SingleFile { length } => (Some(length), None),
                    FileKey::MultiFile { files } => (None, Some(files)),
                };
                println!("MetaInfo:{:?}", meta_info);
                println!("Length: {:?}", _length);

                let info = meta_info.info_parser(&output).unwrap();
                let hashed_info = meta_info.sha1_hash(info);
                println!("hashed_info: {:02x}", hashed_info);

                println!("{:#?}", meta_info.info.pieces_hashes());
            }

            Err(e) => {
                eprintln!("Error occured while reading file.\n{:?}", e)
            }
        }
    }
}
