/*******************************************************************************
 * Copyright (c) 2021 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

use std::process::Command;

fn main() {
    let db_url = "sqlite://src/empty.db";
    println!("cargo:rustc-env=DATABASE_URL={db_url}");

    extract_and_set_env("GIT_HASH", "git", &["rev-parse", "HEAD"]);
    extract_and_set_env("GIT_TAG", "git", &["tag", "-l", "--points-at", "HEAD"]);
}

fn extract_and_set_env(env_var: &str, cmd: &str, args: &[&str]) {
    if let Some(value) = extract_value_from_cmd(cmd, args) {
        println!("cargo:rustc-env={env_var}={value}");
    }
}

/// Run a command with args and extract output in a String
fn extract_value_from_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok().map(|out| out.trim().to_string()))
}
