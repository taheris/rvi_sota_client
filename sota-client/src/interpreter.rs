use chan::{Sender, Receiver};
use std::cell::RefCell;
use std::process::{self, Command as ShellCommand};
use std::rc::Rc;

use authenticate::oauth2;
use datatype::{Auth, Command, Config, EcuCustom, Error, Event, InstallCode,
               InstallOutcome, InstallResult, RoleName, RequestStatus, Url};
use http::{AuthClient, Client};
use pacman::{Credentials, PacMan};
#[cfg(feature = "rvi")]
use rvi::Services;
use sota::Sota;
use uptane::Uptane;


/// An `Interpreter` loops over any incoming values, on receipt of which it
/// delegates to the `interpret` function which will respond with output values.
pub trait Interpreter<I, O> {
    fn interpret(&mut self, input: I, otx: &Sender<O>);

    fn run(&mut self, irx: Receiver<I>, otx: Sender<O>) {
        loop {
            self.interpret(irx.recv().expect("interpreter sender closed"), &otx);
        }
    }
}


/// The `EventInterpreter` listens for `Event`s and queues `Command`s for processing.
pub struct EventInterpreter {
    pub initial: bool,
    pub loop_tx: Sender<Event>,
    pub auth:    Auth,
    pub pacman:  PacMan,
    pub auto_dl: bool,
    pub sysinfo: Option<String>,
}

impl Interpreter<Event, CommandExec> for EventInterpreter {
    fn interpret(&mut self, event: Event, ctx: &Sender<CommandExec>) {
        info!("EventInterpreter received: {}", event);
        let queue = |cmd| ctx.send(CommandExec { cmd: cmd, etx: None });

        match event {
            Event::Authenticated if self.initial => {
                self.loop_tx.send(Event::InstalledPackagesNeeded);
                self.loop_tx.send(Event::SystemInfoNeeded);
                self.loop_tx.send(Event::UptaneManifestNeeded);
                self.initial = false;
            }

            Event::DownloadComplete(ref dl) if self.pacman != PacMan::Off => {
                queue(Command::StartInstall(dl.update_id));
            }

            Event::DownloadFailed(id, reason) => {
                let result = InstallResult::new(format!("{}", id), InstallCode::GENERAL_ERROR, reason);
                queue(Command::SendInstallReport(result.into_report()));
            }

            Event::InstallComplete(result) | Event::InstallFailed(result) => {
                queue(Command::SendInstallReport(result.into_report()));
            }

            Event::InstalledPackagesNeeded if self.pacman != PacMan::Off => {
                self.pacman
                    .installed_packages()
                    .map(|packages| queue(Command::SendInstalledPackages(packages)))
                    .unwrap_or_else(|err| error!("couldn't send a list of packages: {}", err));
            }

            Event::InstallReportSent(_) => {
                self.loop_tx.send(Event::InstalledPackagesNeeded);
            }

            Event::NotAuthenticated => {
                queue(Command::Authenticate(self.auth.clone()));
            }

            Event::SystemInfoNeeded => {
                self.sysinfo.as_ref().map(|_| queue(Command::SendSystemInfo));
            }

            Event::UpdatesReceived(requests) => {
                for request in requests {
                    let id = request.requestId;
                    match request.status {
                        RequestStatus::Pending if self.auto_dl => queue(Command::StartDownload(id)),
                        RequestStatus::InFlight if self.pacman == PacMan::Off => (),
                        RequestStatus::InFlight if self.pacman.is_installed(&request.packageId) => {
                            let result = InstallResult::new(format!("{}", id), InstallCode::OK, "<generated>".to_string());
                            queue(Command::SendInstallReport(result.into_report()));
                        }
                        RequestStatus::InFlight => queue(Command::StartDownload(id)),
                        _ => ()
                    }
                }
            }

            Event::UptaneInstallComplete(manifests) | Event::UptaneInstallFailed(manifests) => {
                queue(Command::UptaneSendManifest(Some(manifests)));
            }

            Event::UptaneManifestNeeded if self.pacman == PacMan::Uptane => {
                queue(Command::UptaneSendManifest(None));
            }

            Event::UptaneTargetsUpdated(targets) => {
                queue(Command::UptaneStartInstall(targets));
            }

            _ => ()
        }
    }
}


/// Wraps a `Command` for execution and (optionally) waits for the outcome `Event`.
#[derive(Debug)]
pub struct CommandExec {
    pub cmd: Command,
    pub etx: Option<Sender<Event>>,
}

/// Toggles the `CommandInterpreter`'s handling procedure.
#[derive(Clone)]
pub enum CommandMode {
    Sota,
    #[cfg(feature = "rvi")]
    Rvi(Rc<RefCell<Services>>),
    Uptane(Rc<RefCell<Uptane>>),
}

/// The `CommandInterpreter` executes the incoming `Command`, broadcasting all
/// `Event`s and (optionally) forwarding the final event to a `Receiver`.
pub struct CommandInterpreter {
    pub mode: CommandMode,
    pub config: Config,
    pub auth: Auth,
    pub http: Box<Client>,
    pub version: Option<String>,
}

impl Interpreter<CommandExec, Event> for  CommandInterpreter {
    fn interpret(&mut self, exec: CommandExec, etx: &Sender<Event>) {
        info!("CommandInterpreter received: {}", &exec.cmd);
        let event = match self.process_command(exec.cmd, etx) {
            Ok(ev) => ev,
            Err(Error::HttpAuth(resp)) => { error!("{}", resp); Event::NotAuthenticated }
            Err(err) => Event::Error(err.to_string())
        };
        exec.etx.map(|etx| etx.send(event.clone()));
        etx.send(event);
    }
}

impl CommandInterpreter {
    fn process_command(&mut self, cmd: Command, etx: &Sender<Event>) -> Result<Event, Error> {
        let event = match (cmd, self.mode.clone()) {
            (Command::Authenticate(creds @ Auth::Credentials(_)), _) => {
                let server = self.config.auth.as_ref().expect("auth config").server.join("/token");
                if self.http.is_testing() {
                    self.auth = Auth::Token(oauth2(server, &*self.http)?);
                } else {
                    self.auth = Auth::Token(oauth2(server, &AuthClient::from(creds, self.version.clone()))?);
                    self.http = Box::new(AuthClient::from(self.auth.clone(), self.version.clone()));
                }
                Event::Authenticated
            }

            (Command::Authenticate(auth), _) => {
                self.auth = auth;
                if ! self.http.is_testing() {
                    self.http = Box::new(AuthClient::from(self.auth.clone(), self.version.clone()));
                }
                Event::Authenticated
            }

            (Command::GetUpdateRequests, CommandMode::Uptane(uptane)) => {
                let mut uptane = uptane.borrow_mut();
                let _ = uptane.get_director(&*self.http, RoleName::Root)?;
                let targets = uptane.get_director(&*self.http, RoleName::Targets)?;
                if targets.is_new() {
                    Event::UptaneTargetsUpdated(Box::new(targets))
                } else {
                    Event::UptaneNoUpdates
                }
            }

            (Command::GetUpdateRequests, _) => {
                let mut sota = Sota::new(&self.config, &*self.http);
                let mut updates = sota.get_update_requests()?;
                if updates.is_empty() {
                    Event::NoUpdateRequests
                } else {
                    updates.sort_by_key(|u| u.installPos);
                    Event::UpdatesReceived(updates)
                }
            }

            (Command::ListInstalledPackages, _) => {
                Event::FoundInstalledPackages(self.config.device.package_manager.installed_packages()?)
            }

            (Command::ListSystemInfo, _) => {
                Event::FoundSystemInfo(self.system_info()?)
            }

            (Command::SendInstalledPackages(packages), _) => {
                let mut sota = Sota::new(&self.config, &*self.http);
                sota.send_installed_packages(&packages)?;
                Event::InstalledPackagesSent
            }

            #[cfg(feature = "rvi")]
            (Command::SendInstalledSoftware(sw), CommandMode::Rvi(services)) => {
                let services = services.borrow_mut();
                services.remote.lock().unwrap().send_installed_software(sw).map_err(Error::Rvi)?;
                Event::InstalledSoftwareSent
            }

            (Command::SendSystemInfo, _) => {
                let mut sota = Sota::new(&self.config, &*self.http);
                sota.send_system_info(self.system_info()?.into_bytes())?;
                Event::SystemInfoSent
            }

            #[cfg(feature = "rvi")]
            (Command::SendInstallReport(report), CommandMode::Rvi(services)) => {
                let services = services.borrow_mut();
                services.remote.lock().unwrap().send_update_report(report.clone()).map_err(Error::Rvi)?;
                Event::InstallReportSent(report)
            }

            (Command::SendInstallReport(report), _) => {
                let mut sota = Sota::new(&self.config, &*self.http);
                sota.send_install_report(&report)?;
                Event::InstallReportSent(report)
            }

            #[cfg(feature = "rvi")]
            (Command::StartDownload(id), CommandMode::Rvi(services)) => {
                let services = services.borrow_mut();
                services.remote.lock().unwrap().send_download_started(id).map_err(Error::Rvi)?;
                Event::DownloadingUpdate(id)
            }

            (Command::StartDownload(id), _) => {
                let mut sota = Sota::new(&self.config, &*self.http);
                etx.send(Event::DownloadingUpdate(id));
                sota.download_update(id)
                    .map(Event::DownloadComplete)
                    .unwrap_or_else(|err| Event::DownloadFailed(id, err.to_string()))
            }

            (Command::StartInstall(id), CommandMode::Sota) => {
                let mut sota = Sota::new(&self.config, &*self.http);
                etx.send(Event::InstallingUpdate(id));
                let result = sota.install_update(&id, &self.credentials())?;
                if result.result_code.is_success() {
                    Event::InstallComplete(result)
                } else {
                    Event::InstallFailed(result)
                }
            }

            (Command::Shutdown, _) => process::exit(0),

            (Command::UptaneSendManifest(manifests), CommandMode::Uptane(uptane)) => {
                let mut uptane = uptane.borrow_mut();
                uptane.put_manifest(&*self.http, manifests)?;
                Event::UptaneManifestSent
            }

            (Command::UptaneStartInstall(targets), CommandMode::Uptane(uptane)) => {
                let mut uptane = uptane.borrow_mut();
                match uptane.install(*targets, self.treehub()?, self.credentials()) {
                    Ok((signed, true))  => Event::UptaneInstallComplete(signed),
                    Ok((signed, false)) => Event::UptaneInstallFailed(signed),
                    Err(err) => {
                        error!("Uptane installation error: {}", err);
                        let result = InstallOutcome::error(err.to_string()).into_result(uptane.primary_ecu.clone());
                        let report = uptane.signed_report(Some(EcuCustom::from_result(result)))?;
                        Event::UptaneInstallFailed(hashmap!{ uptane.primary_ecu.clone() => report })
                    }
                }
            }

            (Command::SendInstalledSoftware(_), _) => unreachable!("Command::SendInstalledSoftware expects CommandMode::Rvi"),
            (Command::StartInstall(_), _)          => unreachable!("Command::StartInstall expects CommandMode::Sota"),
            (Command::UptaneSendManifest(_), _)    => unreachable!("Command::UptaneSendManifest expects CommandMode::Uptane"),
            (Command::UptaneStartInstall(_), _)    => unreachable!("Command::UptaneStartInstall expects CommandMode::Uptane"),
        };

        Ok(event)
    }

    /// Generate a new system information report.
    fn system_info(&self) -> Result<String, Error> {
        let cmd = self.config.device.system_info.as_ref()
            .ok_or_else(|| Error::Config("device.system_info not set".into()))?;
        ShellCommand::new(cmd)
            .output()
            .map_err(|err| Error::SystemInfo(err.to_string()))
            .and_then(|info| Ok(String::from_utf8(info.stdout)?))
    }

    /// Retrieve the current access token and device certificates for TLS.
    fn credentials(&self) -> Credentials {
        let client = Box::new(AuthClient::from(self.auth.clone(), self.version.clone()));
        let token = if let Auth::Token(ref t) = self.auth {
            Some(t.access_token.clone())
        } else {
            None
        };
        let (ca_file, cert_file, pkey_file) = if let Some(ref tls) = self.config.tls {
            (Some(tls.ca_file.clone()), Some(tls.cert_file.clone()), Some(tls.pkey_file.clone()))
        } else {
            (None, None, None)
        };
        Credentials { client, token, ca_file, cert_file, pkey_file }
    }

    /// Return the treehub URL.
    fn treehub(&self) -> Result<Url, Error> {
        self.config.tls.as_ref()
            .map(|tls| tls.server.join("/treehub"))
            .ok_or_else(|| Error::Config("tls.server required".into()))
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    use chan::{self, Sender, Receiver};
    use std::thread;
    use std::fmt::Debug;
    use uuid::Uuid;

    use datatype::{Auth, Command, Config, DownloadComplete, Event, InstallCode};
    use http::TestClient;
    use pacman::PacMan;


    fn new_interpreter(replies: Vec<Vec<u8>>, succeeds: bool) -> (Sender<Command>, Receiver<Event>) {
        let (ctx, crx) = chan::sync::<Command>(0);
        let (etx, erx) = chan::sync::<Event>(0);

        thread::spawn(move || {
            let mut config = Config::default();
            config.device.package_manager = PacMan::new_tpm(succeeds);
            let mut ci = CommandInterpreter {
                mode: CommandMode::Sota,
                config: config,
                auth: Auth::None,
                http: Box::new(TestClient::from(replies)),
                version: None,
            };
            while let Some(cmd) = crx.recv() {
                ci.interpret(CommandExec { cmd: cmd, etx: None }, &etx);
            }
        });

        (ctx, erx)
    }

    fn new_result(code: InstallCode) -> InstallResult {
        InstallResult::new(format!("{}", Uuid::default()), code, "stdout: \nstderr: \n".into())
    }

    fn assert_rx<X: PartialEq + Debug>(rx: &Receiver<X>, vals: &[X]) {
        for val in vals {
            assert_eq!(*val, rx.recv().expect(&format!("rx missing: {:?}", val)));
        }
    }

    #[test]
    fn download_updates() {
        let (ctx, erx) = new_interpreter(vec!["[]".into(); 10], true);
        ctx.send(Command::StartDownload(Uuid::default()));
        assert_rx(&erx, &[
            Event::DownloadingUpdate(Uuid::default()),
            Event::DownloadComplete(DownloadComplete {
                update_id:    Uuid::default(),
                update_image: format!("/tmp/{}", Uuid::default()),
                signature:    "".to_string()
            })
        ]);
    }

    #[test]
    fn install_update_success() {
        let (ctx, erx) = new_interpreter(vec!["[]".into(); 10], true);
        ctx.send(Command::StartInstall(Uuid::default()));
        assert_rx(&erx, &[
            Event::InstallingUpdate(Uuid::default()),
            Event::InstallComplete(new_result(InstallCode::OK)),
        ]);
    }

    #[test]
    fn install_update_failed() {
        let (ctx, erx) = new_interpreter(vec!["[]".into(); 10], false);
        ctx.send(Command::StartInstall(Uuid::default()));
        assert_rx(&erx, &[
            Event::InstallingUpdate(Uuid::default()),
            Event::InstallFailed(new_result(InstallCode::INSTALL_FAILED)),
        ]);
    }
}
