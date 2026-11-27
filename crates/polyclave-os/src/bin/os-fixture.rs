use polyclave_os::{canonical_real_path, spawn_supervised, SpawnSpec};
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|a| polyclave_os::is_supervisor_argument(a))
    {
        std::process::exit(polyclave_os::supervisor_exec_entry(&args[1..]).unwrap());
    }
    let mode = args.first().and_then(|x| x.to_str()).unwrap_or("");
    match mode {
        "host" => {
            let root = canonical_real_path(&PathBuf::from(&args[1])).unwrap();
            let exe = std::env::current_exe().unwrap();
            let mut child = spawn_supervised(&SpawnSpec {
                supervisor: exe.clone(),
                worker: exe,
                args: vec!["tree".into()],
                cwd: root.clone(),
                env: std::env::vars_os()
                    .filter(|(key, _)| key.eq_ignore_ascii_case("SYSTEMROOT"))
                    .collect(),
                released: root.join("turn-tree.released"),
            })
            .unwrap();
            let mut output = child.take_stdout().unwrap();
            child.go(b"").unwrap();
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            print!("{line}");
            io::stdout().flush().unwrap();
            // Keep the descendant's liveness channel open, but do not own the only
            // observation channel: the test also waits on its stable process identity.
            io::copy(&mut output, &mut io::stdout()).unwrap();
        }
        "tree" => {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .arg("leaf")
                .spawn()
                .unwrap();
            let _ = child.wait();
        }
        "orphan" => {
            let mut leaf = Command::new(std::env::current_exe().unwrap())
                .arg("leaf")
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let mut line = String::new();
            io::BufReader::new(leaf.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            print!("{line}");
            io::stdout().flush().unwrap();
            // The fixture deliberately leaves a descendant for the supervisor.
            let _ = leaf.id();
            std::process::exit(0);
        }
        "leaf" => {
            #[cfg(unix)]
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            println!("leaf:{}", std::process::id());
            io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        "hold" => {
            println!("worker-ready");
            io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        _ => {
            let mut data = String::new();
            io::stdin().read_to_string(&mut data).unwrap();
            println!("worker:{data}");
        }
    }
}
