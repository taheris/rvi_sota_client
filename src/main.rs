extern crate env_logger;
extern crate getopts;
extern crate hyper;
#[macro_use] extern crate libotaplus;

use getopts::Options;
use hyper::Url;
use std::env;
use std::path::PathBuf;

use libotaplus::{config, read_interpret};
use libotaplus::config::Config;
use libotaplus::http_client::HttpClient;
use libotaplus::read_interpret::ReplEnv;
use libotaplus::auth_plus::authenticate;
use libotaplus::ota_plus::{post_packages, get_package_updates, download_package_update};
use libotaplus::package_manager::{PackageManager, Dpkg};
use libotaplus::error::Error;


fn main() {

    env_logger::init().unwrap();

    let config = build_config();

    match worker::<hyper::Client, Dpkg>(&config) {
        Err(e)    => exit!("{}", e),
        Ok(paths) => {
            println!("All good. Downloaded {:?}. See you again soon!", paths);
            println!("Installed packages were posted successfully.");
        }
    }

    if config.test.looping {
        read_interpret::read_interpret_loop(ReplEnv::new(Dpkg::new()));
    }

}

fn worker<C: HttpClient, M: PackageManager>(config: &Config) -> Result<Vec<PathBuf>, Error> {

    println!("Trying to acquire access token.");
    let token = try!(authenticate::<C>(&config.auth));

    println!("Asking package manager what packages are installed on the system.");
    let pkg_manager = M::new();
    let pkgs = try!(pkg_manager.installed_packages());

    println!("Letting the OTA server know what packages are installed.");
    try!(post_packages::<C>(&token, &config.ota, &pkgs));

    println!("Fetching possible new package updates.");
    let updates = try!(get_package_updates::<hyper::Client>(&token, &config.ota));

    let updates_len = updates.iter().len();
    println!("Got {} new updates. Downloading...", updates_len);

    let mut paths = Vec::with_capacity(updates_len);

    for update in &updates {
        let path = try!(download_package_update::<C>(&token, &config.ota, update)
                        .map_err(|e| Error::ClientError(
                            format!("Couldn't download update {:?}: {}", update, e))));
        paths.push(path);
    }

    return Ok(paths)

}

fn build_config() -> Config {

    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();
    opts.optflag("h", "help",
                 "print this help menu");
    opts.optopt("", "config",
                "change config path", "PATH");
    opts.optopt("", "auth-server",
                "change the auth server URL", "URL");
    opts.optopt("", "auth-client-id",
                "change auth client id", "ID");
    opts.optopt("", "auth-secret",
                "change auth secret", "SECRET");
    opts.optopt("", "ota-server",
                "change ota server URL", "URL");
    opts.optopt("", "ota-vin",
                "change ota vin", "VIN");
    opts.optopt("", "ota-packages-dir",
                "change downloaded directory for packages", "PATH");
    opts.optflag("", "test-looping",
                 "enable read-interpret test loop");
    opts.optflag("", "test-fake-pm",
                 "enable fake package manager for testing");

    let matches = opts.parse(&args[1..])
        .unwrap_or_else(|err| panic!(err.to_string()));

    if matches.opt_present("h") {
        let brief = format!("Usage: {} [options]", program);
        exit!("{}", opts.usage(&brief));
    }

    let mut config_file = env::var("OTA_PLUS_CLIENT_CFG")
        .unwrap_or("/opt/ats/ota/etc/ota.toml".to_string());

    if let Some(path) = matches.opt_str("config") {
        config_file = path;
    }

    let mut config = config::load_config(&config_file)
        .unwrap_or_else(|err| exit!("{}", err));

    if let Some(s) = matches.opt_str("auth-server") {
        match Url::parse(&s) {
            Ok(url)  => config.auth.server = url,
            Err(err) => exit!("Invalid auth-server URL: {}", err)
        }
    }

    if let Some(client_id) = matches.opt_str("auth-client-id") {
        config.auth.client_id = client_id;
    }

    if let Some(secret) = matches.opt_str("auth-secret") {
        config.auth.secret = secret;
    }

    if let Some(s) = matches.opt_str("ota-server") {
        match Url::parse(&s) {
            Ok(url)  => config.ota.server = url,
            Err(err) => exit!("Invalid ota-server URL: {}", err)
        }
    }

    if let Some(vin) = matches.opt_str("ota-vin") {
        config.ota.vin = vin;
    }

    if let Some(path) = matches.opt_str("ota-packages-dir") {
        config.ota.packages_dir = path;
    }

    if matches.opt_present("test-looping") {
        config.test.looping = true;
    }

    if matches.opt_present("test-fake-pm") {
        config.test.fake_package_manager = true;
    }

    return config
}

// Hack to build a binary with a predictable path for use in tests/. We
// can remove this when https://github.com/rust-lang/cargo/issues/1924
// is resolved.
#[test]
fn build_binary() {
    let output = std::process::Command::new("cargo")
        .arg("build")
        .output()
        .unwrap_or_else(|e| panic!("failed to execute child: {}", e));

    assert!(output.status.success())
}
