#[derive(Clone)]
pub struct Bencode {}

impl Bencode {
    pub fn new() -> Self {
        Self {}
    }
    // value for string is encoded as `<length>:<contents>`. Example: "5:hello"
    // this function returns the string from encoded value
    pub fn decode_string(self, encoded_string: &str) -> (&str, &str) {
        let (num, rest) = encoded_string.split_once(":").unwrap();
        let value = &rest[..(num.parse::<usize>().unwrap())];
        let rest = &encoded_string[(num.len() + 1 + value.len())..];

        (value, rest)
    }

    // value for integer is encoded as `i<integer>e`. Example: i5e or i-5e
    //
    // Note: All integer with leading zero, such as: i-0e or i04e is invalid.
    // However, i0e is valid, as it corresponds to integer `0`.
    pub fn decode_integer(self, encoded_int: &str) -> (i64, &str) {
        let (value, rest) = encoded_int.split_at(1).1.split_once("e").unwrap();

        if value.len() > 1 && (value.starts_with("-0") || value.starts_with("0")) {
            panic!(
                "Got invalid value. Integer with leading zero i.e. -0<number> or 0<number> is incorrect. \n {:?}",
                value
            );
        }

        let num = value.parse::<i64>().unwrap();

        (num, rest)
    }

    fn decoder(self, encoded_value: &str) -> (serde_json::Value, &str) {
        match encoded_value.chars().next() {
            // For string
            Some('0'..='9') => {
                let (value, rest) = self.decode_string(encoded_value);
                (value.into(), rest)
            }

            // For integer
            Some('i') => {
                let (num, rest) = self.decode_integer(encoded_value);
                (num.into(), rest)
            }
            //For List
            //
            // value for list is encoded as `l<bencoded_elements>e`. For example: l5:helloi69ee
            //
            // Note: list may contain any bencoded type including integers, strings, dictionaries, and even lists.
            // `l4:spam4:eggse` represents ["spam","eggs" ]
            // `le` represents empty list []
            Some('l') => {
                let mut rest = encoded_value.split_at(1).1;
                let mut list = Vec::new();

                while !rest.is_empty() && !rest.starts_with("e") {
                    let (value, r) = self.clone().decoder(rest);
                    list.push(value.clone());
                    rest = r
                }

                (list.into(), &rest[1..])
            }

            // if not matched
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
mod bencode_tester {

    use super::*;

    #[test]
    fn test_decode_string() {
        let ben = Bencode::new();
        let encoded_string = "5:hello";
        let output = ben.decode_string(encoded_string);
        assert_eq!("hello", output.0);
    }

    #[test]
    fn test_decode_int() {
        let ben = Bencode::new();
        let encoded_string = "i-5e";
        let output = ben.decode_integer(encoded_string);
        assert_eq!(-5, output.0);
    }

    #[test]
    fn test_decode_list() {
        let ben = Bencode::new();
        let encoded_list = "lli4eei5ee";
        let output = ben.decoder(encoded_list);
        let output_value = serde_json::to_string(&output.0).unwrap();
        assert_eq!("[[4],5]", output_value)
    }
}
