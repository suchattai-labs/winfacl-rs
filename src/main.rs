mod acl;
mod tree;
mod ui;

use acl::facade::{LoadStatus, Model};
use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

/// An ncurses-style editor for POSIX.1e ACLs, laid out like the Windows
/// "Advanced Security Settings" dialog. A directory PATH (or no PATH at
/// all, which starts at /) opens the two-panel browser; a file PATH
/// opens the editor directly.
#[derive(Parser)]
#[command(name = "winfacl", version)]
struct Cli {
    /// report, but do not resolve, a symlink target
    #[arg(short = 'n', long = "no-follow")]
    no_follow: bool,

    /// print the ACL in getfacl(1) form and exit (no terminal required)
    #[arg(short = 'd', long = "dump")]
    dump: bool,

    path: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let follow = !cli.no_follow;

    let path = match cli.path {
        Some(p) => p,
        None => {
            if cli.dump {
                eprintln!("winfacl: --dump needs a path");
                return ExitCode::from(2);
            }
            PathBuf::from("/")
        }
    };

    if !cli.dump {
        let meta = if follow {
            std::fs::metadata(&path)
        } else {
            std::fs::symlink_metadata(&path)
        };
        if meta.map(|m| m.is_dir()).unwrap_or(false) {
            return ui::browser::run(&path, follow);
        }
    }

    let m = Model::load(&path, follow);
    match m.status {
        LoadStatus::Ok => {}
        LoadStatus::NoEnt => {
            eprintln!("winfacl: {}: no such file or directory", path.display());
            return ExitCode::FAILURE;
        }
        LoadStatus::Denied => {
            eprintln!("winfacl: {}: permission denied", path.display());
            return ExitCode::FAILURE;
        }
        LoadStatus::NotSup => {
            // Degraded but usable: the UI shows a read-only banner.
            eprintln!(
                "winfacl: {}: filesystem has no POSIX ACL support; \
                 showing mode bits read-only",
                path.display()
            );
        }
        LoadStatus::Error => {
            eprintln!(
                "winfacl: {}: {}",
                path.display(),
                m.load_errno.map_or_else(|| "I/O error".into(), |e| e.to_string())
            );
            return ExitCode::FAILURE;
        }
    }

    if cli.dump {
        print!("{}", m.format_getfacl());
        return ExitCode::SUCCESS;
    }

    ui::editor::run(m)
}
