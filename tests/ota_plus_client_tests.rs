extern crate tempfile;

use std::io::Write;
use std::process::Command;
use std::vec::Vec;
use tempfile::NamedTempFile;

fn client(args: &[&str]) -> String {
    let output = Command::new("target/debug/ota_plus_client")
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to execute child: {}", e));
    return String::from_utf8(output.stdout).unwrap()
}

fn client_with_config(args: &[&str], cfg: &str) -> String {
    let mut file = NamedTempFile::new().unwrap();
    let _ = file.write_all(cfg.as_bytes()).unwrap();

    let     arg:  String    = "--config=".to_string() + file.path().to_str().unwrap();
    let mut args: Vec<&str> = args.to_vec();

    args.push(&arg);
    client(&args)
}

#[test]
fn help() {

    assert_eq!(client(&["-h"]),
               r#"Usage: target/debug/ota_plus_client [options]

Options:
    -h, --help          print this help menu
        --config PATH   change config path
        --auth-server URL
                        change the auth server URL
        --auth-client-id ID
                        change auth client id
        --auth-secret SECRET
                        change auth secret
        --ota-server URL
                        change ota server URL
        --ota-vin VIN   change ota vin
        --test-looping  enable read-interpret test loop

"#);

}

#[test]
fn bad_auth_server_url() {
    assert_eq!(client(&["--auth-server", "apa"]),
               "Invalid auth-server URL: relative URL without a base\n")
}

#[test]
fn bad_ota_server_url() {
    assert_eq!(client(&["--ota-server", "apa"]),
               "Invalid ota-server URL: relative URL without a base\n")
}

#[test]
fn no_auth_server_to_connect_to() {
    assert_eq!(client(&[""]),
               "Authentication error, Can\'t get AuthPlus token: Cannot send request: connection refused\n")
}

static BAD_SECTION_CONFIG: &'static str =
    r#"
    [uth]
    server = "http://127.0.0.1:9000"
    client_id = "client-id"
    secret = "secret"

    [ota]
    server = "http://127.0.0.1:8080"
    vin = "V1234567890123456"

    [test]
    looping = false
    "#;

#[test]
fn bad_section() {
    assert_eq!(client_with_config(&[""], BAD_SECTION_CONFIG),
               "Failed to parse config: invalid section: auth\n")
}

#[test]
fn bad_toml() {
    assert_eq!(client_with_config(&[""], "auth]"),
               "Failed to parse config: invalid toml\n")
}

#[test]
fn bad_path_dir() {
    assert_eq!(client(&["--config=/"]),
               "Failed to load config: Is a directory (os error 21)\n")
}