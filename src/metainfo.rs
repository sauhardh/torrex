use std::fs;
use std::io::Error;
use std::io::ErrorKind::InvalidInput;
use std::path::Path;

use crate::bencode::Bencode;

use serde::Deserialize;
use serde::Serialize;

#[derive(Default, Deserialize, Debug)]
pub struct File {
    // length of the file in bytes (integer)
    pub length: usize,
    // a list containing one or more string elements that together represent the path and filename.
    // Each element in the list corresponds to either a directory name or (in the case of the final element) the filename.
    pub path: Vec<String>,
}

#[derive(Deserialize, Debug)]
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

#[derive(Default, Deserialize, Debug)]
struct Info {
    // UTF-8 encoded string which is the suggested name to save the file (for a singlefile case) or directory (for a multifile case) as. It is purely advisory.
    pub name: String,
    // Number of bytes in each piece the file is split into.
    #[serde(rename = "piece length")]
    pub piece_length: u64,
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

#[derive(Default, Deserialize, Debug)]
struct MetaInfo {
    //  The URL of the tracker.
    pub announce: String,
    // This maps to a dictionary.
    pub info: Info,
}

impl MetaInfo {
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
        if let Some(pieces_idx) = encoded_value
            .windows(6)
            .position(|window| window == b"pieces")
        {
            let after_pieces = &encoded_value[pieces_idx + 6..];

            let delim_idx = after_pieces.iter().position(|b| b == &b':').unwrap();

            let length = String::from_utf8(after_pieces[..delim_idx].to_vec())
                .unwrap()
                .parse::<usize>()
                .unwrap();

            let pieces_data = &after_pieces[delim_idx + 1..delim_idx + length + 1];

            return Some((pieces_data, pieces_idx - 2));
        }
        None
    }

    pub fn parse_file(
        &self,
        encoded_value: &Vec<u8>,
        pieces_data: &Vec<u8>,
        pieces_idx: usize,
    ) -> MetaInfo {
        let bencoded = Bencode::new();
        let output = bencoded.decoder(&encoded_value[..pieces_idx]).0;

        let mut finally: MetaInfo = serde_json::from_value(output).unwrap();
        finally.info.pieces = Some(pieces_data.to_vec());
        return finally;
    }
}

#[cfg(test)]
mod metainfo_tester {
    use super::*;

    #[test]
    fn test_read_file() {
        let meta = MetaInfo::new();

        match meta.read_file(Path::new("./sample.torrent")) {
            Ok(output) => {
                let (pieces_data, pieces_idx) = meta.parse_pieces(&output).unwrap();
                let meta_info = meta.parse_file(&output, &pieces_data.to_vec(), pieces_idx);

                let (_length, _files) = match &meta_info.info.key {
                    FileKey::SingleFile { length } => (Some(length), None),
                    FileKey::MultiFile { files } => (None, Some(files)),
                };

                println!("{:#?}", meta_info);
            }

            Err(e) => {
                eprintln!("Error occured while reading file.\n{:?}", e)
            }
        }
    }
}
