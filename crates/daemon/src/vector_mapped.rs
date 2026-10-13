//! Read-only mappings of a pinned layer's int8 code file for one candidate scan.
//!
//! The scan maps the rows the file holds when the scan starts and reads every row it scores directly from that mapping. A file shorter than its layer declares maps only its whole rows, and a row past them reads as missing, the classification a short positioned read gives.

use std::fs::File;

use memmap2::{Advice, Mmap, MmapOptions};
use retrieval::dense::{CodeAccess, RowFault};

/// One layer's codes for one candidate scan: `declared` rows of `width` bytes each from offset 0 of the layer's code file, mapped read-only for as long as the scan holds this value.
pub(crate) struct MappedCodes {
    mapping: Mapping,
    width: usize,
    declared: usize,
}

enum Mapping {
    /// The whole rows the file held when it was mapped, at most `declared`; `bytes` is absent when that is no row.
    Rows { bytes: Option<Mmap>, rows: usize },
    /// The file could not be measured or mapped; every row read reports the failure as a failed read.
    Failed(String),
}

impl MappedCodes {
    /// Maps the first `declared` rows of `codes`, or the whole rows it holds when it is shorter.
    ///
    /// `codes` must be a verified generation member that the caller keeps pinned, so its bytes stay as verified for as long as the mapping lives.
    pub(crate) fn map(codes: &File, width: usize, declared: usize) -> Self {
        let length = match codes.metadata() {
            Ok(metadata) => metadata.len(),
            Err(error) => return Self::failed(width, declared, &error),
        };
        let held_rows = if width == 0 {
            declared
        } else {
            usize::try_from(length / width as u64)
                .unwrap_or(usize::MAX)
                .min(declared)
        };
        let mapped = held_rows * width;
        if mapped == 0 {
            return Self {
                mapping: Mapping::Rows {
                    bytes: None,
                    rows: held_rows,
                },
                width,
                declared,
            };
        }
        // SAFETY: `codes` is a published generation member: the store writes it once while staging, renames it into place, verifies its hash on acquisition, and removes it only under the lifecycle's exclusive lock, which a pinned member excludes. The caller holds that pin for the life of this value, so the mapped bytes are never written while they are borrowed. The mapping is read-only and covers only bytes the file held when it was measured, and nothing derives a mutable reference from it.
        #[allow(unsafe_code)]
        let mapped = unsafe { MmapOptions::new().len(mapped).map(codes) };
        match mapped {
            Ok(bytes) => {
                // The scan reads rows in increasing order; the hint only shapes readahead, and a refused hint leaves the mapping as correct as before.
                let _hint = bytes.advise(Advice::Sequential);
                Self {
                    mapping: Mapping::Rows {
                        bytes: Some(bytes),
                        rows: held_rows,
                    },
                    width,
                    declared,
                }
            }
            Err(error) => Self::failed(width, declared, &error),
        }
    }

    fn failed(width: usize, declared: usize, error: &std::io::Error) -> Self {
        Self {
            mapping: Mapping::Failed(error.to_string()),
            width,
            declared,
        }
    }

    /// Bytes this scan maps; the scan's scratch reservation charges them, since mapped pages become resident as the scan reads them.
    pub(crate) fn mapped_bytes(width: usize, declared: usize) -> u64 {
        (width as u64).saturating_mul(declared as u64)
    }
}

impl CodeAccess for MappedCodes {
    fn row_count(&self) -> usize {
        self.declared
    }

    /// The candidate scan checks the dimension and the reserved code.
    fn codes_into(&self, index: usize, into: &mut Vec<i8>) -> Result<(), RowFault> {
        if index >= self.declared {
            return Err(RowFault::Missing(format!(
                "row {index} is past the {} rows the layer declares",
                self.declared
            )));
        }
        let (bytes, rows) = match &self.mapping {
            Mapping::Failed(detail) => {
                return Err(RowFault::Unavailable(format!("row {index}: {detail}")));
            }
            Mapping::Rows { bytes, rows } => (bytes, *rows),
        };
        if index >= rows {
            return Err(RowFault::Missing(format!(
                "row {index}: the code file ends after {rows} rows"
            )));
        }
        let start = index * self.width;
        let row = bytes
            .as_deref()
            .map_or(&[][..], |bytes| &bytes[start..start + self.width]);
        into.clear();
        into.extend(row.iter().map(|byte| *byte as i8));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::FileExt;

    use super::*;

    const WIDTH: usize = 4;
    const DECLARED: usize = 5;

    fn all_codes() -> Vec<u8> {
        (0..DECLARED * WIDTH).map(|byte| byte as u8).collect()
    }

    fn row_codes(codes: &[u8], row: usize) -> Vec<i8> {
        codes[row * WIDTH..(row + 1) * WIDTH]
            .iter()
            .map(|byte| *byte as i8)
            .collect()
    }

    #[test]
    fn a_whole_code_file_serves_every_declared_row_from_one_mapping() {
        let codes = all_codes();
        let file = tempfile::tempfile().unwrap();
        file.write_all_at(&codes, 0).unwrap();
        let mapped = MappedCodes::map(&file, WIDTH, DECLARED);
        assert_eq!(mapped.row_count(), DECLARED);
        let mut into = Vec::new();
        for row in 0..DECLARED {
            mapped.codes_into(row, &mut into).unwrap();
            assert_eq!(into, row_codes(&codes, row), "row {row}");
        }
        assert!(matches!(
            mapped.codes_into(DECLARED, &mut into),
            Err(RowFault::Missing(_))
        ));
    }

    /// A file shorter than its layer declares serves its whole rows and reads every later row as missing, as a short positioned read does; bytes past the declared rows are never mapped.
    #[test]
    fn a_short_code_file_maps_its_whole_rows_and_reads_the_rest_as_missing() {
        let codes = all_codes();
        let file = tempfile::tempfile().unwrap();
        file.write_all_at(&codes[..2 * WIDTH + 1], 0).unwrap();
        let mapped = MappedCodes::map(&file, WIDTH, DECLARED);
        let mut into = Vec::new();
        for row in 0..2 {
            mapped.codes_into(row, &mut into).unwrap();
            assert_eq!(into, row_codes(&codes, row), "row {row}");
        }
        for row in 2..DECLARED {
            assert!(
                matches!(mapped.codes_into(row, &mut into), Err(RowFault::Missing(_))),
                "row {row}"
            );
        }
        let longer = tempfile::tempfile().unwrap();
        longer.write_all_at(&codes, 0).unwrap();
        longer
            .write_all_at(&[0xff; WIDTH], codes.len() as u64)
            .unwrap();
        let mapped = MappedCodes::map(&longer, WIDTH, DECLARED);
        mapped.codes_into(DECLARED - 1, &mut into).unwrap();
        assert_eq!(into, row_codes(&codes, DECLARED - 1));
        assert!(matches!(
            mapped.codes_into(DECLARED, &mut into),
            Err(RowFault::Missing(_))
        ));
    }

    #[test]
    fn an_empty_code_file_reads_every_row_as_missing() {
        let file = tempfile::tempfile().unwrap();
        let mapped = MappedCodes::map(&file, WIDTH, DECLARED);
        let mut into = Vec::new();
        assert!(matches!(
            mapped.codes_into(0, &mut into),
            Err(RowFault::Missing(_))
        ));
    }

    /// A descriptor that cannot be mapped reports every row as a failed read, which leaves the view usable rather than quarantining it.
    #[test]
    fn a_descriptor_that_cannot_be_mapped_reads_every_row_as_failed() {
        let dir = tempfile::tempdir().unwrap();
        let file = File::open(dir.path()).unwrap();
        let declared = usize::try_from(file.metadata().unwrap().len()).unwrap();
        assert!(declared > 0, "a directory reports a nonzero size");
        let mapped = MappedCodes::map(&file, 1, declared);
        let mut into = Vec::new();
        let outcome = mapped.codes_into(0, &mut into);
        assert!(
            matches!(outcome, Err(RowFault::Unavailable(_))),
            "{outcome:?}"
        );
    }
}
