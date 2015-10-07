use super::package_id::PackageId;
use rvi::Service;

#[derive(RustcEncodable)]
pub struct InitiateParams {
    pub packages: Vec<PackageId>,
    pub services: LocalServices,
    pub vin: String
}

#[derive(RustcEncodable)]
pub struct LocalServices {
    pub start: String,
    pub chunk: String,
    pub finish: String,
    pub report: String
}

impl LocalServices {
    pub fn new(s: &Vec<Service>) -> LocalServices {
        let mut services = LocalServices {
            start: "".to_string(),
            chunk: "".to_string(),
            finish: "".to_string(),
            report: "".to_string()
        };

        for service in s {
            let serv = &mut services;
            match service.name.as_ref() {
                "/sota/start" => { serv.start = service.addr.clone() },
                "/sota/chunk" => { serv.chunk = service.addr.clone() },
                "/sota/finish" => { serv.finish = service.addr.clone() },
                "/sota/report" => { serv.report = service.addr.clone() },
                _ => {}
            }
        }

        services
    }

    pub fn get_vin(&self) -> String {
        // TODO: rather match by regex, than on a specific url part
        self.start.split("/").nth(2).unwrap().to_string()
    }
}

impl InitiateParams {
    pub fn new(p: Vec<PackageId>,
               s: &Vec<Service>) -> InitiateParams {
        let services = LocalServices::new(s);
        let vin = services.get_vin();

        InitiateParams {
            packages: p,
            services: services,
            vin: vin
        }
    }
}
