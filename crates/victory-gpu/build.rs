//! The branch and commit this was built from, for the window caption
//! (`VICTORY_BUILD`): several builds run side by side on one machine, and a
//! window has to say which one it is.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (out.status.success() && !text.is_empty()).then(|| text.to_owned())
}

fn main() {
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]);
    let commit = git(&["rev-parse", "--short", "HEAD"]);
    let build = match (branch, commit) {
        (Some(branch), Some(commit)) if branch != "HEAD" => format!("{branch} {commit}"),
        (_, Some(commit)) => commit,
        _ => "unknown build".to_owned(),
    };
    println!("cargo:rustc-env=VICTORY_BUILD={build}");
    // Every commit and every checkout appends to the log of HEAD.
    for path in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", path]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
