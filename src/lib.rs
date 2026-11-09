use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::Sender,
    thread,
};

const REPOSITORY: &str = "https://github.com/bedrock-crustaceans/chorus.git";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Registry {
    servers: Vec<Server>,
}

pub struct Store {
    root: PathBuf,
    servers: Vec<Server>,
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("servers")).context("failed to create data directory")?;
        let registry = root.join("registry.json");
        let servers = if registry.exists() {
            serde_json::from_slice::<Registry>(
                &fs::read(&registry).context("failed to read registry")?,
            )
            .context("invalid registry.json")?
            .servers
        } else {
            Vec::new()
        };
        Ok(Self { root, servers })
    }

    pub fn servers(&self) -> &[Server] {
        &self.servers
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn directory(&self, name: &str) -> PathBuf {
        self.root.join("servers").join(name)
    }

    pub fn add(&mut self, name: &str) -> Result<()> {
        validate_name(name)?;
        if self.servers.iter().any(|server| server.name == name) || self.directory(name).exists() {
            bail!("server name already exists: {name}");
        }
        fs::create_dir(self.directory(name))
            .with_context(|| format!("failed to create server {name}"))?;
        self.servers.push(Server {
            name: name.to_owned(),
        });
        if let Err(error) = self.save() {
            self.servers.pop();
            return Err(error);
        }
        Ok(())
    }

    fn save(&self) -> Result<()> {
        let temp = self.root.join("registry.json.tmp");
        let bytes = serde_json::to_vec_pretty(&Registry {
            servers: self.servers.clone(),
        })?;
        fs::write(&temp, bytes).context("failed to write registry")?;
        fs::rename(&temp, self.root.join("registry.json")).context("failed to save registry")?;
        Ok(())
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 48
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        bail!("name must be 1-48 ASCII letters, numbers, '-' or '_'");
    }
    Ok(())
}

pub fn default_data_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        return env::var_os("APPDATA")
            .map(|dir| PathBuf::from(dir).join("ezymcbesrv"))
            .context("APPDATA is not set; use --data-dir");
    }
    #[cfg(not(windows))]
    {
        if let Some(dir) = env::var_os("XDG_DATA_HOME").filter(|dir| !dir.is_empty()) {
            return Ok(PathBuf::from(dir).join("ezymcbesrv"));
        }
        env::var_os("HOME")
            .map(|dir| PathBuf::from(dir).join(".local/share/ezymcbesrv"))
            .context("HOME is not set; use --data-dir")
    }
}

pub fn binary_path(dir: &Path) -> PathBuf {
    dir.join("source")
        .join("target")
        .join("release")
        .join(format!("chorus{}", env::consts::EXE_SUFFIX))
}

pub fn install_or_build(dir: PathBuf, tx: Sender<(String, Result<()>)>, name: String) {
    thread::spawn(move || {
        let result = build(&dir);
        let _ = tx.send((name, result));
    });
}

fn build(dir: &Path) -> Result<()> {
    let log_path = dir.join("build.log");
    let source = dir.join("source");
    if !source.exists() {
        let log = fs::File::create(&log_path).context("failed to open build log")?;
        let status = Command::new("git")
            .args(["clone", REPOSITORY])
            .arg(&source)
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .status()
            .context("failed to start git (is Git installed?)")?;
        if !status.success() {
            bail!("git clone failed ({status}); see build.log");
        }
    }
    if !source.join("Cargo.toml").is_file() {
        bail!("source is incomplete; inspect the source directory and build.log");
    }
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .context("failed to open build log")?;
    let status = Command::new("cargo")
        .args(["build", "--release", "--bin", "chorus"])
        .current_dir(&source)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .status()
        .context("failed to start cargo (is Rust installed?)")?;
    if !status.success() {
        bail!("Chorus build failed ({status}); see build.log");
    }
    if !binary_path(dir).is_file() {
        bail!("build finished but Chorus binary was not found");
    }
    Ok(())
}

pub fn start(dir: &Path) -> Result<Child> {
    let binary = binary_path(dir);
    if !binary.is_file() {
        bail!("Chorus is not built; press b to build");
    }
    let data = dir.join("data");
    fs::create_dir_all(&data).context("failed to create server data directory")?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("server.log"))?;
    Command::new(&binary)
        .current_dir(data)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()
        .context("failed to start Chorus")
}

pub fn send_command(child: &mut Child, command: &str) -> Result<()> {
    use std::io::Write;
    if command.contains(['\r', '\n']) {
        bail!("command cannot contain a newline");
    }
    let stdin = child
        .stdin
        .as_mut()
        .context("Chorus stdin is unavailable")?;
    writeln!(stdin, "{command}").context("failed to send command to Chorus")?;
    stdin.flush().context("failed to flush Chorus stdin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_escape_server_directory() {
        for name in ["", "../world", "a/b", "a\\b", ".", "a b", "a\nstop"] {
            assert!(validate_name(name).is_err(), "{name:?}");
        }
        for name in ["test", "Bedrock_1", "a-b"] {
            assert!(validate_name(name).is_ok());
        }
    }

    #[test]
    fn registry_survives_reopening() {
        let root = std::env::temp_dir().join(format!(
            "ezymcbesrv-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let mut store = Store::open(root.clone()).expect("open");
        store.add("first").expect("add");
        assert!(store.add("first").is_err());
        drop(store);
        assert_eq!(
            Store::open(root.clone()).expect("reopen").servers()[0].name,
            "first"
        );
        fs::remove_dir_all(root).expect("remove test directory");
    }
}
