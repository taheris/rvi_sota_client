use std::fmt::{self, Debug, Display, Formatter};


/// An enumeration of all authentication types.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Auth {
    None,
    Certificate,
    Credentials(ClientCredentials),
    Token(AccessToken),
}

/// Display should not include any sensitive data for log output.
impl Display for Auth {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match *self {
            Auth::None           => write!(f, "Auth::None"),
            Auth::Token(_)       => write!(f, "Auth::Token"),
            Auth::Certificate    => write!(f, "Auth::Certificate"),
            Auth::Credentials(_) => write!(f, "Auth::Credentials"),
        }
    }
}

impl Debug for Auth {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self)
    }
}


/// Encapsulates the client id and secret used during authentication.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ClientCredentials {
    pub client_id:     String,
    pub client_secret: String,
}

/// Stores the returned access token data following a successful authentication.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct AccessToken {
    pub access_token: String,
    pub token_type:   String,
    pub expires_in:   i32,
    pub scope:        String
}
