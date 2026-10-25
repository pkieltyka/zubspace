use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const LEVEL_ROOTS: &[&str] = &["assets/zones"];
const SETTINGS_EXTENSIONS: &[&str] = &["cfg", "conf", "ini", "set"];

fn main() {
    embed_build_commit();
    let mut levels = Vec::new();
    let mut lvz_archives = Vec::new();
    let mut settings = Vec::new();
    for root in LEVEL_ROOTS {
        println!("cargo:rerun-if-changed={root}");
        collect_files(Path::new(root), "lvl", &mut levels);
        collect_files(Path::new(root), "lvz", &mut lvz_archives);
        collect_files_with_extensions(Path::new(root), SETTINGS_EXTENSIONS, &mut settings);
    }

    sort_manifest_paths(&mut levels);
    sort_manifest_paths(&mut lvz_archives);
    sort_manifest_paths(&mut settings);

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR should be set by Cargo");
    for (name, contents) in [
        (
            "zone_assets.rs",
            generated_zone_groups(&levels, &lvz_archives, &settings),
        ),
        ("levels.rs", generated_slice(levels)),
        ("lvz_archives.rs", generated_slice(lvz_archives)),
        ("zone_settings.rs", generated_slice(settings)),
    ] {
        let path = Path::new(&out_dir).join(name);
        if fs::read(&path).ok().as_deref() != Some(contents.as_bytes()) {
            fs::write(path, contents).expect("generated zone manifest should be writable");
        }
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn embed_build_commit() {
    println!("cargo:rerun-if-env-changed=ZUBSPACE_BUILD_COMMIT");
    // Git metadata only: never hash/read the working tree or owner-only plans.
    for name in [
        Some("HEAD".to_owned()),
        git_output(&["symbolic-ref", "-q", "HEAD"]),
        Some("packed-refs".to_owned()),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = git_output(&["rev-parse", "--git-path", &name]) {
            if Path::new(&path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
    let commit = std::env::var("ZUBSPACE_BUILD_COMMIT")
        .ok()
        .or_else(|| git_output(&["rev-parse", "--verify", "HEAD"]))
        .expect("set ZUBSPACE_BUILD_COMMIT to the full source commit when building outside Git");
    let commit = commit.trim();
    assert!(
        matches!(commit.len(), 40 | 64) && commit.bytes().all(|c| c.is_ascii_hexdigit()),
        "ZUBSPACE_BUILD_COMMIT must be a full hexadecimal Git commit hash"
    );
    println!(
        "cargo:rustc-env=ZUBSPACE_SERVER_BUILD_COMMIT={}",
        commit.to_ascii_lowercase()
    );
}

fn collect_files(root: &Path, extension: &str, paths: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, extension, paths);
            continue;
        }

        if path.extension().and_then(|path_ext| path_ext.to_str()) != Some(extension) {
            continue;
        }

        let path = normalize_path(path);
        println!("cargo:rerun-if-changed={path}");
        paths.push(path);
    }
}

fn collect_files_with_extensions(root: &Path, extensions: &[&str], paths: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_with_extensions(&path, extensions, paths);
            continue;
        }

        let Some(path_ext) = path.extension().and_then(|path_ext| path_ext.to_str()) else {
            continue;
        };
        if !extensions
            .iter()
            .any(|extension| path_ext.eq_ignore_ascii_case(extension))
        {
            continue;
        }

        let path = normalize_path(path);
        println!("cargo:rerun-if-changed={path}");
        paths.push(path);
    }
}

fn generated_slice(paths: Vec<String>) -> String {
    let mut generated = String::from("&[\n");
    for path in paths {
        generated.push_str("    ");
        generated.push_str(&format!("{path:?}"));
        generated.push_str(",\n");
    }
    generated.push(']');
    generated
}

fn sort_manifest_paths(paths: &mut [String]) {
    paths.sort_by(|left, right| {
        manifest_path_priority(left)
            .cmp(&manifest_path_priority(right))
            .then_with(|| left.cmp(right))
    });
}

fn manifest_path_priority(path: &str) -> u8 {
    if path.starts_with("assets/zones/chaos-zone/") {
        0
    } else if path.starts_with("assets/zones/") {
        1
    } else {
        2
    }
}

fn normalize_path(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn generated_zone_groups(levels: &[String], lvz: &[String], settings: &[String]) -> String {
    let mut groups = BTreeMap::<&str, [Vec<String>; 3]>::new();
    for (kind, paths) in [levels, lvz, settings].into_iter().enumerate() {
        for path in paths {
            let Some((zone, _)) = path
                .strip_prefix("assets/zones/")
                .and_then(|p| p.split_once('/'))
            else {
                continue;
            };
            groups.entry(zone).or_default()[kind].push(path.clone());
        }
    }
    let mut result = String::from("&[\n");
    for (id, [levels, lvz, settings]) in groups {
        result.push_str(&format!(
            "ZoneAssetPaths {{ id: {id:?}, levels: {}, lvz: {}, settings: {} }},\n",
            generated_slice(levels),
            generated_slice(lvz),
            generated_slice(settings)
        ));
    }
    result.push(']');
    result
}
