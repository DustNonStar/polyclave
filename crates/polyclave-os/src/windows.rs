use super::{wrapper_arguments, Pipes, ProcessIdentity, SpawnSpec};
use std::ffi::{c_void, OsStr};
use std::fs::File;
use std::io::{self, Write};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::*;

const FOREVER: u32 = u32::MAX;
const WAIT_SIGNALED: u32 = 0;

pub(crate) struct Process {
    child: OwnedHandle,
    job: Option<OwnedHandle>,
    pid: u32,
}

fn raw(h: &OwnedHandle) -> HANDLE {
    h.as_raw_handle()
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn ok(rc: i32) -> io::Result<()> {
    if rc == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Process {
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    pub(crate) fn cancel(&mut self) -> io::Result<()> {
        if let Some(job) = self.job.take() {
            // Drop still executes KILL_ON_JOB_CLOSE even if the explicit termination fails.
            ok(unsafe { TerminateJobObject(raw(&job), 125) })?;
        }
        Ok(())
    }

    pub(crate) fn wait(&mut self) -> io::Result<i32> {
        if unsafe { WaitForSingleObject(raw(&self.child), FOREVER) } != WAIT_SIGNALED {
            return Err(io::Error::last_os_error());
        }
        let mut code = 0;
        ok(unsafe { GetExitCodeProcess(raw(&self.child), &mut code) })?;
        // No background descendant should outlive the authorized turn.
        self.job.take();
        Ok(code as i32)
    }

    pub(crate) fn reap_on_drop(&mut self) {
        self.job.take();
    }
}

fn w(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}

fn quote(arg: &OsStr, out: &mut Vec<u16>) -> io::Result<()> {
    let mut slashes = 0;
    out.push(b'"' as u16);
    for c in arg.encode_wide() {
        if c == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "embedded NUL argument",
            ));
        }
        if c == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        out.extend(std::iter::repeat_n(
            b'\\' as u16,
            if c == b'"' as u16 {
                2 * slashes + 1
            } else {
                slashes
            },
        ));
        slashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    out.push(b'"' as u16);
    Ok(())
}

fn pipe(child_reads: bool) -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut sa: SECURITY_ATTRIBUTES = unsafe { zeroed() };
    sa.nLength = size_of::<SECURITY_ATTRIBUTES>() as u32;
    sa.bInheritHandle = 1;
    let (mut rd, mut wr) = (null_mut(), null_mut());
    ok(unsafe { CreatePipe(&mut rd, &mut wr, &sa, 0) })?;
    let (rd, wr) = (owned(rd)?, owned(wr)?);
    let parent = if child_reads { &wr } else { &rd };
    ok(unsafe { SetHandleInformation(raw(parent), HANDLE_FLAG_INHERIT, 0) })?;
    Ok((rd, wr))
}

struct Attributes {
    mem: Vec<usize>,
}
impl Attributes {
    fn new(handles: &[HANDLE]) -> io::Result<Self> {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut mem = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let list = mem.as_mut_ptr().cast();
        ok(unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) })?;
        let result = Self { mem };
        ok(unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            )
        })?;
        Ok(result)
    }
    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.mem.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.ptr());
        }
    }
}

pub(crate) fn spawn(spec: &SpawnSpec) -> io::Result<Pipes> {
    let (input_child, input_host) = pipe(true)?;
    let (output_host, output_child) = pipe(false)?;
    let (error_host, error_child) = pipe(false)?;
    let handles = [raw(&input_child), raw(&output_child), raw(&error_child)];
    let mut attrs = Attributes::new(&handles)?;
    let mut si: STARTUPINFOEXW = unsafe { zeroed() };
    si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    [
        si.StartupInfo.hStdInput,
        si.StartupInfo.hStdOutput,
        si.StartupInfo.hStdError,
    ] = handles;
    si.lpAttributeList = attrs.ptr();
    let mut line = Vec::new();
    quote(spec.supervisor.as_os_str(), &mut line)?;
    for arg in wrapper_arguments(spec) {
        line.push(b' ' as u16);
        quote(&arg, &mut line)?;
    }
    line.push(0);
    let mut pairs = spec.env.clone();
    pairs.sort_by_key(|(key, _)| key.to_string_lossy().to_uppercase());
    let mut env = Vec::<u16>::new();
    for (key, value) in &pairs {
        if key.is_empty()
            || key.encode_wide().any(|c| c == 0 || c == b'=' as u16)
            || value.encode_wide().any(|c| c == 0)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid environment entry",
            ));
        }
        env.extend(key.encode_wide());
        env.push(b'=' as u16);
        env.extend(value.encode_wide());
        env.push(0);
    }
    if env.is_empty() {
        env.push(0);
    }
    env.push(0);

    let job = owned(unsafe { CreateJobObjectW(null(), null()) })?;
    let mut options: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    // No BREAKAWAY_OK or SILENT_BREAKAWAY_OK.
    options.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    ok(unsafe {
        SetInformationJobObject(
            raw(&job),
            JobObjectExtendedLimitInformation,
            (&options as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    })?;
    let mut pi: PROCESS_INFORMATION = unsafe { zeroed() };
    ok(unsafe {
        CreateProcessW(
            w(spec.supervisor.as_os_str()).as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            env.as_ptr().cast(),
            w(spec.cwd.as_os_str()).as_ptr(),
            &si.StartupInfo,
            &mut pi,
        )
    })?;
    let child = owned(pi.hProcess)?;
    let thread = owned(pi.hThread)?;
    let launch = (|| {
        ok(unsafe { AssignProcessToJobObject(raw(&job), raw(&child)) })?;
        if unsafe { ResumeThread(raw(&thread)) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })();
    if let Err(error) = launch {
        unsafe {
            TerminateProcess(raw(&child), 125);
        }
        return Err(error);
    }
    Ok(Pipes {
        process: Process {
            child,
            job: Some(job),
            pid: pi.dwProcessId,
        },
        control: Box::new(File::from(input_host)),
        stdout: Box::new(File::from(output_host)),
        stderr: Box::new(File::from(error_host)),
    })
}

pub(crate) fn guard_parent(_pipe: impl io::Read + Send + 'static) -> io::Result<()> {
    // Parent-owned non-inherited Job handle is the writer exclusion guarantee.
    Ok(())
}
pub(crate) fn finish_parent_guard() -> io::Result<()> {
    Ok(())
}

pub(crate) fn sync_marker_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub fn process_identity(pid: u32) -> io::Result<Option<ProcessIdentity>> {
    let handle = match owned(unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid)
    }) {
        Ok(handle) => handle,
        Err(e) if e.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) => return Ok(None),
        Err(e) => return Err(e),
    };
    if unsafe { WaitForSingleObject(raw(&handle), 0) } == WAIT_SIGNALED {
        return Ok(None);
    }
    let (mut created, mut exited, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) =
        unsafe { zeroed() };
    ok(unsafe {
        GetProcessTimes(
            raw(&handle),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    })?;
    let time = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    Ok(Some(ProcessIdentity {
        pid,
        started_at: time.to_string(),
    }))
}

pub(crate) fn kill_process_tree(id: &ProcessIdentity) -> io::Result<()> {
    // The host owns the only Job handle: after it dies, the kernel terminates the job.
    // A still-matching process is not proof that its job has quiesced.
    if process_identity(id.pid)?.as_ref() == Some(id) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "old supervisor still running",
        ));
    }
    Ok(())
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, data: *mut c_void, len: u32, used: *mut u32) -> i32;
}

#[repr(C)]
struct BootEnvironment {
    guid: [u8; 16],
    firmware: u32,
    flags: u64,
}

#[repr(C)]
struct TimeOfDay {
    boot: i64,
    current: i64,
    zone_bias: i64,
    zone_id: u32,
    reserved: u32,
    boot_bias: u64,
    sleep_bias: u64,
}

pub fn boot_epoch() -> io::Result<String> {
    let mut data: BootEnvironment = unsafe { zeroed() };
    let code = unsafe {
        NtQuerySystemInformation(
            90,
            (&mut data as *mut BootEnvironment).cast(),
            size_of::<BootEnvironment>() as u32,
            null_mut(),
        )
    };
    if code >= 0 && data.guid != [0; 16] {
        let mut hex = String::with_capacity(32);
        use std::fmt::Write as _;
        for b in data.guid {
            let _ = write!(hex, "{b:02x}");
        }
        return Ok(hex);
    }
    let mut time: TimeOfDay = unsafe { zeroed() };
    let code = unsafe {
        NtQuerySystemInformation(
            3,
            (&mut time as *mut TimeOfDay).cast(),
            size_of::<TimeOfDay>() as u32,
            null_mut(),
        )
    };
    if code < 0 {
        return Err(io::Error::other("Windows boot identifier unavailable"));
    }
    let bias = i64::try_from(time.boot_bias).map_err(io::Error::other)?;
    time.boot
        .checked_sub(bias)
        .map(|v| format!("time:{v}"))
        .ok_or_else(|| io::Error::other("invalid Windows boot bias"))
}

struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn only_current_user() -> io::Result<Local> {
    let mut token = null_mut();
    ok(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
    let token = owned(token)?;
    let mut len = 0;
    unsafe {
        GetTokenInformation(raw(&token), TokenUser, null_mut(), 0, &mut len);
    }
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buf = vec![0usize; (len as usize).div_ceil(size_of::<usize>())];
    ok(unsafe {
        GetTokenInformation(
            raw(&token),
            TokenUser,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    })?;
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = null_mut();
    ok(unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) })?;
    let sid_mem = Local(sid.cast());
    let n = (0..).take_while(|&i| unsafe { *sid.add(i) != 0 }).count();
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid, n) })
        .map_err(io::Error::other)?;
    let mut sd = null_mut();
    ok(unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w(OsStr::new(&format!("D:P(A;;FA;;;{sid})"))).as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            null_mut(),
        )
    })?;
    drop(sid_mem);
    Ok(Local(sd))
}

fn security_attributes(sd: &Local) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    }
}

pub fn create_private_dir(path: &Path) -> io::Result<()> {
    let sd = only_current_user()?;
    ok(unsafe { CreateDirectoryW(w(path.as_os_str()).as_ptr(), &security_attributes(&sd)) })
}

pub fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let sd = only_current_user()?;
    let file = owned(unsafe {
        CreateFileW(
            w(path.as_os_str()).as_ptr(),
            GENERIC_WRITE,
            0,
            &security_attributes(&sd),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    })?;
    let mut file = File::from(file);
    file.write_all(contents)?;
    file.sync_all()
}

pub(crate) fn prepare_isolated_link(source: &Path, destination: &Path) -> io::Result<()> {
    if source.is_dir() {
        return junction::create(source, destination);
    }
    let mut f = File::open(source)?;
    let mut data = Vec::new();
    use std::io::Read as _;
    f.read_to_end(&mut data)?;
    write_private_file(destination, &data)
}
