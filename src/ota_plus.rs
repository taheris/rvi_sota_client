use hyper::client::Response;
use hyper::header::{Authorization, Bearer, ContentType};
use hyper::mime::{Mime, TopLevel, SubLevel, Attr, Value};
use hyper;
use rustc_serialize::json;
use std::io::Read;
use std::result::Result;

use auth_plus::AccessToken;
use config::OtaConfig;
use error::Error;
use package::Package;


pub struct Client {
    hclient: hyper::Client,
    access_token: String,
    config: OtaConfig
}

impl Client {

    pub fn new(token: AccessToken, config: OtaConfig) -> Client {
        Client {
            hclient: hyper::Client::new(),
            access_token: token.access_token,
            config: config
        }
    }

    pub fn check_for_update(&self) -> Result<String, Error> {
        self.hclient.get(self.config.server.join("/updates").unwrap())
            .header(Authorization(Bearer { token: self.access_token.clone() }))
            .send()
            .map_err(|e| Error::ClientError(format!(
                "Cannot send check_for_update request: {}", e)))
            .and_then(|mut resp| {
                let mut rbody = String::new();
                resp.read_to_string(&mut rbody)
                    .map_err(|e| Error::ClientError(format!(
                        "Cannot read check_for_update response: {}", e)))
                    .and_then(|_| Ok(rbody))
            })
    }

    pub fn post_packages(&self, pkgs: Vec<Package>) -> Result<Response, Error> {
        json::encode(&pkgs)
            .map_err(|_| Error::ParseError(String::from("JSON encoding error")))
            .and_then(|json| {
                self.hclient.put(self.config.server.join("/packages").unwrap())
                    .header(Authorization(Bearer { token: self.access_token.clone() }))
                    .header(ContentType(Mime(
                        TopLevel::Application,
                        SubLevel::Json,
                        vec![(Attr::Charset, Value::Utf8)])))
                    .body(&json)
                    .send()
                    .map_err(|e| Error::ClientError(format!("Cannot send packages: {}", e)))
            })
    }

}
