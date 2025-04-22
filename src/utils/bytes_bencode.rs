use std::collections::HashMap;
use std::hash::Hash;
use std::str::from_utf8;

use serde_json::Value;

#[derive(Clone)]
struct BytesBencode;

impl BytesBencode {
    fn new() -> Self {
        Self {}
    }

    /// Decodes the string from the encoded( encoded as <length>:<string> ) value
    /// Example: `5:hello -> hello`
    pub fn decode_string(self, encoded_value: &[u8]) -> (Value, &[u8]) {
        let col_idx = encoded_value.iter().position(|byte| byte == &b':').unwrap();

        let len = from_utf8(&encoded_value[..col_idx])
            .unwrap()
            .parse::<usize>()
            .unwrap();

        let start = col_idx + 1;
        let end = start + len;

        if end > encoded_value.len() {
            eprintln!(
                "Index out of found. Incorrect field value. {:?}",
                from_utf8(encoded_value).unwrap()
            );
        }

        let value = &encoded_value[start..end];
        let rest = &encoded_value[end..];

        (from_utf8(&value).unwrap().into(), rest)
    }

    /// Decodes the integer from the encoded( encoded as i<number>e ) value
    /// Example: `i52e -> 52`
    /// NOTE: all integers with leading 0 is invalid i.e. i-0e or i05e. BUT i0e is valid as 0 is an integer itself.
    pub fn decode_integer(self, encoded_value: &[u8]) -> (Value, &[u8]) {
        let end_idx = encoded_value.iter().position(|byte| byte == &b'e').unwrap();

        let n = from_utf8(&encoded_value[1..end_idx])
            .unwrap()
            .parse::<i64>()
            .unwrap();

        (n.into(), &encoded_value[end_idx..])
    }

    /// This acts as hub for decoding values of several  datatypes like, string, integer, list, dictionary.
    pub fn decoder(self, encoded_value: &[u8]) -> (Value, &[u8]) {
        match encoded_value[0] {
            // String
            b'0'..=b'9' => {
                let (value, rest) = self.decode_string(encoded_value);
                (value, rest)
            }

            // Integer
            b'i' => {
                let (value, rest) = self.decode_integer(encoded_value);
                (value, rest)
            }

            // List
            // Decodes the list from the encoded( encoded as l<bencoded values>e ) value
            // Example:  `l4:spam4:eggse -> [ "spam", "eggs" ]`
            b'l' => {
                let mut rest = &encoded_value[1..];
                let mut list = Vec::new();

                while !rest.is_empty() && rest[0] != b'e' {
                    let (value, r) = self.clone().decoder(rest);
                    list.push(value.clone());
                    rest = r;
                }

                (list.into(), &rest[1..])
            }

            // Dictionary
            // Decodes the dictionary from the encoded ( encoded as  d<bencoded string><bencoded element>e ) value
            // Example:  `d4:spaml1:a1:bee -> { "spam" => [ "a", "b" ] }`
            b'd' => {
                let mut rest = &encoded_value[1..];
                let mut hash: HashMap<String, serde_json::Value> = HashMap::new();

                let (mut if_key, mut key) = (false, None);

                while rest.to_vec().len() > 2 {
                    let (value, r) = self.clone().decoder(rest);

                    if if_key {
                        hash.insert(key.clone().unwrap(), value);
                        if_key = false;
                    } else {
                        if let serde_json::Value::String(s) = &value {
                            key = Some(s.clone());
                            if_key = true;
                        };
                    }
                    rest = r;
                }

                let value = serde_json::Value::Object(hash.into_iter().collect());
                (value, &rest[1..])
            }

            _ => {
                panic!(
                    "Unhandled encoded value. Could not parse: {:?}",
                    encoded_value
                );
            }
        }
    }
}

#[cfg(test)]
mod bytes_bencode_testing {
    use crate::bencode;

    use super::*;

    // #[test]
    fn test_decode_string() {
        let bencoding = BytesBencode::new();
        let output = bencoding.decode_string("5:hell0 ".as_bytes());
        println!("Output is: {:?}", output);
    }

    // #[test]
    fn test_decode_int() {
        let bencoding = BytesBencode::new();
        let output = bencoding.decode_integer("i32e".as_bytes());
        println!("Int output is: {:?}", output);
    }

    #[test]
    fn test_decoder() {
        let bencoding = BytesBencode::new();
        let output = bencoding.decoder("d4:spaml1:a1:bee".as_bytes());
        println!("decoder output is: {}", output.0);
    }
}
