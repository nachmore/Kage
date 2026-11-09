//! Linux: an item in the Secret Service's default collection over D-Bus
//! (GNOME Keyring, KWallet's Secret Service, KeePassXC) (docs/design/SYNC.md
//! §11.4). Attributes `service` and `account` find it; its label is the
//! service name.
//!
//! The secret travels to the service in the `plain` session algorithm, over
//! the user's own session bus. Kage never shows a Secret Service prompt: a
//! locked collection fails with `SecretError::Unavailable`, the engine says
//! sync is locked, and Settings → Sync offers the file key store.

use crate::os::secret_store::{SecretBackend, SecretError};
use std::collections::HashMap;
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zeroize::Zeroizing;

pub const BACKEND: SecretBackend = SecretBackend::SecretService;

const DEST: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
const ITEM_IFACE: &str = "org.freedesktop.Secret.Item";
const PROMPT_IFACE: &str = "org.freedesktop.Secret.Prompt";
/// How long the Secret Service has to answer the reliability probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// `(session, parameters, value, content type)`, the `(oayays)` struct.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

fn unavailable(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(SecretError::Unavailable).context(format!("Secret Service: {}", e))
}

fn is_none_path(p: &ObjectPath<'_>) -> bool {
    p.as_str() == "/"
}

struct Session {
    conn: Connection,
    path: OwnedObjectPath,
}

impl Session {
    fn open() -> anyhow::Result<Self> {
        let conn = Connection::session().map_err(unavailable)?;
        let (_, path): (OwnedValue, OwnedObjectPath) = service(&conn)?
            .call("OpenSession", &("plain", Value::from("")))
            .map_err(unavailable)?;
        Ok(Self { conn, path })
    }

    fn proxy<'a>(&self, path: &'a ObjectPath<'a>, iface: &'a str) -> anyhow::Result<Proxy<'a>> {
        Proxy::new(&self.conn, DEST, path.clone(), iface).map_err(unavailable)
    }

    /// Items with our attributes: (unlocked, locked).
    fn search(
        &self,
        service_name: &str,
        account: &str,
    ) -> anyhow::Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)> {
        service(&self.conn)?
            .call("SearchItems", &(attributes(service_name, account),))
            .map_err(unavailable)
    }

    /// Unlock `objects` if that needs no prompt. A prompt is dismissed and
    /// the call fails: Kage doesn't show one.
    fn unlock(&self, objects: &[OwnedObjectPath]) -> anyhow::Result<Vec<OwnedObjectPath>> {
        let (unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = service(&self.conn)?
            .call("Unlock", &(objects,))
            .map_err(unavailable)?;
        if !is_none_path(&prompt) {
            self.dismiss(&prompt);
            return Err(unavailable("the collection is locked"));
        }
        Ok(unlocked)
    }

    fn dismiss(&self, prompt: &ObjectPath<'_>) {
        if let Ok(p) = self.proxy(prompt, PROMPT_IFACE) {
            let _: Result<(), _> = p.call("Dismiss", &());
        }
    }

    fn close(self) {
        if let Ok(p) = Proxy::new(
            &self.conn,
            DEST,
            self.path.as_ref(),
            "org.freedesktop.Secret.Session",
        ) {
            let _: Result<(), _> = p.call("Close", &());
        }
    }
}

fn service(conn: &Connection) -> anyhow::Result<Proxy<'static>> {
    Proxy::new(conn, DEST, SERVICE_PATH, SERVICE_IFACE).map_err(unavailable)
}

fn attributes<'a>(service: &'a str, account: &'a str) -> HashMap<&'a str, &'a str> {
    HashMap::from([("service", service), ("account", account)])
}

pub fn get_impl(service_name: &str, account: &str) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>> {
    let s = Session::open()?;
    let result = (|| {
        let (mut unlocked, locked) = s.search(service_name, account)?;
        if unlocked.is_empty() && !locked.is_empty() {
            unlocked = s.unlock(&locked)?;
        }
        let Some(item) = unlocked.first() else {
            return Ok(None);
        };
        let secrets: HashMap<OwnedObjectPath, Secret> = service(&s.conn)?
            .call("GetSecrets", &(vec![item.clone()], s.path.clone()))
            .map_err(unavailable)?;
        Ok(secrets
            .into_values()
            .next()
            .map(|(_, _, value, _)| Zeroizing::new(value)))
    })();
    s.close();
    result
}

pub fn set_impl(service_name: &str, account: &str, secret: &[u8]) -> anyhow::Result<()> {
    let s = Session::open()?;
    let result = (|| {
        let collection: OwnedObjectPath = service(&s.conn)?
            .call("ReadAlias", &("default",))
            .map_err(unavailable)?;
        if is_none_path(&collection) {
            return Err(unavailable("there is no default collection"));
        }
        s.unlock(std::slice::from_ref(&collection))?;
        let mut props: HashMap<&str, Value<'_>> = HashMap::new();
        props.insert(
            "org.freedesktop.Secret.Item.Label",
            Value::from(service_name),
        );
        props.insert(
            "org.freedesktop.Secret.Item.Attributes",
            Value::from(attributes(service_name, account)),
        );
        let value = Zeroizing::new(secret.to_vec());
        let secret: (ObjectPath<'_>, &[u8], &[u8], &str) =
            (s.path.as_ref(), &[], &value[..], "application/octet-stream");
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = s
            .proxy(&collection, COLLECTION_IFACE)?
            .call("CreateItem", &(props, secret, true))
            .map_err(unavailable)?;
        if !is_none_path(&prompt) {
            s.dismiss(&prompt);
            return Err(unavailable("storing the key needs a prompt"));
        }
        Ok(())
    })();
    s.close();
    result
}

pub fn delete_impl(service_name: &str, account: &str) -> anyhow::Result<()> {
    let s = Session::open()?;
    let result = (|| {
        let (unlocked, locked) = s.search(service_name, account)?;
        for item in unlocked.iter().chain(locked.iter()) {
            let prompt: OwnedObjectPath = s
                .proxy(item, ITEM_IFACE)?
                .call("Delete", &())
                .map_err(unavailable)?;
            if !is_none_path(&prompt) {
                s.dismiss(&prompt);
                return Err(unavailable("deleting the key needs a prompt"));
            }
        }
        Ok(())
    })();
    s.close();
    result
}

/// The Secret Service answers (and has a default collection) within
/// [`PROBE_TIMEOUT`]. The probe runs on its own thread so a bus that never
/// answers can't hold the caller.
pub fn keychain_is_reliable_impl() -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("kage-secret-probe".into())
        .spawn(move || {
            let answered = (|| -> anyhow::Result<bool> {
                let conn = Connection::session().map_err(unavailable)?;
                let collection: OwnedObjectPath = service(&conn)?
                    .call("ReadAlias", &("default",))
                    .map_err(unavailable)?;
                Ok(!is_none_path(&collection))
            })();
            let _ = tx.send(answered);
        });
    if spawned.is_err() {
        return false;
    }
    match rx.recv_timeout(PROBE_TIMEOUT) {
        Ok(Ok(true)) => true,
        Ok(Ok(false)) => {
            log::info!("Sync key store: the Secret Service has no default collection");
            false
        }
        Ok(Err(e)) => {
            log::info!("Sync key store: no Secret Service ({})", e);
            false
        }
        Err(_) => {
            log::info!("Sync key store: the Secret Service didn't answer in time");
            false
        }
    }
}
