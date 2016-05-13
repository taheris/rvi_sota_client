use rustc_serialize::json;
use std::fs::File;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use datatype::{AccessToken, Config, Event, Error, Url, UpdateRequestId,
               UpdateReport, UpdateReportWithVin, Package,
               UpdateResultCode, UpdateState, PendingUpdateRequest};

use http_client::{Auth, HttpClient, HttpRequest};


fn vehicle_updates_endpoint(config: &Config, path: &str) -> Url {
    config.ota.server.join(& if path.is_empty() {
        format!("/api/v1/vehicle_updates/{}", config.auth.vin)
    } else {
        format!("/api/v1/vehicle_updates/{}/{}", config.auth.vin, path)
    }).unwrap()
}

pub fn download_package_update(config: &Config,
                               client: &mut HttpClient,
                               token:  &AccessToken,
                               id:     &UpdateRequestId) -> Result<PathBuf, Error> {

    let req = HttpRequest::get(
        vehicle_updates_endpoint(config, &format!("{}/download", id)),
        Some(Auth::Token(token)),
    );

    let mut path = PathBuf::new();
    path.push(&config.ota.packages_dir);
    path.push(id);
    path.set_extension(config.ota.package_manager.extension());

    let mut file = try!(File::create(path.as_path()));

    try!(client.send_request_to(&req, &mut file));

    return Ok(path)

}

pub fn send_install_report(config: &Config,
                           client: &mut HttpClient,
                           token:  &AccessToken,
                           report: &UpdateReport) -> Result<(), Error> {

    let report_with_vin = UpdateReportWithVin::new(&config.auth.vin, &report);
    let json            = try!(json::encode(&report_with_vin));

    let req = HttpRequest::post(
        vehicle_updates_endpoint(config, &format!("{}", report.update_id)),
        Some(Auth::Token(token)),
        Some(json)
    );

    let _: String = try!(client.send_request(&req));

    return Ok(())

}

pub fn get_package_updates(config: &Config,
                           client: &mut HttpClient,
                           token:  &AccessToken) -> Result<Vec<PendingUpdateRequest>, Error> {

    let req = HttpRequest::get(
        vehicle_updates_endpoint(&config, ""),
        Some(Auth::Token(token)),
    );

    let resp = try!(client.send_request(&req));

    return Ok(try!(json::decode::<Vec<PendingUpdateRequest>>(&resp)));
}

// XXX: Remove in favour of update_installed_packages()?
pub fn update_packages(config: &Config,
                     client: &mut HttpClient,
                     token:  &AccessToken,
                     pkgs:   &Vec<Package>) -> Result<(), Error> {

    info!("update_packages, pkgs: {:?}", pkgs);

    let json = try!(json::encode(&pkgs));

    let req = HttpRequest::put(
        vehicle_updates_endpoint(config, "installed"),
        Some(Auth::Token(token)),
        Some(json),
    );

    let _: String = try!(client.send_request(&req));

    return Ok(())
}

pub fn update_installed_packages(config: &Config,
                               client: &mut HttpClient,
                               token:  &AccessToken) -> Result<(), Error> {

    let pkgs = try!(config.ota.package_manager.installed_packages());
    update_packages(config, client, token, &pkgs)

}

pub fn install_package_update(config:      &Config,
                              http_client: &mut HttpClient,
                              token:       &AccessToken,
                              id:          &UpdateRequestId,
                              tx:          &Sender<Event>) -> Result<UpdateReport, Error> {

    match download_package_update(config, http_client, token, id) {

        Ok(path) => {
            info!("Downloaded at {:?}. Installing...", path);
            try!(tx.send(Event::UpdateStateChanged(id.clone(), UpdateState::Installing)));

            let p = try!(path.to_str()
                         .ok_or(Error::ParseError(format!("Path is not valid UTF-8: {:?}", path))));

            match config.ota.package_manager.install_package(p) {

                Ok((code, output)) => {
                    try!(tx.send(Event::UpdateStateChanged(id.clone(), UpdateState::Installed)));
                    try!(update_installed_packages(config, http_client, token));
                    Ok(UpdateReport::new(id.clone(), code, output))
                }

                Err((code, output)) => {
                    try!(tx.send(Event::UpdateErrored(id.clone(), format!("{:?}: {:?}", code, output))));
                    Ok(UpdateReport::new(id.clone(), code, output))
                }

            }

        }

        Err(err) => {
            try!(tx.send(Event::UpdateErrored(id.clone(), format!("{:?}", err))));
            Ok(UpdateReport::new(id.clone(),
                              UpdateResultCode::GENERAL_ERROR,
                              format!("Download failed: {:?}", err)))
        }
    }

}



#[cfg(test)]
mod tests {

    use std::fmt::Debug;
    use std::sync::mpsc::{channel, Receiver};
    use rustc_serialize::json;

    use super::*;
    use datatype::{AccessToken, Config, Event, OtaConfig, Package,
                   UpdateResultCode, UpdateState, PendingUpdateRequest};
    use http_client::TestHttpClient;
    use package_manager::PackageManager;


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

    #[test]
    fn test_update_packages_sends_authentication() {
        assert_eq!(update_packages(&Config::default(),
                                 &mut TestHttpClient::from(vec![""]),
                                 &test_token(),
                                 &vec![test_package()])
                   .unwrap(), ())
    }

    #[test]
    fn test_get_package_updates() {
        let pending_update = PendingUpdateRequest {
            id: "someid".to_string(),
            installPos: 0,
            packageId: Package {
                name: "fake-pkg".to_string(),
                version: "0.1.1".to_string()
            },
            createdAt: "2010-01-01".to_string()
        };

        let json_response = format!("[{}]",json::encode(&pending_update).unwrap());

        let updates: Vec<PendingUpdateRequest> = get_package_updates(&Config::default(),
                                       &mut TestHttpClient::from(vec![json_response.as_str()]),
                                       &test_token()).unwrap();

        let update_ids: Vec<String> = updates.iter().map(|p| p.id.clone()).collect();

        assert_eq!(update_ids, vec!["someid".to_string()])
    }

    #[test]
    #[ignore] // TODO: docker daemon requires user namespaces for this to work
    fn bad_packages_dir_download_package_update() {
        let mut config = Config::default();
        config.ota = OtaConfig { packages_dir: "/".to_string(), .. config.ota };

        assert_eq!(format!("{}", download_package_update(&config,
                                                         &mut TestHttpClient::from(vec![""]),
                                                         &test_token(),
                                                         &"0".to_string())
                           .unwrap_err()),
                   "IO error: Permission denied (os error 13)")
    }

    #[test]
    fn bad_client_download_package_update() {
        assert_eq!(format!("{}",
                           download_package_update(&Config::default(),
                                                   &mut TestHttpClient::new(),
                                                   &test_token(),
                                                   &"0".to_string())
                           .unwrap_err()),
                   "Http client error: GET http://127.0.0.1:8080/api/v1/vehicle_updates/V1234567890123456/0/download")
    }

    fn assert_receiver_eq<X: PartialEq + Debug>(rx: Receiver<X>, xs: &[X]) {

        let mut xs = xs.iter();

        while let Ok(x) = rx.try_recv() {
            if let Some(y) = xs.next() {
                assert_eq!(x, *y)
            } else {
                panic!("assert_receiver_eq: never nexted `{:?}`", x)
            }
        }

        if let Some(x) = xs.next() {
            panic!("assert_receiver_eq: never received `{:?}`", x)
        }

    }

    #[test]
    fn test_install_package_update_0() {

        let (tx, rx) = channel();

        assert_eq!(install_package_update(
            &Config::default(),
            &mut TestHttpClient::new(),
            &AccessToken::default(),
            &"0".to_string(),
            &tx).unwrap().operation_results.pop().unwrap().result_code,
                   UpdateResultCode::GENERAL_ERROR);

        assert_receiver_eq(rx, &[
            Event::UpdateErrored("0".to_string(), String::from(
                "ClientError(\"GET http://127.0.0.1:8080/api/v1/vehicle_updates/V1234567890123456/0/download\")"))])

    }

    #[test]
    fn test_install_package_update_1() {

        let mut config = Config::default();

        config.ota.packages_dir    = "/tmp/".to_string();
        config.ota.package_manager = PackageManager::File {
            filename: "test_install_package_update_1".to_string(),
            succeeds: false };

        let (tx, rx) = channel();

        assert_eq!(install_package_update(
            &config,
            &mut TestHttpClient::from(vec![""]),
            &AccessToken::default(),
            &"0".to_string(),
            &tx).unwrap().operation_results.pop().unwrap().result_code,
                   UpdateResultCode::INSTALL_FAILED);

        assert_receiver_eq(rx, &[
            Event::UpdateStateChanged("0".to_string(), UpdateState::Installing),
            // XXX: Not very helpful message?
            Event::UpdateErrored("0".to_string(), "INSTALL_FAILED: \"failed\"".to_string())])
    }

    #[test]
    fn test_install_package_update_2() {

        let mut config = Config::default();

        config.ota.packages_dir    = "/tmp/".to_string();
        config.ota.package_manager = PackageManager::File {
            filename: "test_install_package_update_2".to_string(),
            succeeds: true };

        let (tx, rx) = channel();

        assert_eq!(install_package_update(
            &config,
            &mut TestHttpClient::from(vec!["", ""]),
            &AccessToken::default(),
            &"0".to_string(),
            &tx).unwrap().operation_results.pop().unwrap().result_code,
                   UpdateResultCode::OK);

        assert_receiver_eq(rx, &[
            Event::UpdateStateChanged("0".to_string(), UpdateState::Installing),
            Event::UpdateStateChanged("0".to_string(), UpdateState::Installed)])

    }
}
