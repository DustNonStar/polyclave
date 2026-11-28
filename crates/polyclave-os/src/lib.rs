//! Process and file primitives; stage code remains independent of platform APIs.
use std::ffi::{OsStr, OsString};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod path;
#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

pub use path::{
    augmented_path_env, canonical_real_path, prepare_isolated_link, resolve_cli_binary,
};
pub use platform::{
    boot_epoch, create_private_dir, desktop_idle_secs, process_identity, write_private_file,
};

/// Runs a trusted command in a managed process tree. Files in the caller's
/// private scratch directory avoid waiting on pipes held open by descendants.
pub fn cancellable_output(
    mut command: Command,
    scratch: &Path,
    check: impl Fn() -> io::Result<()>,
) -> io::Result<std::process::Output> {
    use std::{fs::File, time::Duration};

    check()?;
    let stdout = scratch.join("command.stdout");
    let stderr = scratch.join("command.stderr");
    let mut child =
        platform::CommandTree::spawn(&mut command, File::create(&stdout)?, File::create(&stderr)?)?;
    let status = loop {
        check()?;
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::park_timeout(Duration::from_millis(10));
    };
    Ok(std::process::Output {
        status,
        stdout: std::fs::read(stdout)?,
        stderr: std::fs::read(stderr)?,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started_at: String,
}

pub struct SpawnSpec {
    pub supervisor: PathBuf,
    pub worker: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    pub released: PathBuf,
}

pub struct Supervised {
    process: platform::Process,
    control: Option<Box<dyn Write + Send>>,
    output: Option<BufReader<Box<dyn Read + Send>>>,
    stderr: Option<Box<dyn Read + Send>>,
    identity: ProcessIdentity,
    released: bool,
}

impl Supervised {
    pub fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }

    pub fn go(&mut self, prompt: &[u8]) -> io::Result<()> {
        if self.released || prompt.len() > MAX_PROMPT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid release",
            ));
        }
        let control = self
            .control
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "control closed"))?;
        self.released = true;
        write!(control, "go\n{}\n", prompt.len())?;
        control.write_all(prompt)?;
        control.flush()
    }

    pub fn take_stdout(&mut self) -> Option<BufReader<Box<dyn Read + Send>>> {
        self.output.take()
    }

    pub fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stderr.take()
    }

    pub fn cancel(&mut self) -> io::Result<()> {
        self.control.take();
        self.process.cancel()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
        self.process.try_wait()
    }

    pub fn wait(&mut self) -> io::Result<i32> {
        self.process.wait()
    }
}

impl Drop for Supervised {
    fn drop(&mut self) {
        self.control.take();
        self.process.reap_on_drop();
    }
}

pub fn spawn_supervised(spec: &SpawnSpec) -> io::Result<Supervised> {
    let handle = platform::spawn(spec)?;
    let mut process = handle.process;
    let mut control = Some(handle.control);
    let result = (|| {
        let mut output = BufReader::new(handle.stdout);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("supervisor-ready".into())
            .spawn(move || {
                let mut ready = String::new();
                let res = (&mut output).take(2048).read_line(&mut ready);
                let _ = tx.send((output, ready, res));
            })?;
        let (output, ready, read_res) = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "supervisor ready timed out"))?;
        read_res?;
        let ready = ready.trim_end_matches(['\r', '\n']);
        let ready = ready.strip_prefix("ready\t").ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid supervisor ready")
        })?;
        let identity = process_identity(process.pid())?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "supervisor exited before ready")
        })?;
        if identity.started_at != ready {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "identity changed",
            ));
        }
        Ok((output, identity))
    })();
    let (output, identity) = match result {
        Ok(v) => v,
        Err(error) => {
            control.take();
            let _ = process.cancel();
            process.reap_on_drop();
            return Err(error);
        }
    };
    Ok(Supervised {
        process,
        control,
        output: Some(output),
        stderr: Some(handle.stderr),
        identity,
        released: false,
    })
}

pub fn kill_process_tree(identity: &ProcessIdentity) -> io::Result<()> {
    platform::kill_process_tree(identity)
}

pub(crate) struct Pipes {
    process: platform::Process,
    control: Box<dyn Write + Send>,
    stdout: Box<dyn Read + Send>,
    stderr: Box<dyn Read + Send>,
}

const MAX_PROMPT: usize = 16 * 1024 * 1024;

pub(crate) fn wrapper_arguments(spec: &SpawnSpec) -> Vec<OsString> {
    let mut args = vec![
        "--supervisor-exec".into(),
        spec.released.as_os_str().to_owned(),
        spec.worker.as_os_str().to_owned(),
    ];
    args.extend(spec.args.iter().cloned());
    args
}

/// Runs synchronously, before initialization of the host runtime.
pub fn supervisor_exec_entry(args: &[OsString]) -> io::Result<i32> {
    let [marker, worker, worker_args @ ..] = args else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid supervisor invocation",
        ));
    };
    let identity = process_identity(std::process::id())?
        .ok_or_else(|| io::Error::other("supervisor identity unavailable"))?;
    println!("ready\t{}", identity.started_at);
    io::stdout().flush()?;
    let mut control = BufReader::new(io::stdin());
    let mut go = [0; 3];
    if control.read_exact(&mut go).is_err() || &go != b"go\n" {
        return Ok(125);
    }
    let mut len = String::new();
    (&mut control).take(32).read_line(&mut len)?;
    let len = len
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|&n| n <= MAX_PROMPT)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid input length"))?;
    let mut prompt = vec![0; len];
    control.read_exact(&mut prompt)?;
    write_private_file(Path::new(marker), b"released\n")?;
    platform::sync_marker_parent(Path::new(marker))?;
    platform::guard_parent(control)?;
    let mut child = Command::new(worker)
        .args(worker_args)
        .stdin(Stdio::piped())
        .spawn()?;
    let input = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("no worker stdin"))?;
    let pump = std::thread::spawn(move || {
        let mut input = input;
        let _ = input.write_all(&prompt);
    });
    let status = child.wait()?;
    platform::finish_parent_guard()?;
    let _ = pump.join();
    Ok(status.code().unwrap_or(1))
}

pub fn is_supervisor_argument(argument: &OsStr) -> bool {
    argument == "--supervisor-exec"
}

/// Unix process groups may not contain descendants which started a new session.
pub fn hard_stop_excludes_writers() -> bool {
    cfg!(windows)
}

pub fn exclude_old_supervisor(identity: &ProcessIdentity) -> io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match kill_process_tree(identity) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        if process_identity(identity.pid)?.as_ref() != Some(identity) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "old supervisor did not exit",
            ));
        }
        std::thread::park_timeout(std::time::Duration::from_millis(20));
    }
}

/// Reads only an explicitly bound native credential. Never falls back to a mock store.
pub fn read_provider_credential(service: &str, account: &str) -> io::Result<String> {
    #[cfg(any(target_os = "macos", windows))]
    {
        keyring::Entry::new(service, account)
            .and_then(|entry| entry.get_password())
            .map_err(|_| io::Error::other("native credential unavailable"))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (service, account);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native credential store is not configured",
        ))
    }
}

#[cfg(any(target_os = "macos", windows))]
pub fn write_provider_credential(service: &str, account: &str, value: &str) -> io::Result<()> {
    keyring::Entry::new(service, account)
        .and_then(|entry| entry.set_password(value))
        .map_err(|_| io::Error::other("native credential could not be saved"))
}
