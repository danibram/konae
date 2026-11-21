use std::{
    env, fs,
    path::{Path, PathBuf},
};

const EXTENSIONS: [(&str, &str); 4] = [
    ("deno_webidl", "0.260.0"),
    ("deno_web", "0.291.0"),
    ("deno_net", "0.252.0"),
    ("deno_fetch", "0.284.0"),
];

fn main() {
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .expect("CARGO_HOME or HOME must point to the Cargo cache");
    let registry = cargo_home.join("registry/src");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let embedded = out.join("embedded");
    fs::create_dir_all(&embedded).expect("create embedded extension directory");

    let mut generated = String::from(
        "pub(super) fn source(extension: &str, file: &str) -> Option<&'static str> {\n match (extension, file) {\n",
    );
    for (crate_name, version) in EXTENSIONS {
        let source_root = find_crate(&registry, crate_name, version);
        println!("cargo:rerun-if-changed={}", source_root.display());
        let mut files: Vec<_> = fs::read_dir(&source_root)
            .expect("read extension sources")
            .map(|entry| entry.expect("read extension entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "js"))
            .collect();
        files.sort();
        for path in files {
            let name = path
                .file_name()
                .unwrap()
                .to_str()
                .expect("JS filename is UTF-8");
            let packaged = embedded.join(format!("{crate_name}-{name}"));
            fs::copy(&path, &packaged).expect("copy extension source to build output");
            let include_path = packaged.to_str().expect("OUT_DIR path is UTF-8");
            generated.push_str(&format!(
                "({crate_name:?}, {name:?}) => Some(include_str!({include_path:?})),\n"
            ));
        }
    }
    generated.push_str("_ => None,\n }\n}\n");
    fs::write(out.join("embedded_extensions.rs"), generated)
        .expect("write embedded extension lookup");
}

fn find_crate(registry: &Path, name: &str, version: &str) -> PathBuf {
    for index in fs::read_dir(registry).expect("read Cargo registry source directory") {
        let path = index
            .expect("read registry index")
            .path()
            .join(format!("{name}-{version}"));
        if path.is_dir() {
            return path;
        }
    }
    panic!("missing source for pinned extension {name}-{version}");
}
