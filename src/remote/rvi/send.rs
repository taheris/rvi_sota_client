//! Helper functions for sending messages to RVI.

use std::io::Read;
use hyper::Client;
use hyper::header::ContentType;
use hyper::mime::{Mime, TopLevel, SubLevel};
use rustc_serialize::{json, Encodable};
use url::Url;

use remote::jsonrpc;
use remote::rvi::message::RVIMessage;

/// Send a object to RVI. Either returns the full response from RVI or a error message.
///
/// The object will get encoded to json. Apart from that no sanity checks are made. You usually
/// don't need this function.
///
/// # Arguments
/// * `url`: The full URL where RVI can be reached.
/// * `b`: The object to encode and send to RVI.
pub fn send<E: Encodable>(url: Url, b: &E) -> Result<String, String> {
    /* TODO: Re-implement with async hyper
    let client = Client::new();

    let mut resp = try!(json::encode(b)
        .map_err(|e| format!("{}", e))
        .and_then(|j| {
            debug!("<<< Sent Message: {}", j);
            client.post(url)
                .header(ContentType(Mime(TopLevel::Application, SubLevel::Json, vec![])))
                .body(&j)
                .send()
                .map_err(|e| format!("{}", e))
        }));

    let mut rbody = String::new();
    try!(resp.read_to_string(&mut rbody)
         .map_err(|e| format!("{}", e)));
    debug!(">>> Received Response: {}", rbody);
    Ok(rbody)
    */
    Ok("".into())
}

/// Prepare a message and send it to RVI. Returns the full response from RVI on success or a error
/// message on failure.
///
/// This wraps the provided object into a proper RVI message and encodes it to json. You usually
/// should call this function.
///
/// **NOTE:** This currently implements a workaround for RVI, that will get fixed in the upcoming
/// RVI version `0.5.0`, which will break this current implementation. For the new protocol you
/// don't have to wrap the `params` in a one element `Vector` any more.
///
/// # Arguments
/// * `url`: The full URL where RVI can be reached.
/// * `b`: The object to wrap into a RVI Message, encode and send to RVI.
/// * `addr`: The full RVI address (service URL) where this message should be sent to.
#[cfg(not(test))]
pub fn send_message<E: Encodable>(url: Url, b: E, addr: &str) -> Result<String, String> {
    let mut params = Vec::new();
    params.push(b);
    let message = RVIMessage::<E>::new(addr, params, 90);
    let json_rpc = jsonrpc::Request::new("message", message);
    send(url, &json_rpc)
}

#[cfg(test)]
use std::fmt::Display;

#[cfg(test)]
pub fn send_message<U: IntoUrl + Display, E: Encodable>(url: U, _: E, addr: &str) -> Result<String, String> {
    Ok(format!("Faked sending to RVI: {}, {}", url, addr))
}
