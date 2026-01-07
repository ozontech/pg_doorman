// AuthenticationMD5Password
pub const MD5_ENCRYPTED_PASSWORD: i32 = 5;

// SASL
pub const SASL: i32 = 10;
pub const SASL_CONTINUE: i32 = 11;
pub const SASL_FINAL: i32 = 12;
pub const SCRAM_SHA_256: &str = "SCRAM-SHA-256";
pub const MD5_PASSWORD_PREFIX: &str = "md5";
pub const JWT_PUB_KEY_PASSWORD_PREFIX: &str = "jwt-pkey-fpath:";
pub const JWT_PRIV_KEY_PASSWORD_PREFIX: &str = "jwt-priv-key-fpath:";
pub const NONCE_LENGTH: usize = 24;

pub const TALOS_USERNAME: &str = "talos";

// ErrorResponse: A code identifying the field type; if zero, this is the message terminator and no string follows.
pub const MESSAGE_TERMINATOR: u8 = 0;

// AuthenticationOk
pub const AUTHENTICATION_SUCCESSFUL: i32 = 0;
// AuthenticationCleartextPassword
pub const AUTHENTICATION_CLEAR_PASSWORD: i32 = 3;

// Used in the StartupMessage to indicate regular handshake.
pub const PROTOCOL_VERSION_NUMBER: i32 = 196608;

// SSLRequest: used to indicate we want an SSL connection.
pub const SSL_REQUEST_CODE: i32 = 80877103;

// CancelRequest: the cancel request code.
pub const CANCEL_REQUEST_CODE: i32 = 80877102;

pub const REQUEST_GSSENCMODE_CODE: i32 = 80877104;
