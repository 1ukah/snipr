use std::{path::Path, process::Command};

fn main() {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    for p in ["src", "index.html", "package.json", "package-lock.json", "vite.config.ts", "tsconfig.json"] {
        println!("cargo:rerun-if-changed={}", ui.join(p).display());
    }
    println!("cargo:rerun-if-env-changed=SNIPR_SKIP_UI_BUILD");
    if std::env::var_os("SNIPR_SKIP_UI_BUILD").is_none() {
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        if !ui.join("node_modules").exists() {
            run(Command::new(npm).arg("ci").current_dir(&ui));
        }
        run(Command::new(npm).args(["run", "build"]).current_dir(&ui));
    }
    tauri_build::build()
}

fn run(cmd: &mut Command) {
    let status = cmd.status().expect("npm não encontrado: instale o Node.js 22 ou mais recente");
    assert!(status.success(), "falha ao compilar a interface (ui/)");
}
