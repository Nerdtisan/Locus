// SPDX-License-Identifier: AGPL-3.0-or-later

//! A minimal stored (uncompressed) ZIP writer, so the web build can hand the
//! per-layer files over as one download (#291). Core crate: bytes in, bytes
//! out, wasm32-clean, no dependencies.

/// CRC-32 (IEEE), bitwise: the files are small and this runs once per export.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Why no archive was produced.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ZipError {
    #[error("a file is too large for a plain zip archive")]
    TooLarge,
    #[error("too many files for a plain zip archive")]
    TooMany,
}

/// A ZIP archive holding `files` (name, bytes), stored without compression.
/// Names must already be safe, flat file names (see
/// [`file_names`](crate::file_names)).
///
/// # Errors
/// [`ZipError`] past the 4 GiB / 65 535-entry limits of the plain format.
pub fn zip(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, ZipError> {
    let count = u16::try_from(files.len()).map_err(|_| ZipError::TooMany)?;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = u32::try_from(out.len()).map_err(|_| ZipError::TooLarge)?;
        let size = u32::try_from(data.len()).map_err(|_| ZipError::TooLarge)?;
        let name_len = u16::try_from(name.len()).map_err(|_| ZipError::TooLarge)?;
        let crc = crc32(data);
        // Fixed timestamp (1980-01-01): archives are reproducible (SPEC §10).
        let (time, date) = (0u16, 0x0021u16);
        // UTF-8 names (general purpose flag bit 11).
        let flags = 0x0800u16;
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(flags.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(time.to_le_bytes());
        out.extend(date.to_le_bytes());
        out.extend(crc.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(name_len.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name.as_bytes());
        out.extend(data);

        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(flags.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(time.to_le_bytes());
        central.extend(date.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend(size.to_le_bytes());
        central.extend(size.to_le_bytes());
        central.extend(name_len.to_le_bytes());
        central.extend([0u8; 12]);
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let cd_offset = u32::try_from(out.len()).map_err(|_| ZipError::TooLarge)?;
    let cd_size = u32::try_from(central.len()).map_err(|_| ZipError::TooLarge)?;
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(count.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend(cd_size.to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn archive_has_the_expected_structure() {
        let z = zip(&[("a.txt".to_owned(), b"hello".to_vec())]).unwrap();
        assert_eq!(&z[..4], b"PK\x03\x04");
        // Local header 30 + name 5 + data 5, central 46 + 5, end 22.
        assert_eq!(z.len(), 40 + 51 + 22);
        assert_eq!(&z[z.len() - 22..z.len() - 18], b"PK\x05\x06");
        assert_eq!(zip(&[]).unwrap().len(), 22);
    }
}
