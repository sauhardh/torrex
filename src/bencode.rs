use std::fmt;

#[derive(PartialEq)]
pub enum ListValue {
    Int(i64),
    Str(String),
}

impl fmt::Debug for ListValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ListValue::Int(i) => write!(f, "{}", i),
            ListValue::Str(s) => write!(f, "{}", s),
        }
    }
}

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
                "Invalid value passed. Integer with leading zero i.e. -0 or 0<number> is incorrect \n {:?}",
                value
            );
        }

        let num = value.parse::<i64>().unwrap();
        (num, rest)
    }

    // value for list is encoded as `l<bencoded_elements>e`. For example: l5:helloi69ee
    //
    // Note: list may contain any bencoded type including integers, strings, dictionaries, and even lists.
    // `l4:spam4:eggse` represents ["spam","eggs" ]
    // `le` represents empty list []
    // pub fn decode_list(self, encoded_list: &str) -> Vec<ListValue> {
    //     let l_idx = encoded_list.find("l");
    //     if l_idx.is_none() {
    //         eprintln!(
    //             "Unhandled encoded value. It is not a list type: {}",
    //             encoded_list
    //         )
    //     }

    //     let mut bencoded_elements = &encoded_list[(l_idx.unwrap() + 1)..(encoded_list.len() - 1)];
    //     let mut list: Vec<ListValue> = Vec::new();

    //     println!("bencoded element at first : {:?}", bencoded_elements);

    //     loop {
    //         if bencoded_elements.len() <= 2 {
    //             break;
    //         }

    //         if bencoded_elements.chars().next().unwrap().is_digit(10) {
    //             let string = self.clone().decode_string(bencoded_elements);
    //             list.push(ListValue::Str(string.0.to_string()));
    //             println!("Got string: {:?}", string);

    //             bencoded_elements =
    //                 &bencoded_elements[(string.0.len()).to_string().len() + 1 + string.0.len()..];

    //             println!("after string bencoded_elements is: {:?}", bencoded_elements);
    //         }

    //         if bencoded_elements.starts_with("i") {
    //             let (integer, length) = self.clone().decode_integer(bencoded_elements);
    //             println!("Got int, len: {:?} {:?}", integer, length);

    //             list.push(ListValue::Int(integer));
    //             bencoded_elements = &bencoded_elements[length..];
    //             println!("after bencoded_elements is: {:?}", bencoded_elements);
    //         }
    //     }

    //     list
    // }

    fn decoder(self, encoded_value: &str) {
        match encoded_value.chars().next() {
            // For string
            Some('0'..='9') => {
                let (value, rest) = self.decode_string(encoded_value);
                println!("Got from string parser: {:?} and {:?}", value, rest);
            }

            // For integer
            Some('i') => {}
            //For List
            Some('l') => {}

            // it not matched
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

    // #[test]
    // fn test_decode_list() {
    //     let ben = Bencode::new();
    //     let encoded_list = "l5:helloi52ee";
    //     let output = ben.decode_list(encoded_list);
    //     println!("holy output: {:?}", output);
    // }
}

// fn bencode_decoder(encoded_value: &str) -> (, &str) {
//     match encoded_value.chars().next() {
//         // For string
//         // value for string is encoded as `<length>:<contents>`. Example: "5:hello"
//         // this function returns the string from encoded value
//         // Imagine this as 5:hello i52e5:world
//         Some('0'..='9') => {
//             let (num, rest) = encoded_value.split_once(":").unwrap();
//             let integer = num.parse::<usize>().unwrap();
//             let value = &rest[..integer];
//             (value, &encoded_value[(num.len() + 1 + value.len())..])
//         }

//         // For integer
//         Some('i') => {}
//         //For List
//         Some('l') => {}

//         // it not matched
//         _ => {
//             panic!(
//                 "Unhandled encoded value. Could not parse: {:?}",
//                 encoded_value
//             );
//         }
//     }
// }
