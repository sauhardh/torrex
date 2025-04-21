pub struct Bencode {}

impl Bencode {
    pub fn new() -> Self {
        Self {}
    }
    // value for string is encoded in the format `<length>:<contents>`. Example: "5:hello"
    // this function returns the string from encoded value
    pub fn decode_string(self, encoded_string: &str) -> &str {
        if !encoded_string.chars().next().unwrap().is_digit(10) {
            panic!("Unhandled encoded value: {}", encoded_string);
        }

        let delimiter_idx = encoded_string
            .find(":")
            .expect("incorrect encoded value passed.");

        let number = &encoded_string[0..delimiter_idx].parse::<usize>().unwrap();

        // This returns the contents
        &encoded_string[(delimiter_idx + 1)..(delimiter_idx + 1 + *number)]
    }

    // value for integer is encodedi n the format `i<integer>e`. Example: i5e or i-5e
    //
    // Note: All integer with leading zero, such as: i-0e or i04e is invalid.
    // However, i0e is valid, as it corresponds to integer `0`.
    pub fn decode_integer(self, encoded_int: &str) -> i64 {
        let i_idx = encoded_int.find("i");

        if i_idx.is_none() {
            panic!("Unhandled encoded value: {}", encoded_int);
        }

        let e_idx = encoded_int.find("e").unwrap();
        let value = &encoded_int[(i_idx.unwrap() + 1)..e_idx];

        if value.len() > 1 && (value.starts_with("0") || value.starts_with("-0")) {
            eprintln!(
                "Invalid value passed. Integer with leading zero i.e. -0 or 0<number> is incorrect \n {:?}",
                value
            );
        }

        // Returns integer
        value.parse::<i64>().unwrap()
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
        assert_eq!("hello", output);
    }

    #[test]
    fn test_decode_int() {
        let ben = Bencode::new();
        let encoded_string = "i-5e";
        let output = ben.decode_integer(encoded_string);
        assert_eq!(-5, output);
    }
}
