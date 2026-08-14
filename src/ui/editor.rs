//! Permission editor (task 10). Stub until the TUI infra lands.

use crate::acl::facade::Model;
use std::process::ExitCode;

pub fn run(_m: Model) -> ExitCode {
    eprintln!("winfacl: editor not implemented yet");
    ExitCode::FAILURE
}
