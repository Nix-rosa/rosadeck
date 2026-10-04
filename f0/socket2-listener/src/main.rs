//! F0 prototype 4: read-only listener of .socket2.sock until Ctrl+C.
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::time::{SystemTime, UNIX_EPOCH};

fn stamp() -> String {
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let s = t.as_secs() % 86400;
    format!("{:02}:{:02}:{:02}.{:03}", s / 3600, s % 3600 / 60, s % 60, t.subsec_millis())
}

fn main() {
    let timeout = std::env::args()
        .skip_while(|a| a != "--timeout")
        .nth(1)
        .and_then(|v| v.parse::<u64>().ok());
    let runtime = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").expect("HIS");
    let path = format!("{runtime}/hypr/{sig}/.socket2.sock");
    let s = UnixStream::connect(&path).unwrap_or_else(|e| {
        eprintln!("ERROR connect {path}: {e}");
        std::process::exit(1);
    });
    if let Some(t) = timeout {
        s.set_read_timeout(Some(std::time::Duration::from_secs(t))).ok();
    }
    eprintln!("listening on {path} (Ctrl+C to stop)...");
    let mut r = BufReader::new(s);
    let mut line = String::new();
    loop {
        line.clear();
        match r.read_line(&mut line) {
            Ok(0) => {
                eprintln!("[{}] EOF (compositor closed)", stamp());
                break;
            }
            Ok(_) => {
                let l = line.trim();
                if l.is_empty() {
                    continue;
                }
                let mark = if l.starts_with("monitoradded") || l.starts_with("monitorremoved") || l == "configreloaded" || l.starts_with("configreloaded") {
                    " *"
                } else {
                    ""
                };
                println!("[{}] {l}{mark}", stamp());
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                eprintln!("[{}] timeout (--timeout reached), exiting cleanly", stamp());
                break;
            }
            Err(e) => {
                eprintln!("[{}] read error: {e}", stamp());
                break;
            }
        }
    }
}
