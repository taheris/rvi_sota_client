use ring::digest;
use serde_json as json;
use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use datatype::{Config, EcuCustom, EcuManifests, EcuVersion, Error, InstallResult,
               PrivateKey, OstreePackage, RoleData, RoleName, SigType, TufMeta,
               TufSigned, UptaneConfig, Url, Verified, Verifier};
use http::{Client, Response};


/// Uptane service to communicate with.
pub enum Service {
    Director,
    Repo,
}

impl Display for Service {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match *self {
            Service::Director => write!(f, "director"),
            Service::Repo     => write!(f, "repo"),
        }
    }
}


/// Software over the air updates using Uptane endpoints.
pub struct Uptane {
    pub config:   UptaneConfig,
    pub deviceid: String,
    pub verifier: Verifier,
    pub ecu_ver:  EcuVersion,
    pub privkey:  PrivateKey,
    pub sigtype:  SigType,
    pub persist:  bool,
}

impl Uptane {
    pub fn new(config: &Config) -> Result<Self, Error> {
        let private = read_file(&config.uptane.private_key_path)
            .map_err(|err| Error::Client(format!("couldn't read uptane.private_key_path: {}", err)))?;
        let priv_id = digest::digest(&digest::SHA256, &private).as_ref().iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();

        Ok(Uptane {
            config:   config.uptane.clone(),
            deviceid: format!("{}", config.device.uuid),
            verifier: Verifier::default(),
            ecu_ver:  OstreePackage::get_ecu(&config.uptane.primary_ecu_serial)?.ecu_version(None),
            privkey:  PrivateKey { keyid: priv_id, der_key: private },
            sigtype:  SigType::RsaSsaPss,
            persist:  true,
        })
    }

    /// Returns a URL based on the uptane service.
    fn endpoint(&self, service: &Service, endpoint: &str) -> Url {
        let cfg = &self.config;
        match *service {
            Service::Director => cfg.director_server.join(&format!("/{}", endpoint)),
            Service::Repo     => cfg.repo_server.join(&format!("/{}/{}", self.deviceid, endpoint))
        }
    }

    /// GET the bytes response from the given endpoint.
    fn get(&mut self, client: &Client, service: &Service, endpoint: &str) -> Result<Vec<u8>, Error> {
        let rx = client.get(self.endpoint(service, endpoint), None);
        match rx.recv().expect("couldn't GET from uptane") {
            Response::Success(data) => Ok(data.body),
            Response::Failed(data)  => Err(Error::from(data)),
            Response::Error(err)    => Err(err)
        }
    }

    /// PUT bytes to endpoint.
    fn put(&mut self, client: &Client, service: &Service, endpoint: &str, bytes: Vec<u8>) -> Result<(), Error> {
        let rx = client.put(self.endpoint(service, endpoint), Some(bytes));
        match rx.recv().expect("couldn't PUT bytes to uptane") {
            Response::Success(_)   => Ok(()),
            Response::Failed(data) => Err(Error::from(data)),
            Response::Error(err)   => Err(err)
        }
    }

    /// Read local metadata if it exists or download it otherwise.
    fn get_json(&mut self, client: &Client, service: &Service, role: &RoleName) -> Result<Vec<u8>, Error> {
        let path = format!("{}/{}/{}.json", &self.config.metadata_path, service, &role);
        if Path::new(&path).exists() {
            debug!("reading {}", path);
            read_file(&path)
        } else {
            debug!("fetching {}.json from {}", &role, service);
            self.get(client, service, &format!("{}.json", role))
        }
    }

    /// Verify the JSON buffer using the verifier's keys.
    fn verify_data(&mut self, role: RoleName, buf: &[u8]) -> Result<Verified, Error> {
        let sign = json::from_slice::<TufSigned>(buf)?;
        let data = json::from_value::<RoleData>(sign.signed.clone())?;
        let new_ver = self.verifier.verify(&role, &sign)?;
        let old_ver = self.verifier.set_version(&role, new_ver);
        Ok(Verified {
            role:    role,
            data:    data,
            old_ver: old_ver,
            new_ver: new_ver
        })
    }

    /// Fetch the root.json metadata, adding it's keys to the verifier.
    pub fn get_root(&mut self, client: &Client, service: &Service) -> Result<Verified, Error> {
        let buf   = self.get_json(client, service, &RoleName::Root)?;
        let sign  = json::from_slice::<TufSigned>(&buf)?;
        let data  = json::from_value::<RoleData>(sign.signed)?;
        let keys  = data.keys.as_ref().ok_or_else(|| Error::UptaneMissingField("keys"))?;
        let roles = data.roles.as_ref().ok_or_else(|| Error::UptaneMissingField("roles"))?;

        for (id, key) in keys {
            self.verifier.add_key(id.clone(), key.clone());
        }
        for (role, meta) in roles {
            self.verifier.add_meta(role.clone(), meta.clone());
        }

        let verified = self.verify_data(RoleName::Root, &buf)?;
        if self.persist {
            let metadir = &self.config.metadata_path;
            fs::create_dir_all(format!("{}/{}", metadir, service))?;
            write_file(&format!("{}/{}/root.json", metadir, service), &buf)?;
        }
        Ok(verified)
    }

    /// Fetch the specified role's metadata from the Director service.
    pub fn get_director(&mut self, client: &Client, role: RoleName) -> Result<Verified, Error> {
        let buf = self.get_json(client, &Service::Director, &role)?;
        self.verify_data(role, &buf)
    }

    /// Fetch the specified role's metadata from the Repo service.
    pub fn get_repo(&mut self, client: &Client, role: RoleName) -> Result<Verified, Error> {
        let buf = self.get_json(client, &Service::Repo, &role)?;
        self.verify_data(role, &buf)
    }

    /// Send a list of signed manifests to the Director server.
    pub fn put_manifest(&mut self, client: &Client, signed: Vec<TufSigned>) -> Result<(), Error> {
        let ecus = EcuManifests {
            primary_ecu_serial:   self.ecu_ver.ecu_serial.clone(),
            ecu_version_manifest: signed
        };
        let manifest = TufSigned::sign(json::to_value(ecus)?, &self.privkey, self.sigtype)?;
        Ok(self.put(client, &Service::Director, "manifest", json::to_vec(&manifest)?)?)
    }

    /// Sign a manifest with an optional installation result outcome.
    pub fn sign_manifest(&self, result: Option<InstallResult>) -> Result<TufSigned, Error> {
        let mut version = self.ecu_ver.clone();
        result.map(|result| version.custom = Some(EcuCustom { operation_result: result }));
        TufSigned::sign(json::to_value(version)?, &self.privkey, self.sigtype)
    }

    /// Extract a list of `OstreePackage`s from the targets.json metadata.
    pub fn extract_packages(targets: HashMap<String, TufMeta>, treehub: &Url) -> Vec<OstreePackage> {
        targets.iter().filter_map(|(refname, meta)| {
            meta.hashes
                .get("sha256")
                .or_else(|| { error!("couldn't get sha256 for {}", refname); None })
                .map(|commit| {
                    let ecu = &meta.custom.as_ref().expect("no custom field").ecuIdentifier;
                    OstreePackage::new(ecu.clone(), refname.clone(), commit.clone(), "".into(), treehub)
                })
        }).collect::<Vec<_>>()
    }
}


pub fn read_file(path: &str) -> Result<Vec<u8>, Error> {
    let mut file = File::open(path)
        .map_err(|err| Error::Client(format!("couldn't open {}: {}", path, err)))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|err| Error::Client(format!("couldn't read {}: {}", path, err)))?;
    Ok(buf)
}

pub fn write_file(path: &str, buf: &[u8]) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|err| Error::Client(format!("couldn't open {} for writing: {}", path, err)))?;
    let _ = file.write(&*buf)
        .map_err(|err| Error::Client(format!("couldn't write to {}: {}", path, err)))?;
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use pem;
    use std::collections::HashMap;

    use datatype::{EcuManifests, EcuVersion, TufCustom, TufMeta, TufSigned};
    use http::TestClient;


    const RSA_2048_PRIV: &'static str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDdC9QttkMbF5qB\n2plVU2hhG2sieXS2CVc3E8rm/oYGc9EHnlPMcAuaBtn9jaBo37PVYO+VFInzMu9f\nVMLm7d/hQxv4PjTBpkXvw1Ad0Tqhg/R8Lc4SXPxWxlVhg0ahLn3kDFQeEkrTNW7k\nxpAxWiE8V09ETcPwyNhPfcWeiBePwh8ySJ10IzqHt2kXwVbmL4F/mMX07KBYWIcA\n52TQLs2VhZLIaUBv9ZBxymAvogGz28clx7tHOJ8LZ/daiMzmtv5UbXPdt+q55rLJ\nZ1TuG0CuRqhTOllXnIvAYRQr6WBaLkGGbezQO86MDHBsV5TsG6JHPorrr6ogo+Lf\npuH6dcnHAgMBAAECggEBAMC/fs45fzyRkXYn4srHh14d5YbTN9VAQd/SD3zrdn0L\n4rrs8Y90KHmv/cgeBkFMx+iJtYBev4fk41xScf2icTVhKnOF8sTls1hGDIdjmeeb\nQ8ZAvs++a39TRMJaEW2dN8NyiKsMMlkH3+H3z2ZpfE+8pm8eDHza9dwjBP6fF0SP\nV1XPd2OSrJlvrgBrAU/8WWXYSYK+5F28QtJKsTuiwQylIHyJkd8cgZhgYXlUVvTj\nnHFJblpAT0qphji7p8G4Ejg+LNxu/ZD+D3wQ6iIPgKFVdC4uXmPwlf1LeYqXW0+g\ngTmHY7a/y66yn1H4A5gyfx2EffFMQu0Sl1RqzDVYYjECgYEA9Hy2QsP3pxW27yLs\nCu5e8pp3vZpdkNA71+7v2BVvaoaATnsSBOzo3elgRYsN0On4ObtfQXB3eC9poNuK\nzWxj8bkPbVOCpSpq//sUSqkh/XCmAhDl78BkgmWDb4EFEgcAT2xPBTHkb70jVAXB\nE1HBwsBcXhdxzRt8IYiBG+68d/8CgYEA53SJYpJ809lfpAG0CU986FFD7Fi/SvcX\n21TVMn1LpHuH7MZ2QuehS0SWevvspkIUm5uT3PrhTxdohAInNEzsdeHhTU11utIO\nrKnrtgZXKsBG4idsHu5ZQzp4n3CBEpfPFbOtP/UEKI/IGaJWGXVgG4J6LWmQ9LK9\nilNTaOUQ7jkCgYB+YP0B9DTPLN1cLgwf9mokNA7TdrkJA2r7yuo2I5ZtVUt7xghh\nfWk+VMXMDP4+UMNcbGvn8s/+01thqDrOx0m+iO/djn6JDC01Vz98/IKydImLpdqG\nHUiXUwwnFmVdlTrm01DhmZHA5N8fLr5IU0m6dx8IEExmPt/ioaJDoxvPVwKBgC+8\n1H01M3PKWLSN+WEWOO/9muHLaCEBF7WQKKzSNODG7cEDKe8gsR7CFbtl7GhaJr/1\ndajVQdU7Qb5AZ2+dEgQ6Q2rbOBYBLy+jmE8hvaa+o6APe3hhtp1sGObhoG2CTB7w\nwSH42hO3nBDVb6auk9T4s1Rcep5No1Q9XW28GSLZAoGATFlXg1hqNKLO8xXq1Uzi\nkDrN6Ep/wq80hLltYPu3AXQn714DVwNa3qLP04dAYXbs9IaQotAYVVGf6N1IepLM\nfQU6Q9fp9FtQJdU+Mjj2WMJVWbL0ihcU8VZV5TviNvtvR1rkToxSLia7eh39AY5G\nvkgeMZm7SwqZ9c/ZFnjJDqc=\n-----END PRIVATE KEY-----";

    fn new_uptane() -> Uptane {
        Uptane {
            config: UptaneConfig {
                director_server:    "http://localhost:8001".parse().unwrap(),
                repo_server:        "http://localhost:8002".parse().unwrap(),
                primary_ecu_serial: "test-primary-serial".into(),
                metadata_path:      "[unused]".into(),
                private_key_path:   "[unused]".into(),
                public_key_path:    "[unused]".into(),
            },
            deviceid: "uptane-test".into(),
            verifier: Verifier::default(),
            ecu_ver: OstreePackage::default().ecu_version(None),
            privkey: PrivateKey {
                keyid:   "e453c713367595e1a9e5c1de8b2c039fe4178094bdaf2d52b1993fdd1a76ee26".into(),
                der_key: pem::parse(RSA_2048_PRIV).unwrap().contents
            },
            sigtype: SigType::RsaSsaPss,
            persist: false,
        }
    }

    fn client_from_paths(paths: &[&str]) -> TestClient<Vec<u8>> {
        let mut replies = Vec::new();
        for path in paths {
            replies.push(read_file(path).expect("couldn't read file"));
        }
        TestClient::from(replies)
    }

    fn extract_custom(targets: HashMap<String, TufMeta>) -> HashMap<String, TufCustom> {
        let mut out = HashMap::new();
        for (file, meta) in targets {
            let _ = meta.custom.map(|c| out.insert(file, c));
        }
        out
    }


    #[test]
    fn test_read_manifest() {
        let bytes = read_file("tests/uptane/manifest.json").expect("couldn't read manifest.json");
        let signed = json::from_slice::<TufSigned>(&bytes).expect("couldn't load manifest");
        let mut ecus = json::from_value::<EcuManifests>(signed.signed).expect("couldn't load signed manifest");
        assert_eq!(ecus.primary_ecu_serial, "<primary_ecu_serial>");
        assert_eq!(ecus.ecu_version_manifest.len(), 1);
        let ver0 = ecus.ecu_version_manifest.pop().unwrap();
        let ecu0 = json::from_value::<EcuVersion>(ver0.signed).expect("couldn't load first manifest");
        assert_eq!(ecu0.installed_image.filepath, "<ostree_branch>-<ostree_commit>");
    }

    #[test]
    fn test_get_targets() {
        let mut uptane = new_uptane();
        let client = client_from_paths(&[
            "tests/uptane/root.json",
            "tests/uptane/targets.json",
        ]);
        assert!(uptane.get_root(&client, &Service::Director).expect("get_root").is_new());
        let verified = uptane.get_director(&client, RoleName::Targets).expect("get targets");
        assert!(verified.is_new());

        let targets = verified.data.targets.expect("missing targets");
        targets.get("/file.img").map(|meta| {
            assert_eq!(meta.length, 1337);
            let hash = meta.hashes.get("sha256").expect("sha256 hash");
            assert_eq!(hash, "dd250ea90b872a4a9f439027ac49d853c753426f71f61ae44c2f360a16179fb9");
        }).expect("get /file.img");
        let custom = extract_custom(targets);
        let image  = custom.get("/file.img").expect("get /file.img custom");
        assert_eq!(image.ecuIdentifier, "some-ecu-id");
    }

    #[test]
    fn test_get_snapshot() {
        let mut uptane = new_uptane();
        let client = client_from_paths(&[
            "tests/uptane/root.json",
            "tests/uptane/snapshot.json",
        ]);
        assert!(uptane.get_root(&client, &Service::Director).expect("couldn't get_root").is_new());
        let verified = uptane.get_director(&client, RoleName::Snapshot).expect("couldn't get snapshot");
        let metadata = verified.data.meta.as_ref().expect("missing meta");
        assert!(verified.is_new());
        let meta = metadata.get("targets.json").expect("no targets.json metadata");
        assert_eq!(meta.length, 741);
        let hash = meta.hashes.get("sha256").expect("couldn't get sha256 hash");
        assert_eq!(hash, "b10b36997574e6898dda4cfeb61c5f286d84dfa4be807950f14996cd476e6305");
    }

    #[test]
    fn test_get_timestamp() {
        let mut uptane = new_uptane();
        let client = client_from_paths(&[
            "tests/uptane/root.json",
            "tests/uptane/timestamp.json",
        ]);
        assert!(uptane.get_root(&client, &Service::Director).expect("get_root failed").is_new());
        let verified = uptane.get_director(&client, RoleName::Timestamp).expect("couldn't get timestamp");
        let metadata = verified.data.meta.as_ref().expect("missing meta");
        assert!(verified.is_new());
        let meta = metadata.get("snapshot.json").expect("no snapshot.json metadata");
        assert_eq!(meta.length, 784);
    }
}
