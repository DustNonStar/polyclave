use super::{wrapper_arguments, Pipes, ProcessIdentity, SpawnSpec};
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};

pub(crate) struct Process {
    child: Option<Child>,
}

impl Process {
    pub(crate) fn pid(&self) -> u32 {
        self.child.as_ref().expect("running supervisor").id()
    }

    pub(crate) fn cancel(&mut self) -> io::Result<()> {
        // Closing the control pipe drives the wrapper's TERM/KILL path.
        Ok(())
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<i32>> {
        let status = self
            .child
            .as_mut()
            .ok_or_else(|| io::Error::other("missing supervisor"))?
            .try_wait()?
            .map(exit_code);
        if status.is_some() {
            self.child = None;
        }
        Ok(status)
    }

    pub(crate) fn wait(&mut self) -> io::Result<i32> {
        let status = self
            .child
            .as_mut()
            .ok_or_else(|| io::Error::other("missing supervisor"))?
            .wait()
            .map(exit_code)?;
        self.child = None;
        Ok(status)
    }

    pub(crate) fn reap_on_drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = std::thread::Builder::new()
                .name("supervisor-reaper".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(1))
}

pub(crate) fn spawn(spec: &SpawnSpec) -> io::Result<Pipes> {
    let mut child = Command::new(&spec.supervisor)
        .args(wrapper_arguments(spec))
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(spec.env.iter().cloned())
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let control = Box::new(child.stdin.take().expect("piped stdin"));
    let stdout = Box::new(child.stdout.take().expect("piped stdout"));
    let stderr = Box::new(child.stderr.take().expect("piped stderr"));
    Ok(Pipes {
        process: Process { child: Some(child) },
        control,
        stdout,
        stderr,
    })
}

pub fn create_private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(path)
}

pub fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

pub(crate) fn sync_marker_parent(path: &Path) -> io::Result<()> {
    File::open(
        path.parent()
            .ok_or_else(|| io::Error::other("missing marker directory"))?,
    )?
    .sync_all()
}

pub(crate) fn prepare_isolated_link(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

fn signal_group(pgid: i32, sig: i32) -> io::Result<()> {
    if unsafe { libc::kill(-pgid, sig) } == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(err)
    }
}

pub(crate) fn guard_parent(mut pipe: impl Read + Send + 'static) -> io::Result<()> {
    extern "C" fn swallow_term(_: i32) {}
    if unsafe {
        libc::signal(
            libc::SIGTERM,
            swallow_term as *const () as libc::sighandler_t,
        )
    } == libc::SIG_ERR
    {
        return Err(io::Error::last_os_error());
    }
    std::thread::Builder::new()
        .name("control-pipe-eof".into())
        .spawn(move || {
            // Only the host has the pipe's write end. EOF remains authoritative after go.
            let _ = io::copy(&mut pipe, &mut io::sink());
            let group = unsafe { libc::getpgrp() };
            let _ = signal_group(group, libc::SIGTERM);
            std::thread::park_timeout(std::time::Duration::from_secs(2));
            let _ = signal_group(group, libc::SIGKILL);
        })
        .map(|_| ())
}

pub(crate) fn finish_parent_guard() -> io::Result<()> {
    let mut fd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLHUP,
        revents: 0,
    };
    // If the root Worker exits first on TERM, the wrapper must remain alive to
    // enforce the guard's KILL deadline for any remaining descendants.
    let rc = unsafe { libc::poll(&mut fd, 1, 0) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    if fd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
        loop {
            std::thread::park();
        }
    }
    // Normal Worker exit does not grant descendants permission to keep
    // writing. Wait on actual group membership, not a fixed delay in tests.
    let group = unsafe { libc::getpgrp() };
    if group_members(group).is_ok_and(|p| p.is_empty()) {
        return Ok(());
    }
    signal_group(group, libc::SIGTERM)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Ok(members) = group_members(group) {
            if members.is_empty() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                for pid in members {
                    // Recheck group membership before acting on each PID.
                    if unsafe { libc::getpgid(pid) } == group {
                        let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
                    }
                }
                for _ in 0..10 {
                    if group_members(group).is_ok_and(|p| p.is_empty()) {
                        return Ok(());
                    }
                    std::thread::park_timeout(std::time::Duration::from_millis(25));
                }
                signal_group(group, libc::SIGKILL)?;
                std::thread::park();
            }
        } else if std::time::Instant::now() >= deadline {
            signal_group(group, libc::SIGKILL)?;
            std::thread::park();
        }
        std::thread::park_timeout(std::time::Duration::from_millis(20));
    }
}

fn group_members(group: i32) -> io::Result<Vec<i32>> {
    let command = if Path::new("/bin/ps").is_file() {
        "/bin/ps"
    } else if Path::new("/usr/bin/ps").is_file() {
        "/usr/bin/ps"
    } else {
        "ps"
    };
    let output = Command::new(command)
        .env("LC_ALL", "C")
        .process_group(0)
        .args(["-ax", "-o", "pid=", "-o", "pgid=", "-o", "stat="])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("process group query failed"));
    }
    let ours = std::process::id() as i32;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut values = line.split_whitespace();
            let pid = values.next()?.parse().ok()?;
            let pgid = values.next()?.parse::<i32>().ok()?;
            let status = values.next()?;
            (pid != ours && pgid == group && !status.starts_with('Z')).then_some(pid)
        })
        .collect())
}

pub fn boot_epoch() -> io::Result<String> {
    #[cfg(target_os = "macos")]
    {
        let mut tv = libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        };
        let mut len = std::mem::size_of_val(&tv);
        let rc = unsafe {
            libc::sysctlbyname(
                c"kern.boottime".as_ptr(),
                (&mut tv as *mut libc::timeval).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(format!("{}.{:06}", tv.tv_sec, tv.tv_usec))
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map(|s| s.trim().to_owned())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unknown Unix boot identity",
    ))
}

pub fn process_identity(pid: u32) -> io::Result<Option<ProcessIdentity>> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    let started_at = {
        let mut v = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let len = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let read = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                v.as_mut_ptr().cast(),
                len,
            )
        };
        if read <= 0 {
            return match io::Error::last_os_error().raw_os_error() {
                Some(0 | libc::ESRCH | libc::ENOENT | libc::EPERM) => Ok(None),
                _ => Err(io::Error::last_os_error()),
            };
        }
        if read != len {
            return Err(io::Error::other("short proc_pidinfo"));
        }
        let v = unsafe { v.assume_init() };
        if v.pbi_status == 5 {
            return Ok(None);
        }
        format!("{}.{:06}", v.pbi_start_tvsec, v.pbi_start_tvusec)
    };
    #[cfg(target_os = "linux")]
    let started_at = {
        let data = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(data) => data,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let (_, tail) = data
            .rsplit_once(')')
            .ok_or_else(|| io::Error::other("invalid process stat"))?;
        let mut parts = tail.split_whitespace();
        if parts.next() == Some("Z") {
            return Ok(None);
        }
        parts
            .nth(18)
            .ok_or_else(|| io::Error::other("missing process start"))?
            .to_owned()
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    return Ok(Some(ProcessIdentity { pid, started_at }));
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unknown Unix process identity",
    ))
}

pub(crate) fn kill_process_tree(id: &ProcessIdentity) -> io::Result<()> {
    if process_identity(id.pid)?.as_ref() == Some(id) {
        signal_group(id.pid as i32, libc::SIGKILL)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn parse_ioreg_hid_idle_secs(stdout: &str) -> Option<u32> {
    for line in stdout.lines() {
        if let Some((_, rhs)) = line.split_once("\"HIDIdleTime\" =") {
            let ns: u64 = rhs.trim().parse().ok()?;
            return Some((ns / 1_000_000_000).min(u64::from(u32::MAX)) as u32);
        }
    }
    None
}

pub fn desktop_idle_secs() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("/usr/sbin/ioreg")
            .args(["-c", "IOHIDSystem", "-d", "4"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8(out.stdout).ok()?;
        parse_ioreg_hid_idle_secs(&text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(all(test, target_os = "macos"))]
mod idle_tests {
    use super::*;

    #[test]
    fn parses_ioreg_hid_idle_time_and_reads_host_idle() {
        let sample = "  | |   \"HIDIdleTime\" = 45000000000\n";
        assert_eq!(parse_ioreg_hid_idle_secs(sample), Some(45));
        assert!(desktop_idle_secs().is_some());
    }
}
