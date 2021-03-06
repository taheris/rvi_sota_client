use chan::{Sender, Receiver};
use dbus::{self, BusType, Connection, Message, MessageItem, NameFlag, Signature};
use dbus::arg::{Arg, ArgType, Get, Iter};
use dbus::tree::{Argument, Factory};
use std::thread;
use std::convert::From;
use std::str::FromStr;
use uuid::Uuid;

use datatype::{Command, DBusConfig, Event, InstalledFirmware, InstalledPackage,
               InstallResult, InstalledSoftware, InstallReport};
use gateway::Gateway;
use interpreter::CommandExec;


/// The `DBus` gateway is used with the RVI module for communicating with the
/// system session bus.
#[derive(Clone)]
pub struct DBus {
    pub cfg: DBusConfig
}

impl Gateway for DBus {
    fn start(&mut self, ctx: Sender<CommandExec>, erx: Receiver<Event>) {
        info!("Starting DBus gateway.");

        let cfg  = self.cfg.clone();
        let conn = Connection::get_private(BusType::Session).expect("couldn't get dbus session bus");
        conn.register_name(&cfg.name, NameFlag::ReplaceExisting as u32).expect("couldn't register name");

        let arg0 = Argument::new(Some("update_id".into()), Signature::new("s").expect("arg1 signature"));
        let arg1 = arg0.clone();
        let arg2 = Argument::new(Some("operations_results".into()), Signature::new("aa{sv}").expect("arg2 signature"));
        let ctx1 = ctx.clone();
        let ctx2 = ctx.clone();

        let fact = Factory::new_fn::<()>();
        let tree = fact.tree(()).add(
            fact.object_path(cfg.path, ()).introspectable().add(
                fact.interface(cfg.interface, ())
                    .add_m(fact.method("initiateDownload", (), move |info| {
                        debug!("dbus initiateDownload called: {:?}", info);
                        let uuid = Uuid::from_str(info.msg.read1()?)
                            .map_err(|err| dbus::Error::new_custom("read1", &format!("{}", err)))?;
                        ctx1.send(CommandExec { cmd: Command::StartDownload(uuid), etx: None });
                        Ok(Vec::new())
                    }).in_arg(arg0))

                    .add_m(fact.method("updateReport", (), move |info| {
                        debug!("dbus updateReport called: {:?}", info);
                        let (id, res): (String, Vec<InstallResult>) = info.msg.read2()?;
                        let report = InstallReport::new(id, res);
                        ctx2.send(CommandExec { cmd: Command::SendInstallReport(report), etx: None });
                        Ok(Vec::new())
                    }).in_arg(arg1).in_arg(arg2))));

        let session_cfg = self.cfg.clone();
        let session_ctx = ctx.clone();
        thread::spawn(move || {
            let session = Session::new(session_ctx, session_cfg);
            loop {
                session.handle_event(erx.recv().expect("dbus etx closed"))
            }
        });

        tree.set_registered(&conn, true).expect("couldn't set registered");
        for _ in tree.run(&conn, conn.iter(1000)) {}
    }
}


struct Session {
    conn:    Connection,
    ctx:     Sender<CommandExec>,
    dest:    String,
    path:    String,
    iface:   String,
    timeout: i32,
}

impl Session {
    fn new(ctx: Sender<CommandExec>, cfg: DBusConfig) -> Self {
        Session {
            conn:    Connection::get_private(BusType::Session).expect("couldn't get session bus"),
            ctx:     ctx,
            dest:    cfg.software_manager.clone(),
            path:    cfg.software_manager_path.clone(),
            iface:   cfg.software_manager.clone(),
            timeout: cfg.timeout,
        }
    }

    fn send_async(&self, msg: Message) {
        let _ = self.conn.send(msg).map_err(|err| error!("couldn't send dbus message: {:?}", err));
    }

    fn send_sync(&self, msg: Message) -> Result<Message, dbus::Error> {
        self.conn.send_with_reply_and_block(msg, self.timeout)
    }

    fn send_internal(&self, cmd: Command) {
        self.ctx.send(CommandExec { cmd: cmd, etx: None });
    }

    fn new_message(&self, method: &str, args: &[MessageItem]) -> Message {
        let mut msg = Message::new_method_call(&self.dest, &self.path, &self.iface, method).expect("new dbus message");
        msg.append_items(args);
        msg
    }

    fn handle_event(&self, event: Event) {
        match event {
            Event::UpdateAvailable(avail) => {
                let msg = self.new_message("updateAvailable", &[
                    MessageItem::from(avail.update_id),
                    MessageItem::from(avail.signature),
                    MessageItem::from(avail.description),
                    MessageItem::from(avail.request_confirmation)
                ]);
                self.send_async(msg);
            }

            Event::DownloadComplete(comp) => {
                let msg = self.new_message("downloadComplete", &[
                    MessageItem::from(comp.update_image),
                    MessageItem::from(comp.signature)
                ]);
                self.send_async(msg);
            }

            Event::InstalledSoftwareNeeded => {
                let msg = self.new_message("getInstalledPackages", &[
                    MessageItem::from(true), // include packages?
                    MessageItem::from(false) // include firmware?
                ]);
                self.send_sync(msg)
                    .map(|reply| reply.read2()
                         .map(|(pkgs, firms): (Vec<InstalledPackage>, Vec<InstalledFirmware>)| {
                             let inst = InstalledSoftware::new(pkgs, firms);
                             self.send_internal(Command::SendInstalledSoftware(inst));
                         })
                         .unwrap_or_else(|err| error!("failed to send SendInstalledSoftware: {}", err))
                    )
                    .unwrap_or_else(|err| error!("failed to send InstalledSoftwareNeeded: {}", err));
            }

            _ => ()
        }
    }
}


// FIXME: parsing implementations
impl Arg for InstallResult {
    fn arg_type() -> ArgType { ArgType::Variant }
    fn signature() -> Signature<'static> { unsafe { Signature::from_slice_unchecked(b"v\0") } }
}

impl Arg for InstalledPackage {
    fn arg_type() -> ArgType { ArgType::Variant }
    fn signature() -> Signature<'static> { unsafe { Signature::from_slice_unchecked(b"v\0") } }
}

impl Arg for InstalledFirmware {
    fn arg_type() -> ArgType { ArgType::Variant }
    fn signature() -> Signature<'static> { unsafe { Signature::from_slice_unchecked(b"v\0") } }
}

impl<'a> Get<'a> for InstallResult {
    fn get(_: &mut Iter<'a>) -> Option<Self> {
        None
    }
}

impl<'a> Get<'a> for InstalledPackage {
    fn get(_: &mut Iter<'a>) -> Option<Self> {
        None
    }
}

impl<'a> Get<'a> for InstalledFirmware {
    fn get(_: &mut Iter<'a>) -> Option<Self> {
        None
    }
}
