//! Metadata-only acquisition policy and real pinned package verification.
use super::acquisition::{confined, hex_digest, invalid, read_bounded, require};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

const MAX_ARCHIVE: usize = 8 * 1024 * 1024;
const MAX_SOURCE: usize = 4 * 1024 * 1024;
const MAX_TEXT: usize = 128 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    asset_policy: Policy,
    programs: Vec<Program>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    repository: String,
    program_files: String,
    observations: String,
    release_assets: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Program {
    id: String,
    version: String,
    url: String,
    sha256: String,
    program_license: String,
    license_notice: String,
    source_archive: Source,
    application_root: String,
    working_directory: String,
    command: String,
    configuration: Configuration,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    path: String,
    settings: BTreeMap<String, String>,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Packages {
    schema_version: u32,
    raw_root: PathBuf,
    files: BTreeMap<String, PathBuf>,
}

fn relative(value: &str, root_allowed: bool) -> io::Result<()> {
    if root_allowed && value == "." {
        return Ok(());
    }
    require(
        !value.is_empty()
            && value.len() <= 256
            && !value.contains(['\\', ':', '\0'])
            && Path::new(value)
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "asset path must be confined and portable",
    )
}

fn pin(value: &str) -> io::Result<()> {
    require(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "asset SHA256 must be lowercase hexadecimal",
    )
}

fn load(path: &Path) -> io::Result<Manifest> {
    parse(&read_bounded(path, 64 * 1024)?)
}

fn parse(bytes: &[u8]) -> io::Result<Manifest> {
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    require(manifest.schema_version == 1, "unsupported asset schema")?;
    let policy = &manifest.asset_policy;
    require(
        policy.repository == "acquisition metadata only"
            && policy.program_files
                == "temporary test guests only; preserve bundled source and notices"
            && policy.observations == "generate from live emulator and SDK; retain locally only"
            && policy.release_assets == "exclude downloaded programs, sources and observations",
        "asset publication policy changed",
    )?;
    let ids: BTreeSet<_> = manifest
        .programs
        .iter()
        .map(|program| program.id.as_str())
        .collect();
    require(
        manifest.programs.len() == 2 && ids == BTreeSet::from(["grafx2", "seal"]),
        "required approved programs missing or duplicated",
    )?;
    for program in &manifest.programs {
        let category = if program.id == "grafx2" {
            "apps"
        } else {
            "gui"
        };
        require(
            program.url
                == format!(
                    "https://www.ibiblio.org/pub/micro/pc-stuff/freedos/files/repositories/latest/{category}/{}.zip",
                    program.id
                )
                && !program.version.is_empty()
                && program.version.len() <= 32
                && program
                    .version
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte)),
            "unapproved package source or version",
        )?;
        require(
            matches!(
                program.program_license.as_str(),
                "GPL-2.0-only" | "GPL-2.0-or-later"
            ),
            "program license needs a new decision",
        )?;
        for digest in [
            &program.sha256,
            &program.source_archive.sha256,
            &program.configuration.sha256,
        ] {
            pin(digest)?;
        }
        for path in [
            &program.application_root,
            &program.configuration.path,
            &program.source_archive.path,
        ] {
            relative(path, false)?;
        }
        relative(&program.working_directory, true)?;
        let (license_archive, license_path) = program.license_notice.split_once(':').map_or(
            (None, program.license_notice.as_str()),
            |(archive, member)| (Some(archive), member),
        );
        if let Some(archive) = license_archive {
            require(
                archive == program.source_archive.path,
                "license source archive differs",
            )?;
        }
        relative(license_path, false)?;
        let words: Vec<_> = program.command.split_whitespace().collect();
        require(
            !words.is_empty()
                && words.len() <= 8
                && words[0].ends_with(".EXE")
                && words.iter().all(|word| {
                    word.len() <= 128
                        && word
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"/.-_=".contains(&byte))
                }),
            "invalid DOS acquisition command",
        )?;
        relative(words[0], false)?;
        require(
            !program.configuration.settings.is_empty()
                && program.configuration.settings.len() <= 8
                && program.configuration.settings.iter().all(|(key, value)| {
                    !key.is_empty()
                        && key.len() <= 64
                        && !value.is_empty()
                        && value.len() <= 256
                        && key.is_ascii()
                        && value.is_ascii()
                        && !key.chars().any(char::is_control)
                        && !value.chars().any(char::is_control)
                }),
            "invalid application configuration",
        )?;
    }
    Ok(manifest)
}

pub(super) fn approvals(path: &Path) -> io::Result<()> {
    let manifest = load(path)?;
    println!(
        "Verified metadata-only asset policy for {} pinned programs; live acquisition checks remain separate",
        manifest.programs.len()
    );
    Ok(())
}

struct Archive {
    bytes: Vec<u8>,
    entries: Vec<rust_dos_zip::ZipEntry>,
}

fn directory_bound(bytes: &[u8]) -> io::Result<()> {
    // These pinned DOS packages use ordinary ZIPs. Bound the directory before
    // the general reader allocates its entry table; Zip64 needs a new review.
    let footer = bytes
        .len()
        .checked_sub(22)
        .and_then(|end| {
            (end.saturating_sub(65535)..=end)
                .rev()
                .find(|&at| bytes[at..at + 4] == *b"PK\x05\x06")
        })
        .ok_or_else(|| invalid("ZIP footer missing"))?;
    let header = &bytes[footer..footer + 22];
    let word = |at| u16::from_le_bytes([header[at], header[at + 1]]) as usize;
    let entries = word(10);
    require(
        word(4) == 0
            && word(6) == 0
            && word(8) == entries
            && (1..=2048).contains(&entries)
            && header[12..16] != [255; 4]
            && header[16..20] != [255; 4]
            && (footer < 20 || bytes[footer - 20..footer - 16] != *b"PK\x06\x07")
            && footer + 22 + word(20) == bytes.len(),
        "unsupported ZIP layout or entry count",
    )
}

impl Archive {
    fn pinned(bytes: Vec<u8>, expected: &str) -> io::Result<Self> {
        let mut digest = Sha256::new();
        digest.update(&bytes);
        require(
            hex_digest(digest) == expected,
            "package/source bytes differ from approved pin",
        )?;
        directory_bound(&bytes)?;
        let entries = rust_dos_zip::central_directory(&mut io::Cursor::new(&bytes))
            .map_err(io::Error::other)?;
        require(
            !entries.is_empty() && entries.len() <= 2048,
            "archive entry count bound",
        )?;
        let mut total = 0u64;
        let mut names = BTreeSet::new();
        for entry in &entries {
            relative(&entry.path, false)?;
            require(names.insert(&entry.path), "duplicate archive member")?;
            require(
                !entry.encrypted && matches!(entry.method, 0 | 8),
                "unsupported archive member",
            )?;
            total = total
                .checked_add(entry.size)
                .ok_or_else(|| invalid("archive size overflow"))?;
            require(total <= 64 * 1024 * 1024, "archive expansion bound")?;
        }
        Ok(Self { bytes, entries })
    }

    fn member(&self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.path == path && !entry.dir)
            .ok_or_else(|| invalid("required package member missing"))?;
        require(
            entry.size > 0 && entry.size <= limit as u64,
            "package member byte bound",
        )?;
        let start = usize::try_from(entry.data_at).map_err(io::Error::other)?;
        let count = usize::try_from(entry.compressed).map_err(io::Error::other)?;
        let end = start
            .checked_add(count)
            .ok_or_else(|| invalid("compressed member overflow"))?;
        let packed = self
            .bytes
            .get(start..end)
            .ok_or_else(|| invalid("compressed member outside archive"))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(entry.size as usize)
            .map_err(io::Error::other)?;
        if entry.method == 0 {
            require(packed.len() <= limit, "stored member byte bound")?;
            bytes.extend_from_slice(packed);
        } else {
            flate2::read::DeflateDecoder::new(packed)
                .take(limit as u64 + 1)
                .read_to_end(&mut bytes)?;
        }
        require(
            bytes.len() == entry.size as usize
                && bytes.len() <= limit
                && crc32fast::hash(&bytes) == entry.crc,
            "package member size/CRC mismatch",
        )?;
        Ok(bytes)
    }
}

pub(super) fn verify_packages(manifest_path: &Path, catalog_path: &Path) -> io::Result<()> {
    let manifest = load(manifest_path)?;
    let packages: Packages = serde_json::from_slice(&read_bounded(catalog_path, 64 * 1024)?)
        .map_err(io::Error::other)?;
    require(
        packages.schema_version == 1
            && packages.raw_root.is_absolute()
            && packages.files.len() == manifest.programs.len(),
        "invalid actual package catalog",
    )?;
    let root = packages.raw_root.canonicalize()?;
    for program in &manifest.programs {
        let path = packages
            .files
            .get(&program.id)
            .ok_or_else(|| invalid("required acquired package missing"))?;
        let package = Archive::pinned(
            read_bounded(&confined(&root, path)?, MAX_ARCHIVE)?,
            &program.sha256,
        )?;
        let lsm = package.member(
            &format!("APPINFO/{}.LSM", program.id.to_ascii_uppercase()),
            MAX_TEXT,
        )?;
        let lsm = std::str::from_utf8(&lsm).map_err(io::Error::other)?;
        let versions: Vec<_> = lsm
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter(|(key, _)| key.trim().eq_ignore_ascii_case("Version"))
            .map(|(_, value)| value.trim())
            .collect();
        require(
            versions == [program.version.as_str()],
            "package version differs from metadata",
        )?;
        let source = Archive::pinned(
            package.member(&program.source_archive.path, MAX_SOURCE)?,
            &program.source_archive.sha256,
        )?;
        let notice = if let Some((_, member)) = program.license_notice.split_once(':') {
            source.member(member, MAX_TEXT)?
        } else {
            package.member(&program.license_notice, MAX_TEXT)?
        };
        let notice = std::str::from_utf8(&notice).map_err(io::Error::other)?;
        require(
            notice.contains("GNU GENERAL PUBLIC LICENSE") && notice.contains("Version 2"),
            "actual GPLv2 notice missing",
        )?;
        let executable = program
            .command
            .split_whitespace()
            .next()
            .ok_or_else(|| invalid("DOS executable missing"))?;
        let executable = if program.working_directory == "." {
            format!("{}/{executable}", program.application_root)
        } else {
            format!(
                "{}/{}/{executable}",
                program.application_root, program.working_directory
            )
        };
        package.member(&executable, MAX_SOURCE)?;
        package.member(
            &format!(
                "{}/{}",
                program.application_root, program.configuration.path
            ),
            MAX_TEXT,
        )?;
    }
    println!(
        "Verified {} actual pinned program/source archives and bundled notices; mouse observations remain separate",
        manifest.programs.len()
    );
    Ok(())
}

fn configured(original: &[u8], settings: &BTreeMap<String, String>) -> io::Result<Vec<u8>> {
    let original = std::str::from_utf8(original).map_err(io::Error::other)?;
    let mut plain = BTreeMap::new();
    let mut sections: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
    for (key, value) in settings {
        if let Some((section, key)) = key.split_once('.') {
            sections.entry(section).or_default().insert(key, value);
        } else {
            plain.insert(key.as_str(), value.as_str());
        }
    }
    let mut output = String::new();
    let mut active = None;
    for line in original.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some(section) = trimmed
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        {
            active = Some(section);
            if let Some(values) = sections.get(section) {
                output.push_str(&format!("[{section}]\n"));
                for (key, value) in values {
                    output.push_str(&format!("{key} = {value}\n"));
                }
                continue;
            }
        }
        if let Some((key, _)) = line.split_once('=') {
            let key = key.trim();
            if let Some(value) = plain.remove(key) {
                let equals = line
                    .find('=')
                    .ok_or_else(|| invalid("configuration assignment missing"))?;
                let ending = if line.ends_with("\r\n") {
                    "\r\n"
                } else if line.ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                output.push_str(&format!("{}= {value}{ending}", &line[..equals]));
                continue;
            }
            if active
                .and_then(|section| sections.get(section))
                .is_some_and(|values| values.contains_key(key))
            {
                continue;
            }
        }
        output.push_str(line);
    }
    require(
        plain.is_empty(),
        "required application configuration key missing",
    )?;
    for (section, values) in sections {
        let header = format!("[{section}]");
        if original.lines().any(|line| line.trim() == header) {
            continue;
        }
        output.push_str(&format!("\n[{section}]\n"));
        for (key, value) in values {
            output.push_str(&format!("{key} = {value}\n"));
        }
    }
    require(
        output.len() <= MAX_TEXT,
        "configured application byte bound",
    )?;
    Ok(output.into_bytes())
}

pub(super) fn download_packages(manifest_path: &Path, root: &Path) -> io::Result<()> {
    super::replay::validate_root(root)?;
    let manifest = load(manifest_path)?;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .max_redirects(3)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    std::fs::create_dir(root)?;
    let result = (|| {
        let mut files = BTreeMap::new();
        for program in &manifest.programs {
            let mut response = agent.get(&program.url).call().map_err(io::Error::other)?;
            let bytes = response
                .body_mut()
                .with_config()
                .limit(MAX_ARCHIVE as u64)
                .read_to_vec()
                .map_err(io::Error::other)?;
            let mut hash = Sha256::new();
            hash.update(&bytes);
            require(
                hex_digest(hash) == program.sha256,
                "downloaded package hash mismatch",
            )?;
            let path = format!("{}.zip", program.id);
            std::fs::write(root.join(&path), bytes)?;
            files.insert(program.id.clone(), path);
        }
        let catalog = root.join("programs.json");
        std::fs::write(
            &catalog,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version":1,"raw_root":root,"files":files
            }))
            .map_err(io::Error::other)?,
        )?;
        verify_packages(manifest_path, &catalog)?;
        println!("Acquired actual pinned packages: {}", catalog.display());
        Ok(())
    })();
    if result.is_err() {
        std::fs::remove_dir_all(root)?;
    }
    result
}

pub(super) fn prepare_packages(manifest_path: &Path, catalog_path: &Path) -> io::Result<PathBuf> {
    verify_packages(manifest_path, catalog_path)?;
    let manifest = load(manifest_path)?;
    let packages: Packages = serde_json::from_slice(&read_bounded(catalog_path, 64 * 1024)?)
        .map_err(io::Error::other)?;
    let raw_root = packages.raw_root.canonicalize()?;
    require(
        !raw_root.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?),
        "program guests must remain outside repository",
    )?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let root = raw_root.join(format!("prepared-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&root)?;
    let result = (|| {
        let mut guests = Vec::new();
        for program in &manifest.programs {
            let relative_package = packages
                .files
                .get(&program.id)
                .ok_or_else(|| invalid("acquired package missing"))?;
            let package = Archive::pinned(
                read_bounded(&confined(&raw_root, relative_package)?, MAX_ARCHIVE)?,
                &program.sha256,
            )?;
            let destination = root.join(&program.id);
            std::fs::create_dir(&destination)?;
            for entry in &package.entries {
                let path = destination.join(&entry.path);
                if entry.dir {
                    std::fs::create_dir_all(&path)?;
                } else {
                    std::fs::create_dir_all(
                        path.parent()
                            .ok_or_else(|| invalid("package parent missing"))?,
                    )?;
                    std::fs::write(&path, package.member(&entry.path, 64 * 1024 * 1024)?)?;
                }
            }
            let guest = destination.join(&program.application_root);
            let config_path = confined(&guest, Path::new(&program.configuration.path))?;
            let config = configured(
                &read_bounded(&config_path, MAX_TEXT)?,
                &program.configuration.settings,
            )?;
            let mut digest = Sha256::new();
            digest.update(&config);
            require(
                hex_digest(digest) == program.configuration.sha256,
                "prepared configuration differs from observed pin",
            )?;
            std::fs::write(&config_path, config)?;
            let working_directory = if program.working_directory == "." {
                program.application_root.clone()
            } else {
                format!("{}/{}", program.application_root, program.working_directory)
            };
            guests.push(serde_json::json!({
                "id": program.id,
                "guest": destination,
                "working_directory": working_directory,
                "command": program.command
            }));
        }
        let result =
            serde_json::to_vec_pretty(&serde_json::json!({"schema_version":1,"guests":guests}))
                .map_err(io::Error::other)?;
        std::fs::write(root.join("guests.json"), result)?;
        println!(
            "Prepared {} actual isolated program guests: {}",
            manifest.programs.len(),
            root.display()
        );
        Ok(root.clone())
    })();
    if result.is_err() {
        std::fs::remove_dir_all(&root)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approved() -> serde_json::Value {
        serde_json::from_slice(&std::fs::read("tests/fixtures/approved/mcp/manifest.json").unwrap())
            .unwrap()
    }

    #[test]
    fn actual_approved_metadata_is_valid() {
        parse(&serde_json::to_vec(&approved()).unwrap()).unwrap();
    }

    #[test]
    fn metadata_mutations_cannot_expand_publication_or_program_scope() {
        for field in [
            "repository",
            "program_files",
            "observations",
            "release_assets",
        ] {
            let mut value = approved();
            value["asset_policy"][field] = "changed".into();
            assert!(
                parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{field}"
            );
        }
        for (field, replacement) in [
            ("url", "http://example.invalid/package.zip"),
            ("sha256", "bad"),
            ("application_root", "../outside"),
            ("working_directory", "C:\\outside"),
            ("program_license", "unknown"),
            ("command", "GRAFX2.EXE; echo injected"),
        ] {
            let mut value = approved();
            value["programs"][0][field] = replacement.into();
            assert!(
                parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{field}"
            );
        }
        let mut value = approved();
        value["programs"][1] = value["programs"][0].clone();
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut value = approved();
        value["extra"] = true.into();
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
