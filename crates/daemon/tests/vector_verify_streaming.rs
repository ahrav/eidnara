//! A generation with several faults refuses with the earliest in refusal order: a row before a code, scales before codes, and identifiers before codes.

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use daemon::vector_generation::{
    CODES_FILE, FileFault, ROW_IDS_FILE, ROWS_FILE, SCALES_FILE, SIDECAR_FILE, VectorRefusal,
    VectorSidecar, verify,
};
use host_runtime::generation::GenerationManifest;
use retrieval::dense::codec::{self, ArtifactRejection, RowRejection};
use retrieval::dense::scalar::{Scales, encode, encode_codes};
use sha2::{Digest, Sha256};
use support::vector_store::{DIMENSION, Fixture, export};

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_sidecar(dir: &Path) -> VectorSidecar {
    serde_json::from_slice(&fs::read(dir.join(SIDECAR_FILE)).unwrap()).unwrap()
}

fn write_bound(dir: &Path, sidecar: &VectorSidecar) {
    fs::write(dir.join(SIDECAR_FILE), sidecar.canonical_bytes()).unwrap();
    fs::write(
        dir.join("manifest.json"),
        sidecar.stage_manifest().canonical_bytes(),
    )
    .unwrap();
}

fn rehash_file(dir: &Path, path: &str, bytes: &[u8]) {
    fs::write(dir.join(path), bytes).unwrap();
    let mut sidecar = read_sidecar(dir);
    for file in &mut sidecar.files {
        if file.path == path {
            file.size = bytes.len() as u64;
            file.sha256 = sha(bytes);
        }
    }
    write_bound(dir, &sidecar);
}

fn restaged(fixture: &Fixture, digest: &str, tamper: impl FnOnce(&Path)) -> String {
    let scratch = fixture.work_dir();
    for entry in fs::read_dir(fixture.generation_dir(digest)).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), scratch.join(entry.file_name())).unwrap();
    }
    tamper(&scratch);
    let manifest: GenerationManifest =
        serde_json::from_slice(&fs::read(scratch.join("manifest.json")).unwrap()).unwrap();
    let new_digest = manifest.digest();
    let target = fixture.generation_dir(&new_digest);
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(&scratch).unwrap() {
        let entry = entry.unwrap();
        let dest = target.join(entry.file_name());
        fs::copy(entry.path(), &dest).unwrap();
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).unwrap();
    }
    new_digest
}

fn bump_code(dir: &Path, index: usize) {
    let mut codes = fs::read(dir.join(CODES_FILE)).unwrap();
    codes[index] = codes[index].wrapping_add(1);
    rehash_file(dir, CODES_FILE, &codes);
}

fn file_fault(path: &'static str, fault: FileFault) -> VectorRefusal {
    VectorRefusal::File { path, fault }
}

#[test]
fn a_bad_code_never_hides_an_earlier_fault() {
    let fixture = Fixture::new();
    let layer = fixture.layer(1, 10);
    let layout = layer.sidecar.layout().unwrap();
    let digest = layer.digest;
    let verify_of = |digest: &str| verify(&fixture.store, digest, &fixture.expected(), u64::MAX);
    let codes_fault = file_fault(CODES_FILE, FileFault::Codes);

    let codes_only = restaged(&fixture, &digest, |dir| bump_code(dir, 3));
    assert_eq!(verify_of(&codes_only).unwrap_err(), codes_fault);

    let row = restaged(&fixture, &digest, |dir| {
        bump_code(dir, 0);
        let mut rows = fs::read(dir.join(ROWS_FILE)).unwrap();
        let offset = codec::row_offset(2, DIMENSION) as usize;
        rows[offset..offset + 4].copy_from_slice(&2.0f32.to_le_bytes());
        rehash_file(dir, ROWS_FILE, &rows);
    });
    assert!(
        matches!(
            verify_of(&row).unwrap_err(),
            VectorRefusal::Rows(ArtifactRejection::Row {
                index: 2,
                rejection: RowRejection::Normalization { .. }
            })
        ),
        "a row outside the layout refuses ahead of a code mismatch in an earlier row"
    );

    let arbitrary = Scales::from_values(vec![0.5; DIMENSION as usize], DIMENSION).unwrap();
    for recode in [true, false] {
        let scales = restaged(&fixture, &digest, |dir| {
            if recode {
                let codes: Vec<u8> = export(1, 10)
                    .rows
                    .iter()
                    .flat_map(|row| {
                        encode_codes(&encode(&layout, &arbitrary, &row.vector).unwrap().codes)
                    })
                    .collect();
                rehash_file(dir, CODES_FILE, &codes);
            }
            rehash_file(dir, SCALES_FILE, &arbitrary.encode());
            let mut sidecar = read_sidecar(dir);
            sidecar.scales_sha256 = sha(&arbitrary.encode());
            write_bound(dir, &sidecar);
        });
        assert_eq!(
            verify_of(&scales).unwrap_err(),
            file_fault(SCALES_FILE, FileFault::Calibration),
            "codes re-encoded under the stored scales: {recode}"
        );
    }

    let short_scales = restaged(&fixture, &digest, |dir| {
        bump_code(dir, 5);
        let mut scales = fs::read(dir.join(SCALES_FILE)).unwrap();
        scales.truncate(scales.len() - 4);
        rehash_file(dir, SCALES_FILE, &scales);
        let mut sidecar = read_sidecar(dir);
        sidecar.scales_sha256 = sha(&scales);
        write_bound(dir, &sidecar);
    });
    assert_eq!(
        verify_of(&short_scales).unwrap_err(),
        file_fault(SCALES_FILE, FileFault::Calibration),
        "a scales file that does not decode refuses as calibration beside a code mismatch"
    );

    for resize in [false, true] {
        let ids = restaged(&fixture, &digest, |dir| {
            if resize {
                let mut codes = fs::read(dir.join(CODES_FILE)).unwrap();
                codes.truncate(codes.len() - DIMENSION as usize);
                rehash_file(dir, CODES_FILE, &codes);
            } else {
                bump_code(dir, 3);
            }
            rehash_file(dir, ROW_IDS_FILE, br#"["b","a","c","d"]"#);
        });
        assert_eq!(
            verify_of(&ids).unwrap_err(),
            file_fault(ROW_IDS_FILE, FileFault::Identifiers),
            "identifiers refuse ahead of codes; codes resized: {resize}"
        );
    }
}
