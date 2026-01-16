#[cfg(test)]
mod fuzz_tests {
    use super::super::protocol::{parse_params, parse_startup};
    use bytes::{BufMut, BytesMut};

    #[test]
    fn fuzz_parse_params_basic() {
        // Valid case: key-value pair
        let mut bytes = BytesMut::new();
        bytes.put_slice(b"user\0postgres\0");
        assert!(parse_params(bytes).is_ok());
    }

    #[test]
    fn fuzz_parse_params_empty() {
        let bytes = BytesMut::new();
        let result = parse_params(bytes);
        assert!(result.is_err());
    }

    #[test]
    fn fuzz_parse_params_odd_count() {
        // Odd number of parameters (invalid)
        let mut bytes = BytesMut::new();
        bytes.put_slice(b"user\0postgres\0key\0");
        let result = parse_params(bytes);
        assert!(result.is_err());
    }

    #[test]
    fn fuzz_parse_startup_no_user() {
        // Missing required 'user' parameter
        let mut bytes = BytesMut::new();
        bytes.put_slice(b"database\0testdb\0");
        let result = parse_startup(bytes);
        assert!(result.is_err());
    }

    #[test]
    fn fuzz_parse_startup_with_user() {
        let mut bytes = BytesMut::new();
        bytes.put_slice(b"user\0postgres\0database\0testdb\0");
        let result = parse_startup(bytes);
        assert!(result.is_ok());
    }
}
