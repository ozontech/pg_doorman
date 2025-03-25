use std::fmt::{self, Display};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    Sasl,
    ClearPassword,
    Jwt,
    Md5,
}

impl Display for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sasl => "SASL",
            Self::ClearPassword => "clear password",
            Self::Jwt => "JWT",
            Self::Md5 => "MD5-encrypted password",
        })
    }
}
