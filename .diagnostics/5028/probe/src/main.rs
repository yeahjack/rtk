#[path = "../../../../src/core/windows_path.rs"]
mod windows_path;
use std::{path::Path, process::Command};
fn main() {
    let dir = std::env::args().nth(1).expect("directory");
    let candidate = Path::new(&dir).join("git.exe");
    println!("candidate metadata: {:?}", std::fs::symlink_metadata(&candidate).map(|m| (m.is_file(),m.is_symlink())));
    println!("candidate open: {:?}", std::fs::File::open(&candidate).map(|_| ()));
    let resolved = which::which("git");
    println!("which: {:?}", resolved);
    let resolved = windows_path::resolve("git");
    println!("fixed resolver: {:?}", resolved);
    if let Ok(path) = resolved {
        println!("resolved spawn: {:?}", Command::new(path).arg("--version").output().map(|o| (o.status.code(),String::from_utf8_lossy(&o.stdout).into_owned(),String::from_utf8_lossy(&o.stderr).into_owned())));
    }
    println!("native spawn: {:?}", Command::new("git").arg("--version").output().map(|o| (o.status.code(),String::from_utf8_lossy(&o.stdout).into_owned(),String::from_utf8_lossy(&o.stderr).into_owned())));
}
