use std::{env, fs::File, io::Read, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let version_path = manifest_dir.join("release-version");
    let mut version = String::new();
    File::open(&version_path)
        .and_then(|mut file| file.read_to_string(&mut version))
        .expect("read rust/release-version");
    let version = version.trim();
    assert!(
        version.split('.').count() == 3
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            && version.split('.').nth(2).unwrap_or("").len() == 4,
        "release-version must be semver-style with a four-digit patch: {version}"
    );
    println!("cargo:rustc-env=APPSDK_VERSION={version}");
    println!("cargo:rerun-if-changed=release-version");
}
