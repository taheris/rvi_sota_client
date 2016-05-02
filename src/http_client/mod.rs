pub use self::http_client::{Auth, HttpClient, HttpRequest};
pub use self::hyper::Hyper;
pub use self::test::TestHttpClient;

pub mod http_client;
pub mod hyper;
pub mod test;
