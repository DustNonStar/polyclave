use super::{wrapper_arguments, Pipes, ProcessIdentity, SpawnSpec};
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
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

    pub(crate) fn wait(&mut self) -> io::Result<i32> {
        self.child
            .as_mut()
            .ok_or_else(|| io::Error::other("missing supervisor"))?
            .wait()
            .map(|s| s.code().unwrap_or(1))
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

pub(crate) fn spawn(spec: &SpawnSpec) -> io::Result<Pipes> {
    let mut child = Command::new(&spec.supervisor)
        .args(wrapper_arguments(spec))
        .current_dir(&spec.cwd)
        .env_clear()
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
    Ok(())
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
                Some(0 | libc::ESRCH | libc::ENOENT) => Ok(None),
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
