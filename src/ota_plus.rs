use hyper::header::{Authorization, Bearer, ContentType};
use hyper::mime::{Mime, TopLevel, SubLevel, Attr, Value};
use hyper::Url;
use rustc_serialize::json;
use std::result::Result;

use access_token::AccessToken;
use config::{OtaConfig, PackagesConfig};
use error::{Error, ClientReason};
use http_client::{HttpClient, HttpRequest};
use package::Package;
use update_request::UpdateRequestId;

use std::fs::File;
use std::path::PathBuf;

fn vehicle_endpoint(config: OtaConfig, s: &str) -> Url {
    config.server.join(&format!("/api/v1/vehicles/{}{}", config.vin, s)).unwrap()
}

pub fn download_package_update<C: HttpClient>(token: AccessToken,
                                              config: OtaConfig,
                                              pkgs_config: PackagesConfig,
                                              id: &UpdateRequestId) -> Result<PathBuf, Error> {
    let http_client = C::new();

    let req = HttpRequest::get(vehicle_endpoint(config, &format!("/updates/{}", id)))
        .with_header(Authorization(Bearer { token: token.access_token.clone() }));

    let p = format!("{}/{}.deb", pkgs_config.dir, id);
    let path = PathBuf::from(p);
    let file = try!(File::create(path.as_path()).map_err(|e| Error::ClientErrorWithReason(ClientReason::Io(e))));

    http_client.send_request_to(&req, file).map(move |_| path)
}

pub fn get_package_updates<C: HttpClient>(token: AccessToken,
                                          config: OtaConfig) -> Result<Vec<UpdateRequestId>, Error> {
    let http_client = C::new();

    let req = HttpRequest::get(vehicle_endpoint(config, "/updates"))
        .with_header(Authorization(Bearer { token: token.access_token.clone() }));
    http_client.send_request(&req)
        .map_err(|e| Error::ClientError(format!("Can't consult package updates: {}", e)))
        .and_then(|body| {
            json::decode::<Vec<UpdateRequestId>>(&body)
                .map_err(|e| Error::ParseError(format!("Cannot parse response: {}. Got: {}", e, &body)))
        })
}

pub fn post_packages<C: HttpClient>(token: AccessToken,
                                    config: OtaConfig,
                                    pkgs: Vec<Package>) -> Result<(), Error> {

    let http_client = C::new();
    json::encode(&pkgs)
        .map_err(|_| Error::ParseError(String::from("JSON encoding error")))
        .and_then(|json| {
            let req = HttpRequest::post(vehicle_endpoint(config, "/packages"))
                .with_header(Authorization(Bearer { token: token.access_token.clone() }))
                .with_header(ContentType(Mime(
                    TopLevel::Application,
                    SubLevel::Json,
                    vec![(Attr::Charset, Value::Utf8)])))
                .with_body(&json);

            http_client.send_request(&req).map(|_| ())
        })
}

#[cfg(test)]
mod tests {

    use super::*;
    use http_client::{HttpRequest, HttpClient};
    use error::Error;
    use package::Package;
    use config::OtaConfig;
    use access_token::AccessToken;

    use std::io::Write;

    fn test_token() -> AccessToken {
        AccessToken {
            access_token: "token".to_string(),
            token_type: "bar".to_string(),
            expires_in: 20,
            scope: vec![]
        }
    }

    fn test_package() -> Package {
        Package {
            name: "hey".to_string(),
            version: "1.2.3".to_string()
        }
    }

    struct MockClient;

    impl HttpClient for MockClient {

        fn new() -> MockClient {
            MockClient
        }

        fn send_request(&self, _: &HttpRequest) -> Result<String, Error> {
            return Ok("[\"pkgid\"]".to_string())
        }

        fn send_request_to<W: Write>(&self, _: &HttpRequest, _: W) -> Result<(), Error> {
            return Ok(())
        }

    }

    #[test]
    fn test_post_packages_sends_authentication() {
        assert_eq!(
            post_packages::<MockClient>(test_token(), OtaConfig::default(), vec![test_package()])
                .unwrap(), ())
    }


    #[test]
    fn test_get_package_updates() {
        assert_eq!(get_package_updates::<MockClient>(test_token(), OtaConfig::default()).unwrap(),
                   vec!["pkgid".to_string()])
    }
}
