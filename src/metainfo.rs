use std::fs;
use std::io::BufRead;
use std::io::ErrorKind::InvalidInput;
use std::io::{BufReader, Error};
use std::path::Path;

#[derive(Default)]
struct Info {
    length: u64,
    name: String,
    piece_length: u64,
    pieces: Vec<u8>,
}

#[derive(Default)]
struct MetaInfo {
    announce: String,
    info: Info,
}

impl MetaInfo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read_file(self, path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
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
}

#[cfg(test)]
mod metainfo_tester {
    use crate::bencode::Bencode;

    use super::*;

    #[test]
    fn test_read_file() {
        let meta = MetaInfo::new();

        match meta.read_file(Path::new("./sample.torrent")) {
            Ok(output) => {
                let b = Bencode::new();
                // let string = String::from_utf8(output).unwrap();

                // let o = b.decoder(&output);
                // println!("O:{:?}", o);
            }

            Err(e) => {
                eprintln!("Error occured while reading file.\n{:?}", e)
            }
        }
    }
}
