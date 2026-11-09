//! The `doc.steering` namespace: the user steering doc at its default path
//! and the auto-steering doc (docs/design/SYNC.md §4.2).
//!
//! Records `user` and `auto`, one field `text`: the document's body (the
//! auto doc without its header comment, which every copy carries). A user
//! doc at a path the user chose stays on this computer: sync never writes
//! outside Kage's own folder, so that record reports [`super::unchanged`]
//! and isn't written. A missing file is an empty document.

use super::{
    is_unchanged, unchanged, LocalRecord, LocalState, LocalWrite, NsSpec, ProjectResult, Source,
};
use crate::sync::engine::ConfigPort;
use crate::sync::types::{Category, DefaultScope, Origin, Structure};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const NS: &str = "doc.steering";
pub const USER: &str = "user";
pub const AUTO: &str = "auto";
pub const FIELD: &str = "text";
const USER_FILE: &str = "user-steering.md";
const AUTO_FILE: &str = "auto-steering.md";
/// Largest document read; a bigger one isn't a steering doc Kage wrote.
const MAX_BYTES: u64 = 4 << 20;

/// The two steering documents in `<config>/kage`.
pub struct SteeringSource {
    port: Arc<dyn ConfigPort>,
    dir: PathBuf,
}

impl SteeringSource {
    /// `dir` is `<config>/kage`.
    pub fn new(port: Arc<dyn ConfigPort>, dir: PathBuf) -> Self {
        Self { port, dir }
    }

    fn user_path(&self) -> PathBuf {
        self.dir.join(USER_FILE)
    }

    fn auto_path(&self) -> PathBuf {
        self.dir.join(AUTO_FILE)
    }

    /// Whether the user doc is at the default path, so it syncs.
    fn user_doc_syncs(&self) -> bool {
        let (_, config) = self.port.snapshot();
        match config.acp.agent.user_steering_path.as_deref() {
            None => true,
            Some(p) if p.trim().is_empty() => true,
            Some(p) => same_file(Path::new(p), &self.user_path()),
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

fn read_text(path: &Path) -> anyhow::Result<String> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_BYTES => anyhow::bail!("a steering document is over its size limit"),
        Ok(_) => Ok(String::from_utf8_lossy(&std::fs::read(path)?).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

fn record(text: Value) -> LocalRecord {
    let mut fields = Map::new();
    fields.insert(FIELD.to_string(), text);
    LocalRecord {
        fields,
        avail: None,
    }
}

/// The auto doc's body: the file without its header comment.
fn auto_body(file: &str) -> String {
    crate::auto_steering::strip_header_comment(file)
}

impl Source for SteeringSource {
    fn id(&self) -> &'static str {
        "steering"
    }

    fn specs(&self) -> Vec<NsSpec> {
        vec![NsSpec {
            ns: NS.to_string(),
            category: Category::Settings,
            schema: 1,
            echo: true,
            structure: Structure::default(),
            default_scope: DefaultScope::Shared,
            large: true,
            label_field: None,
        }]
    }

    fn read_local(&mut self) -> anyhow::Result<LocalState> {
        let mut st = LocalState::default();
        let user = if self.user_doc_syncs() {
            Value::String(read_text(&self.user_path())?)
        } else {
            unchanged()
        };
        st.put(NS, USER, record(user));
        st.put(
            NS,
            AUTO,
            record(Value::String(auto_body(&read_text(&self.auto_path())?))),
        );
        Ok(st)
    }

    fn project(
        &mut self,
        writes: Vec<LocalWrite>,
        _origin: Origin,
    ) -> anyhow::Result<ProjectResult> {
        let user_syncs = self.user_doc_syncs();
        for w in writes.iter().filter(|w| w.ns == NS) {
            let text = match w.value.as_ref().and_then(|r| r.fields.get(FIELD)) {
                Some(Value::String(s)) => s.clone(),
                Some(v) if is_unchanged(v) => continue,
                // A deleted or empty document.
                _ => String::new(),
            };
            let (path, content, perms) = match w.id.as_str() {
                USER if user_syncs => (
                    self.user_path(),
                    text.clone(),
                    crate::atomic_write::Perms::Inherit,
                ),
                AUTO => (
                    self.auto_path(),
                    if text.is_empty() {
                        String::new()
                    } else {
                        format!("{}{}", crate::auto_steering::auto_steering_header(), text)
                    },
                    crate::atomic_write::Perms::Private,
                ),
                _ => continue,
            };
            if content.is_empty() && !path.exists() {
                continue;
            }
            std::fs::create_dir_all(&self.dir)?;
            crate::atomic_write::write(&path, content, perms)?;
        }
        Ok(ProjectResult::Applied {
            read_back: self.read_local()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::sync::engine::{CommitError, LoadReset};
    use std::sync::Mutex;

    struct Port(Mutex<Config>);

    impl ConfigPort for Port {
        fn snapshot(&self) -> (u64, Config) {
            (0, self.0.lock().unwrap().clone())
        }
        fn commit(&self, _: u64, next: Config, _: Origin) -> Result<u64, CommitError> {
            *self.0.lock().unwrap() = next;
            Ok(1)
        }
        fn load_was_reset(&self) -> LoadReset {
            LoadReset::None
        }
        fn saves_suspended(&self) -> bool {
            false
        }
        fn installed(&self) -> Vec<crate::extensions::InstalledItem> {
            Vec::new()
        }
    }

    fn write(id: &str, text: &str) -> LocalWrite {
        LocalWrite {
            ns: NS.into(),
            id: id.into(),
            value: Some(record(Value::String(text.into()))),
        }
    }

    #[test]
    fn documents_round_trip_and_a_custom_path_stays_here() {
        let dir = tempfile::tempdir().unwrap();
        let port = Arc::new(Port(Mutex::new(Config::default())));
        let mut src = SteeringSource::new(port.clone(), dir.path().to_path_buf());
        let st = src.read_local().unwrap();
        assert_eq!(
            st.get(NS, USER).unwrap().fields[FIELD],
            Value::String(String::new())
        );

        let ProjectResult::Applied { read_back } = src
            .project(
                vec![write(USER, "Be brief."), write(AUTO, "## Style\n- terse")],
                Origin::Sync,
            )
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(read_back, src.read_local().unwrap());
        let auto = std::fs::read_to_string(dir.path().join(AUTO_FILE)).unwrap();
        assert!(auto.starts_with("<!--"), "the header is kept");
        assert_eq!(
            read_back.get(NS, AUTO).unwrap().fields[FIELD],
            Value::String("## Style\n- terse".into())
        );

        port.0.lock().unwrap().acp.agent.user_steering_path = Some("/elsewhere/mine.md".into());
        assert!(is_unchanged(
            &src.read_local().unwrap().get(NS, USER).unwrap().fields[FIELD]
        ));
        src.project(vec![write(USER, "Not here")], Origin::Sync)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(USER_FILE)).unwrap(),
            "Be brief."
        );
    }
}
