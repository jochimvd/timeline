//! Real temporary git repositories, isolated from the developer's git
//! configuration: a fixed global config gives `me@home.example`, and
//! repositories under a `clients` directory `me@work.example` through a
//! conditional include, like a separate work identity.
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

/// Isolates git for this test process. Changing the environment races with
/// threads reading it, so a test binary calls this before `main` with
/// [`isolate_git!`](crate::isolate_git).
pub fn isolate() -> &'static Path {
    static BASE: OnceLock<PathBuf> = OnceLock::new();
    BASE.get_or_init(|| {
        let base = tempfile::tempdir().unwrap().keep();
        let work = base.join("work.gitconfig");
        std::fs::write(&work, "[user]\n\temail = me@work.example\n").unwrap();
        let global = base.join("global.gitconfig");
        std::fs::write(
            &global,
            format!(
                "[user]\n\tname = Me\n\temail = me@home.example\n[init]\n\tdefaultBranch = main\n\
                 [includeIf \"gitdir:**/clients/**\"]\n\tpath = {}\n",
                work.display()
            ),
        )
        .unwrap();
        // SAFETY: runs once, before main starts any other thread.
        unsafe {
            std::env::set_var("GIT_CONFIG_GLOBAL", &global);
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            std::env::set_var("HOME", &base);
            std::env::remove_var("GIT_AUTHOR_EMAIL");
            std::env::remove_var("GIT_COMMITTER_EMAIL");
            std::env::remove_var("EMAIL");
        }
        base
    })
}

/// Runs git in `directory` at `seconds`, which sets both the commit dates
/// and the reflog time, with optional extra environment.
pub fn run(directory: &Path, seconds: i64, arguments: &[&str], env: &[(&str, &str)]) -> String {
    let date = format!("@{seconds} +0000");
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub fn init(directory: &Path) {
    std::fs::create_dir_all(directory).unwrap();
    run(directory, 0, &["init", "-q"], &[]);
}

/// Commits a change to `file` with `subject` at `seconds`.
pub fn commit(directory: &Path, seconds: i64, subject: &str, env: &[(&str, &str)]) -> String {
    let file = directory.join("file.txt");
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    text.push_str(subject);
    text.push('\n');
    std::fs::write(&file, text).unwrap();
    run(directory, seconds, &["add", "file.txt"], &[]);
    run(directory, seconds, &["commit", "-q", "-m", subject], env);
    run(directory, seconds, &["rev-parse", "HEAD"], &[])
}
