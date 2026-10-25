use std::collections::BTreeSet;
#[cfg(test)]
use std::fs;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use sha2::{Digest, Sha256};

pub(super) const BUNDLE_MANIFEST_FILENAME: &str = "zone.bundle";
const MAX_BUNDLES: usize = crate::zone_wire::MAX_ZONE_COUNT;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_MAP_BYTES: u64 = crate::zone_wire::MAX_MAP_SOURCE_BYTES as u64;
const MAX_SETTINGS_BYTES: u64 = 256 * 1024;
const MAX_NEWS_BYTES: u64 = 256 * 1024;
const MAX_LVZ_BYTES: u64 = crate::zone_wire::MAX_TRANSFER_FILE_BYTES as u64;
const MAX_LVZ_FILES: usize = crate::zone_wire::MAX_ZONE_FILES - 1;
const MAX_BUNDLE_BYTES: u64 = (crate::zone_wire::MAX_ZONE_TRANSFER_BYTES + 512 * 1024) as u64;
// A catalog root may contain one directory for every supported zone plus a
// bounded number of ignored regular metadata files (for example an operator's
// README or deployment marker). Count every entry type at readdir time so a
// directory full of irrelevant files or symlinks cannot grow the scan.
const MAX_CATALOG_ROOT_EXTRA_ENTRIES: usize = 64;
const MAX_CATALOG_ROOT_ENTRIES: usize = MAX_BUNDLES + MAX_CATALOG_ROOT_EXTRA_ENTRIES;
// A maximally populated bundle has a manifest, map, settings, news, and 64
// LVZs. Sixteen extra regular deployment files are tolerated, but also consume
// this cap; nested directories, symlinks, and special nodes are counted before
// the stricter bundle type policy rejects them.
const MAX_BUNDLE_DIRECTORY_EXTRA_ENTRIES: usize = 16;
const MAX_BUNDLE_DIRECTORY_ENTRIES: usize =
    1 + 1 + 1 + 1 + MAX_LVZ_FILES + MAX_BUNDLE_DIRECTORY_EXTRA_ENTRIES;

pub(super) struct DiskZoneDefinition {
    pub(super) id: String,
    pub(super) display_name: String,
    pub(super) override_id: Option<String>,
    pub(super) map_filename: String,
    pub(super) map_bytes: Vec<u8>,
    pub(super) settings_bytes: Vec<u8>,
    pub(super) news_filename: String,
    pub(super) news_bytes: Vec<u8>,
    pub(super) lvz_files: Vec<(String, Vec<u8>)>,
}

#[derive(Default)]
struct ParsedManifest {
    version: Option<String>,
    id: Option<String>,
    display_name: Option<String>,
    override_id: Option<String>,
    map: Option<String>,
    map_sha256: Option<String>,
    settings: Option<String>,
    settings_sha256: Option<String>,
    news: Option<String>,
    news_sha256: Option<String>,
    lvz: Vec<(String, String)>,
}

#[cfg(test)]
pub(super) fn load_disk_zone_definitions(root: &Path) -> Result<Vec<DiskZoneDefinition>, String> {
    load_disk_zone_definitions_with_source_budget(
        root,
        crate::zone_wire::MAX_ZONE_CATALOG_SOURCE_BYTES as u64,
    )
}

pub(super) fn load_disk_zone_definitions_with_source_budget(
    root: &Path,
    source_budget: u64,
) -> Result<Vec<DiskZoneDefinition>, String> {
    let root_directory = descriptor::Directory::open_root(root)?;
    let entries = root_directory.entries(MAX_CATALOG_ROOT_ENTRIES)?;
    load_disk_zone_definitions_from_open_root(root, root_directory, entries, source_budget)
}

fn load_disk_zone_definitions_from_open_root(
    root: &Path,
    root_directory: descriptor::Directory,
    entries: Vec<descriptor::Entry>,
    source_budget: u64,
) -> Result<Vec<DiskZoneDefinition>, String> {
    let mut directories = Vec::new();
    for entry in entries {
        if entry.kind == descriptor::EntryKind::Symlink {
            return Err(format!(
                "symlink entries are forbidden in zone bundle root: {}/{:?}",
                root.display(),
                entry.name
            ));
        }
        if entry.kind == descriptor::EntryKind::Directory {
            directories.push(entry.name);
        } else if entry.kind != descriptor::EntryKind::Regular {
            return Err(format!(
                "special entries are forbidden in zone bundle root: {}/{:?}",
                root.display(),
                entry.name
            ));
        }
    }
    directories.sort();
    if directories.len() > MAX_BUNDLES {
        return Err(format!(
            "zone bundle root contains {} directories; maximum is {MAX_BUNDLES}",
            directories.len()
        ));
    }

    let mut definitions = Vec::with_capacity(directories.len());
    let mut ids = BTreeSet::new();
    let mut aliases = BTreeSet::new();
    let mut catalog_budget = SourceReadBudget::new(source_budget);
    for directory_name in directories {
        let definition = load_bundle(&root_directory, &directory_name, &mut catalog_budget)?;
        if !ids.insert(definition.id.clone()) {
            return Err(format!("duplicate disk zone id {:?}", definition.id));
        }
        let normalized_aliases = [&definition.id, &definition.display_name]
            .into_iter()
            .map(|alias| super::normalized_zone_lookup_name(alias))
            .collect::<BTreeSet<_>>();
        for normalized in normalized_aliases {
            if normalized.is_empty() {
                return Err(format!(
                    "disk zone {:?} has an empty normalized identity",
                    definition.id
                ));
            }
            if !aliases.insert(normalized) {
                return Err(format!(
                    "disk zone identity conflicts after normalization: {:?}",
                    definition.id
                ));
            }
        }
        definitions.push(definition);
    }
    Ok(definitions)
}

#[cfg(all(
    test,
    any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    )
))]
fn load_disk_zone_definitions_with_forced_enumeration_error(
    root: &Path,
    accepted_entries: usize,
) -> Result<Vec<DiskZoneDefinition>, String> {
    let root_directory = descriptor::Directory::open_root(root)?;
    let entries = root_directory
        .entries_with_forced_error_after(MAX_CATALOG_ROOT_ENTRIES, accepted_entries)?;
    load_disk_zone_definitions_from_open_root(
        root,
        root_directory,
        entries,
        crate::zone_wire::MAX_ZONE_CATALOG_SOURCE_BYTES as u64,
    )
}

fn load_bundle(
    root: &descriptor::Directory,
    directory_name: &str,
    catalog_budget: &mut SourceReadBudget,
) -> Result<DiskZoneDefinition, String> {
    let directory = root.open_directory(directory_name)?;
    for entry in directory.entries(MAX_BUNDLE_DIRECTORY_ENTRIES)? {
        if entry.kind != descriptor::EntryKind::Regular {
            return Err(format!(
                "zone bundle entries must be real regular files, not symlinks or nested directories: {}/{:?}",
                directory.display().display(),
                entry.name
            ));
        }
    }
    let manifest_path = directory.display_path(BUNDLE_MANIFEST_FILENAME);
    // Charge even transient manifest reads so peak loader input stays inside
    // the catalog source envelope.
    let manifest_bytes =
        catalog_budget.read(&directory, BUNDLE_MANIFEST_FILENAME, MAX_MANIFEST_BYTES)?;
    let manifest_text = std::str::from_utf8(&manifest_bytes).map_err(|err| {
        format!(
            "zone bundle manifest {} is not UTF-8: {err}",
            manifest_path.display()
        )
    })?;
    let manifest = parse_manifest(manifest_text, &manifest_path)?;

    let version = required(&manifest.version, "version", &manifest_path)?;
    if version != "1" {
        return Err(format!(
            "zone bundle manifest {} has unsupported version {version:?}",
            manifest_path.display()
        ));
    }
    let id = required(&manifest.id, "id", &manifest_path)?.to_string();
    validate_zone_id(&id)?;
    if directory_name != id {
        return Err(format!(
            "zone bundle identity mismatch: directory {directory_name:?} declares id {id:?}"
        ));
    }
    let display_name =
        required(&manifest.display_name, "display_name", &manifest_path)?.to_string();
    validate_display_name(&display_name)?;
    if let Some(override_id) = &manifest.override_id {
        validate_zone_id(override_id)?;
        if override_id != &id {
            return Err(format!(
                "zone bundle {id:?} may only override the same authenticated identity, got {override_id:?}"
            ));
        }
    }

    let map_filename = required(&manifest.map, "map", &manifest_path)?.to_string();
    validate_bundle_filename(&map_filename, "lvl")?;
    let settings_filename = required(&manifest.settings, "settings", &manifest_path)?;
    validate_settings_filename(settings_filename)?;
    let news_filename = required(&manifest.news, "news", &manifest_path)?.to_string();
    validate_bundle_filename(&news_filename, "txt")?;

    let mut bundle_budget = SourceReadBudget::new(MAX_BUNDLE_BYTES);
    let map_bytes = read_manifest_file(
        &directory,
        &map_filename,
        MAX_MAP_BYTES,
        required(&manifest.map_sha256, "map_sha256", &manifest_path)?,
        &mut bundle_budget,
        catalog_budget,
    )?;
    let settings_bytes = read_manifest_file(
        &directory,
        settings_filename,
        MAX_SETTINGS_BYTES,
        required(&manifest.settings_sha256, "settings_sha256", &manifest_path)?,
        &mut bundle_budget,
        catalog_budget,
    )?;
    let news_bytes = read_manifest_file(
        &directory,
        &news_filename,
        MAX_NEWS_BYTES,
        required(&manifest.news_sha256, "news_sha256", &manifest_path)?,
        &mut bundle_budget,
        catalog_budget,
    )?;

    if manifest.lvz.len() > MAX_LVZ_FILES {
        return Err(format!(
            "zone bundle {id:?} declares {} LVZ files; maximum is {MAX_LVZ_FILES}",
            manifest.lvz.len()
        ));
    }
    let mut seen_files = BTreeSet::new();
    for filename in [&map_filename, settings_filename, &news_filename] {
        seen_files.insert(filename.to_ascii_lowercase());
    }
    let mut lvz_files = Vec::with_capacity(manifest.lvz.len());
    for (filename, digest) in manifest.lvz {
        validate_bundle_filename(&filename, "lvz")?;
        if !seen_files.insert(filename.to_ascii_lowercase()) {
            return Err(format!("zone bundle {id:?} repeats filename {filename:?}"));
        }
        let bytes = read_manifest_file(
            &directory,
            &filename,
            MAX_LVZ_BYTES,
            &digest,
            &mut bundle_budget,
            catalog_budget,
        )?;
        lvz_files.push((filename, bytes));
    }

    let total_bytes = map_bytes.len() as u64
        + settings_bytes.len() as u64
        + news_bytes.len() as u64
        + lvz_files
            .iter()
            .map(|(_, bytes)| bytes.len() as u64)
            .sum::<u64>();
    if total_bytes > MAX_BUNDLE_BYTES {
        return Err(format!(
            "zone bundle {id:?} contains {total_bytes} bytes; maximum is {MAX_BUNDLE_BYTES}"
        ));
    }

    Ok(DiskZoneDefinition {
        id,
        display_name,
        override_id: manifest.override_id,
        map_filename,
        map_bytes,
        settings_bytes,
        news_filename,
        news_bytes,
        lvz_files,
    })
}

fn parse_manifest(text: &str, path: &Path) -> Result<ParsedManifest, String> {
    let mut parsed = ParsedManifest::default();
    let mut scalar_keys = BTreeSet::new();
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("{}:{line_number}: expected key=value", path.display()))?;
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            return Err(format!(
                "{}:{line_number}: bundle keys and values must not be empty",
                path.display()
            ));
        }
        if key == "lvz" {
            let (filename, digest) = value.split_once(',').ok_or_else(|| {
                format!(
                    "{}:{line_number}: lvz must be filename,sha256",
                    path.display()
                )
            })?;
            parsed
                .lvz
                .push((filename.trim().to_string(), digest.trim().to_string()));
            continue;
        }
        if !scalar_keys.insert(key.to_string()) {
            return Err(format!(
                "{}:{line_number}: duplicate key {key:?}",
                path.display()
            ));
        }
        let target = match key {
            "version" => &mut parsed.version,
            "id" => &mut parsed.id,
            "display_name" => &mut parsed.display_name,
            "override" => &mut parsed.override_id,
            "map" => &mut parsed.map,
            "map_sha256" => &mut parsed.map_sha256,
            "settings" => &mut parsed.settings,
            "settings_sha256" => &mut parsed.settings_sha256,
            "news" => &mut parsed.news,
            "news_sha256" => &mut parsed.news_sha256,
            _ => {
                return Err(format!(
                    "{}:{line_number}: unsupported bundle key {key:?}",
                    path.display()
                ));
            }
        };
        *target = Some(value.to_string());
    }
    Ok(parsed)
}

fn required<'a>(value: &'a Option<String>, key: &str, path: &Path) -> Result<&'a str, String> {
    value.as_deref().ok_or_else(|| {
        format!(
            "zone bundle manifest {} is missing required key {key:?}",
            path.display()
        )
    })
}

fn read_manifest_file(
    directory: &descriptor::Directory,
    filename: &str,
    max_bytes: u64,
    expected_sha256: &str,
    bundle_budget: &mut SourceReadBudget,
    catalog_budget: &mut SourceReadBudget,
) -> Result<Vec<u8>, String> {
    validate_sha256(expected_sha256)?;
    let allowed = max_bytes
        .min(bundle_budget.remaining())
        .min(catalog_budget.remaining());
    let bytes = directory.read_file(filename, allowed)?;
    bundle_budget.consume(bytes.len() as u64)?;
    catalog_budget.consume(bytes.len() as u64)?;
    let actual = sha256_hex(&bytes);
    if actual != expected_sha256 {
        return Err(format!(
            "zone bundle checksum mismatch for {}: expected {expected_sha256}, got {actual}",
            directory.display_path(filename).display()
        ));
    }
    Ok(bytes)
}

#[derive(Clone, Copy, Debug)]
struct SourceReadBudget {
    remaining: u64,
}

impl SourceReadBudget {
    fn new(maximum: u64) -> Self {
        Self { remaining: maximum }
    }

    fn remaining(&self) -> u64 {
        self.remaining
    }

    fn consume(&mut self, bytes: u64) -> Result<(), String> {
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or_else(|| "zone catalog source read budget underflow".to_string())?;
        Ok(())
    }

    fn read(
        &mut self,
        directory: &descriptor::Directory,
        filename: &str,
        per_file_maximum: u64,
    ) -> Result<Vec<u8>, String> {
        let bytes = directory.read_file(filename, per_file_maximum.min(self.remaining))?;
        self.consume(bytes.len() as u64)?;
        Ok(bytes)
    }
}

fn validate_zone_id(id: &str) -> Result<(), String> {
    if !crate::zone_wire::valid_zone_id(id) {
        return Err(format!(
            "invalid zone id {id:?}; expected 1-15 lowercase ASCII letters, digits, or interior hyphens"
        ));
    }
    Ok(())
}

fn validate_display_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 32
        || !name.is_ascii()
        || name.bytes().any(|byte| byte.is_ascii_control())
        || super::normalized_zone_lookup_name(name).is_empty()
    {
        return Err(format!("invalid zone display_name {name:?}"));
    }
    Ok(())
}

fn validate_settings_filename(filename: &str) -> Result<(), String> {
    validate_plain_filename(filename)?;
    let extension = Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !["cfg", "conf", "ini", "set"]
        .iter()
        .any(|allowed| extension.eq_ignore_ascii_case(allowed))
    {
        return Err(format!(
            "zone settings filename has unsupported extension: {filename:?}"
        ));
    }
    Ok(())
}

fn validate_bundle_filename(filename: &str, extension: &str) -> Result<(), String> {
    crate::zone_wire::validate_wire_asset_filename(filename, extension)
}

fn validate_plain_filename(filename: &str) -> Result<(), String> {
    crate::zone_wire::validate_plain_bundle_filename(filename)
}

fn validate_sha256(digest: &str) -> Result<(), String> {
    crate::zone_wire::parse_sha256_hex(digest).map(|_| ())
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd"
))]
mod descriptor {
    use std::ffi::{CStr, CString, OsStr};
    use std::fs::{File, Metadata, OpenOptions};
    use std::io::{Read, Seek, SeekFrom};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum EntryKind {
        Regular,
        Directory,
        Symlink,
        Other,
    }

    pub(super) struct Entry {
        pub(super) name: String,
        pub(super) kind: EntryKind,
    }

    pub(super) struct Directory {
        file: File,
        diagnostic_path: PathBuf,
    }

    impl Directory {
        pub(super) fn open_root(path: &Path) -> Result<Self, String> {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)
                .map_err(|err| {
                    format!(
                        "zone bundle root {} must be a real directory opened without following symlinks: {err}",
                        path.display()
                    )
                })?;
            Ok(Self {
                file,
                diagnostic_path: path.to_path_buf(),
            })
        }

        pub(super) fn display(&self) -> &Path {
            &self.diagnostic_path
        }

        pub(super) fn display_path(&self, name: &str) -> PathBuf {
            self.diagnostic_path.join(name)
        }

        pub(super) fn entries(&self, maximum_entries: usize) -> Result<Vec<Entry>, String> {
            self.entries_with_reader(maximum_entries, |directory, _accepted_entries| unsafe {
                libc::readdir(directory)
            })
        }

        fn entries_with_reader(
            &self,
            maximum_entries: usize,
            mut read_entry: impl FnMut(*mut libc::DIR, usize) -> *mut libc::dirent,
        ) -> Result<Vec<Entry>, String> {
            let duplicate = unsafe { libc::dup(self.file.as_raw_fd()) };
            if duplicate < 0 {
                return Err(format!(
                    "failed to duplicate zone directory descriptor {}: {}",
                    self.display().display(),
                    std::io::Error::last_os_error()
                ));
            }
            let directory = unsafe { libc::fdopendir(duplicate) };
            if directory.is_null() {
                unsafe { libc::close(duplicate) };
                return Err(format!(
                    "failed to enumerate zone directory {}: {}",
                    self.display().display(),
                    std::io::Error::last_os_error()
                ));
            }
            struct DirectoryStream(*mut libc::DIR);
            impl Drop for DirectoryStream {
                fn drop(&mut self) {
                    unsafe { libc::closedir(self.0) };
                }
            }
            let stream = DirectoryStream(directory);
            let mut entries = Vec::new();
            loop {
                // `readdir` returns a pointer owned by the directory stream.
                clear_errno();
                let raw = read_entry(stream.0, entries.len());
                if raw.is_null() {
                    let errno = current_errno();
                    if errno != 0 {
                        return Err(format!(
                            "failed while enumerating zone directory {}: {}",
                            self.display().display(),
                            std::io::Error::from_raw_os_error(errno)
                        ));
                    }
                    break;
                }
                let name_bytes = unsafe { CStr::from_ptr((*raw).d_name.as_ptr()) }.to_bytes();
                if name_bytes == b"." || name_bytes == b".." {
                    continue;
                }
                if entries.len() >= maximum_entries {
                    return Err(format!(
                        "zone directory {} contains more than {maximum_entries} entries",
                        self.display().display()
                    ));
                }
                let name = std::str::from_utf8(name_bytes).map_err(|_| {
                    format!(
                        "zone directory {} contains a non-UTF-8 entry",
                        self.display().display()
                    )
                })?;
                let c_name = CString::new(name_bytes).map_err(|_| {
                    format!(
                        "zone directory {} contains an invalid NUL filename",
                        self.display().display()
                    )
                })?;
                let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
                let result = unsafe {
                    libc::fstatat(
                        self.file.as_raw_fd(),
                        c_name.as_ptr(),
                        stat.as_mut_ptr(),
                        libc::AT_SYMLINK_NOFOLLOW,
                    )
                };
                if result != 0 {
                    return Err(format!(
                        "zone directory entry {}/{} changed during catalog scan: {}",
                        self.display().display(),
                        name,
                        std::io::Error::last_os_error()
                    ));
                }
                let stat = unsafe { stat.assume_init() };
                let kind = match stat.st_mode & libc::S_IFMT {
                    libc::S_IFREG => EntryKind::Regular,
                    libc::S_IFDIR => EntryKind::Directory,
                    libc::S_IFLNK => EntryKind::Symlink,
                    _ => EntryKind::Other,
                };
                entries.push(Entry {
                    name: name.to_string(),
                    kind,
                });
            }
            entries.sort_by(|left, right| left.name.cmp(&right.name));
            Ok(entries)
        }

        #[cfg(test)]
        pub(super) fn entries_with_forced_error_after(
            &self,
            maximum_entries: usize,
            accepted_entries: usize,
        ) -> Result<Vec<Entry>, String> {
            self.entries_with_reader(maximum_entries, |directory, accepted| {
                if accepted >= accepted_entries {
                    set_errno(libc::EIO);
                    std::ptr::null_mut()
                } else {
                    unsafe { libc::readdir(directory) }
                }
            })
        }

        pub(super) fn open_directory(&self, name: &str) -> Result<Self, String> {
            let c_name = safe_name(name)?;
            let descriptor = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    c_name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if descriptor < 0 {
                return Err(format!(
                    "zone bundle directory {}/{} could not be opened without following symlinks: {}",
                    self.display().display(),
                    name,
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self {
                file: unsafe { File::from_raw_fd(descriptor) },
                diagnostic_path: self.display_path(name),
            })
        }

        pub(super) fn read_file(&self, name: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
            self.read_file_with_hook(name, max_bytes, || {})
        }

        fn read_file_with_hook(
            &self,
            name: &str,
            max_bytes: u64,
            hook: impl FnOnce(),
        ) -> Result<Vec<u8>, String> {
            let c_name = safe_name(name)?;
            let descriptor = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    c_name.as_ptr(),
                    libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if descriptor < 0 {
                return Err(format!(
                    "zone bundle file {}/{} could not be opened without following symlinks: {}",
                    self.display().display(),
                    name,
                    std::io::Error::last_os_error()
                ));
            }
            let mut file = unsafe { File::from_raw_fd(descriptor) };
            let before = file.metadata().map_err(|err| {
                format!(
                    "failed to inspect opened zone bundle file {}/{}: {err}",
                    self.display().display(),
                    name
                )
            })?;
            if !before.file_type().is_file() {
                return Err(format!(
                    "zone bundle file {}/{} is not a regular file",
                    self.display().display(),
                    name
                ));
            }
            if before.len() > max_bytes {
                return Err(format!(
                    "zone bundle file {}/{} is {} bytes; maximum is {max_bytes}",
                    self.display().display(),
                    name,
                    before.len()
                ));
            }
            hook();
            file.seek(SeekFrom::Start(0)).map_err(|err| {
                format!(
                    "failed to seek opened zone bundle file {}/{}: {err}",
                    self.display().display(),
                    name
                )
            })?;
            let mut bytes = Vec::with_capacity(before.len().min(max_bytes) as usize);
            (&mut file)
                .take(max_bytes.saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(|err| {
                    format!(
                        "failed to read zone bundle file {}/{}: {err}",
                        self.display().display(),
                        name
                    )
                })?;
            if bytes.len() as u64 > max_bytes {
                return Err(format!(
                    "zone bundle file {}/{} exceeded the {max_bytes}-byte limit while reading",
                    self.display().display(),
                    name
                ));
            }
            let after = file.metadata().map_err(|err| {
                format!(
                    "failed to re-inspect opened zone bundle file {}/{}: {err}",
                    self.display().display(),
                    name
                )
            })?;
            if bytes.len() as u64 != after.len() || !same_file_state(&before, &after) {
                return Err(format!(
                    "zone bundle file {}/{} changed while it was being read",
                    self.display().display(),
                    name
                ));
            }
            Ok(bytes)
        }
    }

    #[cfg(any(target_os = "linux", target_os = "dragonfly"))]
    fn errno_location() -> *mut libc::c_int {
        unsafe { libc::__errno_location() }
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    fn errno_location() -> *mut libc::c_int {
        unsafe { libc::__error() }
    }

    #[cfg(any(target_os = "android", target_os = "openbsd", target_os = "netbsd"))]
    fn errno_location() -> *mut libc::c_int {
        unsafe { libc::__errno() }
    }

    fn clear_errno() {
        set_errno(0);
    }

    fn set_errno(value: libc::c_int) {
        unsafe { *errno_location() = value };
    }

    fn current_errno() -> libc::c_int {
        unsafe { *errno_location() }
    }

    fn safe_name(name: &str) -> Result<CString, String> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || OsStr::new(name).as_bytes().contains(&b'/')
        {
            return Err(format!("unsafe descriptor-relative filename {name:?}"));
        }
        CString::new(name).map_err(|_| format!("filename contains NUL: {name:?}"))
    }

    fn same_file_state(left: &Metadata, right: &Metadata) -> bool {
        left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.len() == right.len()
            && left.mtime() == right.mtime()
            && left.mtime_nsec() == right.mtime_nsec()
            && left.ctime() == right.ctime()
            && left.ctime_nsec() == right.ctime_nsec()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::fs;
        use std::os::unix::fs::symlink;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

        fn root() -> PathBuf {
            let path = std::env::temp_dir().join(format!(
                "zubspace-descriptor-test-{}-{}",
                std::process::id(),
                NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create root");
            path
        }

        #[test]
        fn bounded_read_accepts_exact_limit_and_rejects_limit_plus_one() {
            let path = root();
            fs::write(path.join("exact"), b"1234").expect("write exact");
            fs::write(path.join("large"), b"12345").expect("write large");
            let directory = Directory::open_root(&path).expect("open root");
            assert_eq!(directory.read_file("exact", 4).unwrap(), b"1234");
            assert!(directory.read_file("large", 4).is_err());
            fs::remove_dir_all(path).expect("cleanup");
        }

        #[test]
        fn oversized_descriptor_is_rejected_before_the_read_allocation_seam() {
            use std::sync::atomic::AtomicBool;

            let path = root();
            fs::write(path.join("large"), b"12345").expect("write large");
            let directory = Directory::open_root(&path).expect("open root");
            let reached_read_seam = AtomicBool::new(false);
            assert!(
                directory
                    .read_file_with_hook("large", 4, || {
                        reached_read_seam.store(true, Ordering::Relaxed);
                    })
                    .is_err()
            );
            assert!(
                !reached_read_seam.load(Ordering::Relaxed),
                "fstat size must reject before allocating or reading"
            );
            fs::remove_dir_all(path).expect("cleanup");
        }

        #[test]
        fn opened_file_is_not_retargeted_by_swap_to_symlink() {
            let path = root();
            fs::write(path.join("asset"), b"trusted").expect("write trusted");
            fs::write(path.join("outside"), b"poisoned").expect("write outside");
            let directory = Directory::open_root(&path).expect("open root");
            let result = directory.read_file_with_hook("asset", 64, || {
                fs::rename(path.join("asset"), path.join("old")).expect("rename asset");
                symlink(path.join("outside"), path.join("asset")).expect("replace symlink");
            });
            assert!(
                result.is_err(),
                "a swap is detected from the opened descriptor state and must fail closed"
            );
            fs::remove_dir_all(path).expect("cleanup");
        }

        #[test]
        fn catalog_root_descriptor_is_stable_after_path_replacement() {
            let path = root();
            fs::create_dir(path.join("original-zone")).expect("original child");
            let replacement = root();
            fs::create_dir(replacement.join("poison-zone")).expect("poison child");
            let directory = Directory::open_root(&path).expect("open root");
            let moved = path.with_extension("moved");
            fs::rename(&path, &moved).expect("move root");
            symlink(&replacement, &path).expect("replace root path");
            let names = directory
                .entries(1)
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>();
            assert_eq!(names, vec!["original-zone"]);
            fs::remove_file(&path).expect("remove root symlink");
            fs::remove_dir_all(moved).expect("cleanup moved");
            fs::remove_dir_all(replacement).expect("cleanup replacement");
        }

        fn assert_exact_and_max_plus_one(path: &Path, maximum: usize) {
            let directory = Directory::open_root(path).expect("open exact bounded directory");
            assert_eq!(
                directory.entries(maximum).expect("exact entry cap").len(),
                maximum
            );
            // A duplicated directory descriptor shares its stream offset, so
            // each independent scan starts from a newly opened stable root.
            let directory = Directory::open_root(path).expect("reopen bounded directory");
            assert!(
                directory.entries(maximum - 1).is_err(),
                "cap+1 must reject before retaining the extra entry"
            );
        }

        #[test]
        fn enumeration_cap_counts_regular_files_directories_symlinks_and_mixed_entries() {
            let regular = root();
            for index in 0..4 {
                fs::write(regular.join(format!("file-{index}")), b"x").expect("regular entry");
            }
            assert_exact_and_max_plus_one(&regular, 4);

            let directories = root();
            for index in 0..4 {
                fs::create_dir(directories.join(format!("directory-{index}")))
                    .expect("directory entry");
            }
            assert_exact_and_max_plus_one(&directories, 4);

            let symlinks = root();
            for index in 0..4 {
                symlink("/dev/null", symlinks.join(format!("symlink-{index}")))
                    .expect("symlink entry");
            }
            assert_exact_and_max_plus_one(&symlinks, 4);

            let mixed = root();
            fs::write(mixed.join("regular"), b"x").expect("mixed regular");
            fs::create_dir(mixed.join("directory")).expect("mixed directory");
            symlink("/dev/null", mixed.join("symlink")).expect("mixed symlink");
            assert_exact_and_max_plus_one(&mixed, 3);

            for path in [regular, directories, symlinks, mixed] {
                fs::remove_dir_all(path).expect("cleanup entry-cap fixture");
            }
        }

        #[test]
        fn type_and_oversize_mutations_fail_on_the_opened_descriptor() {
            let path = root();
            fs::create_dir(path.join("directory-asset")).expect("directory asset");
            fs::write(path.join("growing"), b"1234").expect("growing asset");
            let directory = Directory::open_root(&path).expect("open root");
            assert!(directory.read_file("directory-asset", 64).is_err());
            assert!(
                directory
                    .read_file_with_hook("growing", 4, || {
                        fs::write(path.join("growing"), b"12345").expect("grow opened file");
                    })
                    .is_err(),
                "a file that grows after fstat must fail the max+1 read/post-fstat checks"
            );
            fs::remove_dir_all(path).expect("cleanup");
        }
    }
}

// Disk overrides require descriptor-relative O_NOFOLLOW semantics. Targets
// without that API still compile and retain the embedded catalog, but any
// configured disk root fails closed before reading a path.
#[cfg(any(
    test,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))
))]
#[allow(dead_code)]
mod unsupported_descriptor {
    use std::path::{Path, PathBuf};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum EntryKind {
        Regular,
        Directory,
        Symlink,
        Other,
    }

    pub(super) struct Entry {
        pub(super) name: String,
        pub(super) kind: EntryKind,
    }

    pub(super) struct Directory;

    impl Directory {
        pub(super) fn open_root(path: &Path) -> Result<Self, String> {
            Err(format!(
                "disk zone bundle root {} is unsupported on this target because descriptor-relative no-follow opens are unavailable; embedded zones remain available",
                path.display()
            ))
        }

        pub(super) fn display(&self) -> &Path {
            Path::new("")
        }

        pub(super) fn display_path(&self, name: &str) -> PathBuf {
            PathBuf::from(name)
        }

        pub(super) fn entries(&self, _maximum_entries: usize) -> Result<Vec<Entry>, String> {
            Err("disk zone bundles are unsupported on this target".to_string())
        }

        pub(super) fn open_directory(&self, _name: &str) -> Result<Self, String> {
            Err("disk zone bundles are unsupported on this target".to_string())
        }

        pub(super) fn read_file(&self, _name: &str, _max_bytes: u64) -> Result<Vec<u8>, String> {
            Err("disk zone bundles are unsupported on this target".to_string())
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd"
)))]
use unsupported_descriptor as descriptor;

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(1);

    pub(crate) struct TempZoneRoot {
        pub(crate) path: PathBuf,
    }

    impl TempZoneRoot {
        pub(crate) fn new() -> Self {
            let suffix = NEXT_TEMP_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "zubspace-zone-bundle-{}-{suffix}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary zone root");
            Self { path }
        }

        pub(crate) fn write_valid_bundle(
            &self,
            id: &str,
            display_name: &str,
            override_id: Option<&str>,
        ) -> PathBuf {
            let directory = self.path.join(id);
            fs::create_dir(&directory).expect("zone directory");
            let map = super::super::assets::SERVER_MAP_BYTES;
            let settings = include_bytes!("../../../../assets/zones/extreme/arena.conf");
            let news = b"Temporary zone\n";
            fs::write(directory.join("test.lvl"), map).expect("map fixture");
            fs::write(directory.join("arena.conf"), settings).expect("settings fixture");
            fs::write(directory.join("news.txt"), news).expect("news fixture");
            let override_line = override_id
                .map(|value| format!("override={value}\n"))
                .unwrap_or_default();
            let manifest = format!(
                "version=1\nid={id}\ndisplay_name={display_name}\n{override_line}map=test.lvl\nmap_sha256={}\nsettings=arena.conf\nsettings_sha256={}\nnews=news.txt\nnews_sha256={}\n",
                sha256_hex(map),
                sha256_hex(settings),
                sha256_hex(news),
            );
            fs::write(directory.join(BUNDLE_MANIFEST_FILENAME), manifest).expect("manifest");
            directory
        }
    }

    impl Drop for TempZoneRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn directory_source_bytes(directory: &Path) -> u64 {
        fs::read_dir(directory)
            .expect("bundle directory")
            .map(|entry| {
                entry
                    .expect("bundle entry")
                    .metadata()
                    .expect("metadata")
                    .len()
            })
            .sum()
    }

    #[test]
    fn valid_bundle_loads_all_authenticated_inputs() {
        let root = TempZoneRoot::new();
        root.write_valid_bundle("test-zone", "Test Zone", None);
        let definitions = load_disk_zone_definitions(&root.path).expect("valid bundle");
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].id, "test-zone");
        assert_eq!(
            definitions[0].map_bytes,
            super::super::assets::SERVER_MAP_BYTES
        );
    }

    #[test]
    fn catalog_root_entry_cap_accepts_exact_and_rejects_max_plus_one() {
        let root = TempZoneRoot::new();
        for index in 0..MAX_CATALOG_ROOT_ENTRIES {
            fs::write(root.path.join(format!("metadata-{index:03}")), b"x")
                .expect("root metadata entry");
        }
        assert!(
            load_disk_zone_definitions(&root.path)
                .expect("exact root entry cap")
                .is_empty()
        );
        fs::write(root.path.join("metadata-over-cap"), b"x").expect("over-cap root entry");
        let error = load_disk_zone_definitions(&root.path)
            .err()
            .expect("root cap+1 must reject");
        assert!(
            error.contains(&format!("more than {MAX_CATALOG_ROOT_ENTRIES} entries")),
            "unexpected root entry-cap error: {error}"
        );
    }

    #[test]
    fn bundle_directory_entry_cap_accepts_exact_and_rejects_max_plus_one() {
        let root = TempZoneRoot::new();
        let directory = root.write_valid_bundle("test-zone", "Test Zone", None);
        let baseline = fs::read_dir(&directory).expect("bundle directory").count();
        assert_eq!(baseline, 4);
        for index in baseline..MAX_BUNDLE_DIRECTORY_ENTRIES {
            fs::write(directory.join(format!("extra-{index:03}")), b"x")
                .expect("bundle extra entry");
        }
        assert_eq!(
            load_disk_zone_definitions(&root.path)
                .expect("exact bundle directory entry cap")
                .len(),
            1
        );
        fs::write(directory.join("extra-over-cap"), b"x").expect("over-cap bundle entry");
        let error = load_disk_zone_definitions(&root.path)
            .err()
            .expect("bundle cap+1 must reject");
        assert!(
            error.contains(&format!("more than {MAX_BUNDLE_DIRECTORY_ENTRIES} entries")),
            "unexpected bundle entry-cap error: {error}"
        );
    }

    #[test]
    fn catalog_read_budget_accepts_exact_bytes_and_rejects_max_plus_one_preflight() {
        let root = TempZoneRoot::new();
        let directory = root.write_valid_bundle("test-zone", "Test Zone", None);
        let exact = directory_source_bytes(&directory);
        assert_eq!(
            load_disk_zone_definitions_with_source_budget(&root.path, exact)
                .expect("exact source budget")
                .len(),
            1
        );
        let error = load_disk_zone_definitions_with_source_budget(&root.path, exact - 1)
            .err()
            .expect("one byte over the remaining budget must fail");
        assert!(error.contains("maximum is"), "unexpected error: {error}");
    }

    #[test]
    fn late_bundle_cannot_read_past_the_running_catalog_budget() {
        let root = TempZoneRoot::new();
        let first = root.write_valid_bundle("alpha-zone", "Alpha Zone", None);
        let second = root.write_valid_bundle("beta-zone", "Beta Zone", None);
        let budget = directory_source_bytes(&first)
            + fs::metadata(second.join(BUNDLE_MANIFEST_FILENAME))
                .expect("second manifest metadata")
                .len();
        let error = load_disk_zone_definitions_with_source_budget(&root.path, budget)
            .err()
            .expect("the second bundle's first asset must exceed the remaining budget");
        assert!(
            error.contains("maximum is 0"),
            "the descriptor must reject from the exhausted running budget before reading: {error}"
        );
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    #[test]
    fn partial_root_enumeration_eio_rejects_the_entire_catalog() {
        let root = TempZoneRoot::new();
        root.write_valid_bundle("alpha-zone", "Alpha Zone", None);
        root.write_valid_bundle("beta-zone", "Beta Zone", None);
        let error = load_disk_zone_definitions_with_forced_enumeration_error(&root.path, 1)
            .err()
            .expect("a readdir EIO after one accepted entry must reject the catalog");
        assert!(error.contains("enumerating zone directory"), "{error}");
        assert!(error.contains("Input/output error"), "{error}");
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    #[test]
    fn unsupported_target_embedded_only_loader_path_is_compile_checked_and_fails_closed() {
        let error = unsupported_descriptor::Directory::open_root(Path::new("ignored"))
            .err()
            .expect("unsupported descriptor backend must reject disk roots");
        assert!(error.contains("embedded zones remain available"));
    }

    #[test]
    fn malformed_partial_and_identity_mismatched_bundles_fail_closed() {
        for mutation in ["missing", "duplicate", "identity"] {
            let root = TempZoneRoot::new();
            let directory = root.write_valid_bundle("test-zone", "Test Zone", None);
            let manifest_path = directory.join(BUNDLE_MANIFEST_FILENAME);
            let mut manifest = fs::read_to_string(&manifest_path).expect("manifest text");
            match mutation {
                "missing" => manifest = manifest.replace("news=news.txt\n", ""),
                "duplicate" => manifest.push_str("map=test.lvl\n"),
                "identity" => manifest = manifest.replace("id=test-zone", "id=other-zone"),
                _ => unreachable!(),
            }
            fs::write(manifest_path, manifest).expect("mutated manifest");
            assert!(
                load_disk_zone_definitions(&root.path).is_err(),
                "{mutation} bundle must fail"
            );
        }
    }

    #[test]
    fn traversal_checksum_and_oversized_files_fail_closed() {
        for mutation in [
            "traversal",
            "backslash",
            "whitespace",
            "embedded-dotdot",
            "checksum",
            "oversized",
        ] {
            let root = TempZoneRoot::new();
            let directory = root.write_valid_bundle("test-zone", "Test Zone", None);
            let manifest_path = directory.join(BUNDLE_MANIFEST_FILENAME);
            let mut manifest = fs::read_to_string(&manifest_path).expect("manifest text");
            match mutation {
                "traversal" => manifest = manifest.replace("map=test.lvl", "map=../test.lvl"),
                "backslash" => manifest = manifest.replace("map=test.lvl", "map=bad\\map.lvl"),
                "whitespace" => manifest = manifest.replace("map=test.lvl", "map=bad map.lvl"),
                "embedded-dotdot" => {
                    manifest = manifest.replace("map=test.lvl", "map=bad..map.lvl")
                }
                "checksum" => {
                    manifest = manifest.replacen(
                        &format!(
                            "map_sha256={}",
                            sha256_hex(super::super::assets::SERVER_MAP_BYTES)
                        ),
                        &format!("map_sha256={}", "0".repeat(64)),
                        1,
                    )
                }
                "oversized" => {
                    let bytes = vec![b'x'; MAX_NEWS_BYTES as usize + 1];
                    fs::write(directory.join("news.txt"), &bytes).expect("oversized news");
                    manifest = manifest.replacen(
                        &format!("news_sha256={}", sha256_hex(b"Temporary zone\n")),
                        &format!("news_sha256={}", sha256_hex(&bytes)),
                        1,
                    );
                }
                _ => unreachable!(),
            }
            fs::write(manifest_path, manifest).expect("mutated manifest");
            assert!(
                load_disk_zone_definitions(&root.path).is_err(),
                "{mutation} bundle must fail"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_bundle_files_and_directories_fail_closed() {
        use std::os::unix::fs::symlink;

        let root = TempZoneRoot::new();
        let directory = root.write_valid_bundle("test-zone", "Test Zone", None);
        fs::remove_file(directory.join("test.lvl")).expect("remove map");
        symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/zones/chaos-zone/bdhadar.lvl"),
            directory.join("test.lvl"),
        )
        .expect("map symlink");
        assert!(load_disk_zone_definitions(&root.path).is_err());

        let second_root = TempZoneRoot::new();
        symlink(&directory, second_root.path.join("linked-zone")).expect("directory symlink");
        assert!(load_disk_zone_definitions(&second_root.path).is_err());

        let third_root = TempZoneRoot::new();
        let third_directory = third_root.write_valid_bundle("test-zone", "Test Zone", None);
        symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"),
            third_directory.join("unlisted.txt"),
        )
        .expect("unlisted symlink");
        assert!(load_disk_zone_definitions(&third_root.path).is_err());

        let fourth_root = TempZoneRoot::new();
        let root_link = fourth_root.path.join("catalog-link");
        symlink(&third_root.path, &root_link).expect("root symlink");
        assert!(load_disk_zone_definitions(&root_link).is_err());
    }
}
