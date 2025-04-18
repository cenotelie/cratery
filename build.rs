/*******************************************************************************
 * Copyright (c) 2021 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

use std::process::Command;

fn main() {
    let db_url = "sqlite://src/empty.db";
    println!("cargo:rustc-env=DATABASE_URL={db_url}");
    if let Ok(output) = Command::new("git").args(["rev-parse", "HEAD"]).output() {
        let value = String::from_utf8(output.stdout).unwrap();
        println!("cargo:rustc-env=GIT_HASH={value}");
    }
    if let Ok(output) = Command::new("git").args(["tag", "-l", "--points-at", "HEAD"]).output() {
        let value = String::from_utf8(output.stdout).unwrap();
        println!("cargo:rustc-env=GIT_TAG={value}");
    }
}
