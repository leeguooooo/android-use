//! Assemble the verified UIAutomator library with android-use's source-built adapter.
use phone_control::Failure;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read, Write};

const ADAPTER: &[u8] = include_bytes!("../runner/adapter.dex");
pub fn assemble(original: &[u8]) -> Result<Vec<u8>, Failure> {
    if format!("{:x}", Sha256::digest(original)) != crate::backend::RUNNER_SHA256 {
        return Err(Failure::not_sent(
            "runner_checksum_mismatch",
            "Only the pinned UIAutomator library is supported",
        ));
    }
    if !ADAPTER.starts_with(b"dex\n") {
        return Err(Failure::not_sent(
            "adapter_invalid",
            "Source-built Android adapter is missing",
        ));
    }
    let invalid = || {
        Failure::not_sent(
            "runner_archive_failed",
            "Unable to prepare the Android control runner",
        )
    };
    let mut archive = zip::ZipArchive::new(Cursor::new(original)).map_err(|_| invalid())?;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|_| invalid())?;
        if entry.name() == "classes8.dex" {
            return Err(invalid());
        }
        let options = zip::write::FileOptions::default()
            .compression_method(entry.compression())
            .last_modified_time(entry.last_modified());
        if entry.is_dir() {
            writer
                .add_directory(entry.name(), options)
                .map_err(|_| invalid())?;
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).map_err(|_| invalid())?;
        writer
            .start_file(entry.name(), options)
            .map_err(|_| invalid())?;
        writer.write_all(&data).map_err(|_| invalid())?;
    }
    writer
        .start_file(
            "classes8.dex",
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .map_err(|_| invalid())?;
    writer.write_all(ADAPTER).map_err(|_| invalid())?;
    Ok(writer.finish().map_err(|_| invalid())?.into_inner())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unknown_upstream_library() {
        assert_eq!(
            assemble(b"wrong").unwrap_err().error,
            "runner_checksum_mismatch"
        );
    }
    #[test]
    fn embedded_adapter_matches_source_manifest() {
        assert!(ADAPTER.starts_with(b"dex\n"));
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../runner/manifest.json")).unwrap();
        assert_eq!(
            manifest["adapter_sha256"],
            format!("{:x}", Sha256::digest(ADAPTER))
        );
        assert_eq!(
            manifest["source_sha256"],
            format!(
                "{:x}",
                Sha256::digest(include_bytes!("../runner/src/com/androiduse/Runner.java"))
            )
        );
    }
}
