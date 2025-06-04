use hex;
use serde_json::Value;

use std::collections::HashMap;
use std::str::from_utf8;

#[derive(Clone, PartialEq)]
pub struct Bencode;

impl Bencode {
    pub fn new() -> Self {
        Self {}
    }

    /// Decodes the string from the encoded value. It is encoded as `<length>:<string>`
    ///
    /// Example: `5:helloworld -> hello`
    #[inline]
    fn decode_string<'a>(&'a self, encoded_value: &'a [u8]) -> (Value, &'a [u8]) {
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

        match from_utf8(&value) {
            Ok(s) => (serde_json::Value::String(s.to_string()), rest),
            Err(_) => (
                serde_json::Value::Array(
                    value
                        .iter()
                        .map(|b| serde_json::Value::Number(serde_json::Number::from(*b)).into())
                        .collect(),
                ),
                rest,
            ),
        }
    }

    /// Decodes the integer from the encoded value. It is encoded as `i<number>e`
    ///
    /// Example: `i52e -> 52`
    ///
    /// NOTE: all integers with leading 0 is invalid i.e. `i-0e` or `i05e`. BUT `i0e` is valid as 0 is an integer itself.
    #[inline]
    fn decode_integer<'a>(&'a self, encoded_value: &'a [u8]) -> (Value, &'a [u8]) {
        let end_idx = encoded_value.iter().position(|byte| byte == &b'e').unwrap();

        let n = from_utf8(&encoded_value[1..end_idx])
            .unwrap()
            .parse::<i64>()
            .unwrap();

        (n.into(), &encoded_value[end_idx + 1..])
    }

    /// This acts as hub for decoding values of several  datatypes like, string, integer, list, dictionary.
    pub fn decoder<'a>(&'a self, encoded_value: &'a [u8]) -> (Value, &'a [u8]) {
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
                    let (value, r) = self.decoder(rest);
                    list.push(value);
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

                while !rest.is_empty() && rest[0] != b'e' {
                    let (value, r) = self.decoder(rest);

                    if if_key {
                        hash.insert(key.clone().unwrap(), value);
                        if_key = false;
                    } else {
                        if let serde_json::Value::String(s) = &value {
                            key = Some(s.to_string());
                            if_key = true;
                        };
                    }
                    rest = r;
                }

                let value = serde_json::Value::Object(hash.into_iter().collect());
                let rest = if rest.len() > 1 { &rest[1..] } else { &rest };

                (value, rest)
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
mod test_bencode {

    use super::*;

    // #[test]
    fn test_decode_string() {
        let bencoding = Bencode::new();
        let output = bencoding.decode_string("5:hell0 ".as_bytes());
        assert_eq!("hell0", output.0);
    }

    // #[test]
    fn test_decode_int() {
        let bencoding = Bencode::new();
        let output = bencoding.decode_integer("i32e".as_bytes());
        assert_eq!("32", output.0);
    }

    #[test]
    fn test_decoder() {
        let bencoding = Bencode::new();
        let encoded_value = "d8:intervali60e12:min intervali60e5:peersld7:peer id20:-RN0.0.0-�\u{1c}��ۆ�jʚ!2:ip13:167.71.143.544:porti51501eed7:peer id20:-RN0.0.0-\u{12}\u{8}F�\u{10}% �%��2:ip14:139.59.184.2554:porti51510eed2:ip14:165.232.35.1394:porti51413e7:peer id20:-RN0.0.0-2��Qᓮ��ɍee8:completei3e10:incompletei1ee";
        println!("Encoded value: {encoded_value}");
        let output = bencoding.decoder(encoded_value.as_bytes());
        println!("{:?}", output.0);
    }
}
