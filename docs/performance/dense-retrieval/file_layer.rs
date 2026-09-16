//! This module ranks frozen fixtures through the shared layered API with positioned file reads.
//! It models original-row reads, not daemon acquisition, VerifiedVectors, pins, or ledger admission.

use std::cell::Cell;
use std::fs::File;
use std::io::Write;
use std::num::NonZeroUsize;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

use kernel::MAX_ELIGIBILITY_CANDIDATES;
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use retrieval::batch::{ProjectionCheckpoint, VectorGeneration, read_checkpoint};
use retrieval::dense::codec::{self, ARTIFACT_HEADER_BYTES, RowLayout};
use retrieval::dense::{
    Completion, ExhaustiveQuery, Layer, LayeredQuery, Precedence, RowAccess, RowFault, rank_layers,
    resolve,
};
use retrieval::eligibility::{Disposition, judge_tracked};
use serde_json::{Value, json};

use super::candidates::{self, PageSource, ProbeResult};
use super::support::dense::Fixture;

pub(super) struct FileLayer {
    file: File,
    ids: Vec<String>,
    checkpoint: ProjectionCheckpoint,
    generation: VectorGeneration,
    layout: RowLayout,
    root: PathBuf,
    version: (i64, u64),
    live_rows: usize,
    reads: Cell<(usize, u64)>,
    pub file_bytes: u64,
    pub resident_id_bytes: usize,
    pub build_measurements: Value,
}

impl FileLayer {
    fn layer(&self) -> Layer<'_> {
        Layer {
            precedence: Precedence {
                base_epoch: self.generation.generation_epoch,
                delta_ordinal: 0,
            },
            checkpoint: &self.checkpoint,
            occurrence_ids: &self.ids,
            rows: self,
            tombstones: &[],
        }
    }

    fn check_frozen(&self, fixture: &Fixture, query: &ExhaustiveQuery<'_>) {
        assert_eq!(fixture.root.path(), self.root);
        assert_eq!(query.authority, fixture.authority());
        assert_eq!(*query.generation, self.generation);
        assert_eq!(query.metric, self.layout.metric);
        assert_eq!(query.unit_norm_tolerance, self.layout.unit_norm_tolerance);
        assert_eq!(fixture.rows.len(), self.ids.len());
        assert_eq!(
            fixture.kernel.tip().unwrap(),
            self.checkpoint.checkpoint_commit_seq,
            "file layer requires a frozen kernel"
        );
        assert_eq!(
            fixture
                .store
                .with_conn(|conn| Ok(candidates::projection_version(conn)))
                .unwrap(),
            self.version,
            "file layer requires a frozen projection"
        );
    }

    fn measured(&self, before: (usize, u64), mut result: ProbeResult) -> ProbeResult {
        let (calls, bytes) = self.reads.get();
        let calls = calls - before.0;
        let bytes = bytes - before.1;
        assert_eq!(
            calls, self.live_rows,
            "each live row needs one positioned read"
        );
        assert_eq!(bytes, calls as u64 * u64::from(self.layout.dimension) * 4);
        assert_eq!(result.measurements["scanned"], json!(self.live_rows));
        result.measurements["row_read_calls"] = json!(calls);
        result.measurements["row_read_bytes"] = json!(bytes);
        result.measurements["row_shape_decodes"] = json!(calls);
        result.measurements["row_layout_validations"] = json!(calls);
        result.measurements["row_file_opens_during_query"] = json!(0);
        result
    }
}

impl RowAccess for FileLayer {
    fn row_count(&self) -> usize {
        self.ids.len()
    }

    fn row(&self, index: usize) -> Result<Vec<f32>, RowFault> {
        if index >= self.ids.len() {
            return Err(RowFault::Unavailable(format!(
                "row {index} is past the {} rows the layer declares",
                self.ids.len()
            )));
        }
        let width = self.layout.dimension as usize * 4;
        let offset = ARTIFACT_HEADER_BYTES as u64 + index as u64 * width as u64;
        let mut bytes = vec![0u8; width];
        let (calls, read_bytes) = self.reads.get();
        self.reads.set((calls + 1, read_bytes));
        self.file
            .read_exact_at(&mut bytes, offset)
            .map_err(|error| RowFault::Unavailable(format!("row {index}: {error}")))?;
        self.reads.set((calls + 1, read_bytes + width as u64));
        codec::decode_shape(&bytes, self.layout.dimension).map_err(RowFault::Rejected)
    }
}

pub(super) fn build(fixture: &Fixture, query: &ExhaustiveQuery<'_>) -> FileLayer {
    assert!(!fixture.rows.is_empty());
    assert_eq!(query.authority, fixture.authority());
    assert!(
        fixture
            .rows
            .iter()
            .all(|row| row.class == OccurrenceClass::CanonicalClaims)
    );
    let layout = RowLayout {
        dimension: query.generation.vector_dimension,
        metric: query.metric,
        unit_norm_tolerance: query.unit_norm_tolerance,
    };
    codec::validate(query.query, &layout).expect("invalid query");
    let mut rows: Vec<_> = fixture
        .rows
        .iter()
        .map(|row| (row.occurrence_id(), row))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(rows.windows(2).all(|pair| pair[0].0 < pair[1].0));
    let (checkpoint, version, live_rows, metadata) = fixture
        .store
        .with_conn(|conn| {
            let checkpoint = read_checkpoint(conn, &fixture.incarnation)
                .unwrap()
                .expect("missing projection checkpoint");
            let mut stmt = conn.prepare_cached(
                "SELECT occurrence_id,class,source_object_id,revision,source_artifact_digest
             FROM occurrences ORDER BY occurrence_id",
            )?;
            let metadata = stmt
                .query_map([], candidates::candidate)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let live_rows: i64 = conn.query_row(
                "SELECT count(*) FROM occurrences o
             LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
             WHERE t.occurrence_id IS NULL",
                [],
                |row| row.get(0),
            )?;
            Ok((
                checkpoint,
                candidates::projection_version(conn),
                usize::try_from(live_rows).unwrap(),
                metadata,
            ))
        })
        .unwrap();
    assert_eq!(metadata.len(), rows.len());
    for (candidate, (id, _)) in metadata.iter().zip(&rows) {
        assert_eq!(&candidate.occurrence_id, id);
        assert_eq!(candidate.class, OccurrenceClass::CanonicalClaims);
    }
    let mut snapshot = None;
    let mut incarnation = None;
    for batch in metadata.chunks(MAX_ELIGIBILITY_CANDIDATES) {
        let (report, moved) = judge_tracked(
            &fixture.kernel,
            query.authority,
            batch,
            &EvalBudget::unbounded(),
            &mut snapshot,
            &mut incarnation,
        )
        .unwrap();
        assert_eq!(moved, None);
        assert_eq!(report.occurrences.len(), batch.len());
        assert!(
            report
                .occurrences
                .iter()
                .all(|row| row.disposition == Disposition::Eligible),
            "file layer requires an all-admitted fixture"
        );
    }
    assert_eq!(snapshot.unwrap().tip, checkpoint.checkpoint_commit_seq);
    let bytes = codec::encode_rows(
        &layout,
        rows.iter()
            .map(|(_, row)| row.vector.as_deref().expect("missing fixture vector")),
    )
    .expect("invalid fixture vectors");
    let file_bytes = bytes.len() as u64;
    assert_eq!(
        file_bytes,
        ARTIFACT_HEADER_BYTES as u64 + rows.len() as u64 * u64::from(layout.dimension) * 4
    );
    let mut file = tempfile::tempfile_in(fixture.root.path()).unwrap();
    file.write_all(&bytes).unwrap();
    assert_eq!(file.metadata().unwrap().len(), file_bytes);
    let ids: Vec<_> = rows.into_iter().map(|(id, _)| id).collect();
    let resident_id_bytes =
        ids.capacity() * size_of::<String>() + ids.iter().map(String::capacity).sum::<usize>();
    let layer = FileLayer {
        file,
        ids,
        checkpoint,
        generation: query.generation.clone(),
        layout,
        root: fixture.root.path().to_owned(),
        version,
        live_rows,
        reads: Cell::new((0, 0)),
        file_bytes,
        resident_id_bytes,
        build_measurements: json!({
            "rows": fixture.rows.len(), "live_rows": live_rows, "layers": 1, "deltas": 0,
            "layer_tombstones": 0, "projection_tombstones": fixture.rows.len() - live_rows,
            "file_bytes": file_bytes, "header_bytes": ARTIFACT_HEADER_BYTES,
            "resident_id_bytes": resident_id_bytes, "resident_vector_bytes": 0,
            "row_file_opens": 1, "setup_judged": metadata.len(),
            "setup_batches": metadata.len().div_ceil(MAX_ELIGIBILITY_CANDIDATES),
            "resident_accounting": "ID vector and string capacities only; excludes allocator overhead, fixture rows, transient buffers, and OS cache",
            "cache_policy": "fixture-written file; warm OS cache; no cache drops",
            "scope": "shared rank_layers and original-row accessor; no daemon acquire, VerifiedVectors, pins, ledger, or host wire"
        }),
    };
    let layers = [layer.layer()];
    let resolved = resolve(&layers, NonZeroUsize::new(layer.ids.len()).unwrap()).unwrap();
    assert_eq!(resolved.winners.len(), layer.ids.len());
    assert_eq!((resolved.superseded, resolved.masked), (0, 0));
    assert!(
        resolved
            .winners
            .iter()
            .enumerate()
            .all(|(index, winner)| winner.layer == 0
                && winner.row == index
                && winner.occurrence_id == layer.ids[index])
    );
    layer.check_frozen(fixture, query);
    layer
}

pub(super) fn baseline(
    layer: &FileLayer,
    fixture: &Fixture,
    query: &ExhaustiveQuery<'_>,
) -> ProbeResult {
    layer.check_frozen(fixture, query);
    let before = layer.reads.get();
    let layers = [layer.layer()];
    let request = LayeredQuery {
        generation: query.generation,
        metric: query.metric,
        unit_norm_tolerance: query.unit_norm_tolerance,
        query: query.query,
        authority: query.authority,
        bounds: query.bounds,
        layers: &layers,
        max_entries: NonZeroUsize::new(layer.ids.len()).unwrap(),
    };
    let ranked = fixture
        .store
        .with_conn(|conn| {
            Ok(rank_layers(conn, &fixture.kernel, &request, &EvalBudget::unbounded()).unwrap())
        })
        .unwrap();
    layer.check_frozen(fixture, query);
    let ranking = ranked.ranking;
    assert_eq!(ranking.completion, Completion::Complete);
    assert_eq!(ranking.coverage.required, layer.live_rows);
    assert_eq!(ranking.coverage.with_vector, layer.live_rows);
    assert_eq!(ranking.coverage.missing(), 0);
    assert!(ranking.consumed.excluded.is_empty());
    assert_eq!(ranked.layers.winners, layer.ids.len());
    assert_eq!(ranked.layers.revoked, layer.ids.len() - layer.live_rows);
    assert_eq!(
        (
            ranked.layers.superseded,
            ranked.layers.masked,
            ranked.layers.unvisited
        ),
        (0, 0, 0)
    );
    layer.measured(
        before,
        ProbeResult {
            measurements: json!({
                "passes": 1, "scanned": ranking.coverage.required,
                "decoded": ranking.coverage.with_vector, "scored": ranking.coverage.with_vector,
                "pages": ranking.consumed.pages, "judged": ranking.consumed.judged,
                "batches": ranking.consumed.batches, "empty_snapshot_batches": 0,
                "winners": ranked.layers.winners, "revoked": ranked.layers.revoked,
                "superseded": ranked.layers.superseded, "masked": ranked.layers.masked,
                "unvisited": ranked.layers.unvisited,
            }),
            ranked: ranking.ranked,
        },
    )
}

pub(super) fn optimized(
    layer: &FileLayer,
    fixture: &Fixture,
    query: &ExhaustiveQuery<'_>,
) -> ProbeResult {
    layer.check_frozen(fixture, query);
    let before = layer.reads.get();
    let layers = [layer.layer()];
    let resolved = resolve(&layers, NonZeroUsize::new(layer.ids.len()).unwrap()).unwrap();
    assert_eq!(resolved.winners.len(), layer.ids.len());
    assert_eq!((resolved.superseded, resolved.masked), (0, 0));
    let result = candidates::page_score_first_from(
        &fixture.store,
        &fixture.kernel,
        query,
        PageSource::Resolved {
            winners: resolved.winners.iter(),
            rows: layer,
        },
    );
    layer.check_frozen(fixture, query);
    layer.measured(before, result)
}

#[cfg(test)]
mod tests {
    use super::super::support::dense::{OBJECTS, axis, bounds, corpus, generation, reference_over};
    use super::*;

    #[test]
    fn static_base_keeps_file_ordinals_and_skips_projection_tombstones() {
        let fixture = Fixture::new(
            &OBJECTS,
            corpus()
                .into_iter()
                .filter(|row| row.class == OccurrenceClass::CanonicalClaims)
                .collect(),
        );
        let mut ids: Vec<_> = fixture.rows.iter().map(|row| row.occurrence_id()).collect();
        ids.sort();
        let tombstones = [&ids[0], ids.last().unwrap()];
        for id in tombstones {
            assert_eq!(fixture.raw().execute(
                "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at)
                 VALUES (?1,?2,'retired',1)",
                rusqlite::params![id, fixture.kernel.tip().unwrap()],
            ).unwrap(), 1);
        }
        let query = axis(0);
        let generation = generation();
        let request = fixture.query(&query, &generation, bounds(2));
        let mut expected = reference_over(
            &query,
            fixture
                .rows
                .iter()
                .filter(|row| !tombstones.contains(&&row.occurrence_id()))
                .map(|row| (row.occurrence_id(), row.vector.as_deref().unwrap())),
        );
        expected.truncate(2);
        let stored = candidates::page_score_first(&fixture.store, &fixture.kernel, &request);
        assert_eq!(
            fixture
                .raw()
                .execute("DELETE FROM occurrence_vectors", [])
                .unwrap(),
            ids.len()
        );
        let layer = build(&fixture, &request);
        assert_eq!(layer.row_count(), ids.len());
        assert_eq!(
            layer.file_bytes,
            ARTIFACT_HEADER_BYTES as u64 + ids.len() as u64 * 8 * 4
        );
        assert_eq!(layer.build_measurements["resident_vector_bytes"], 0);
        assert!(layer.resident_id_bytes >= ids.iter().map(String::len).sum::<usize>());
        for (index, id) in ids.iter().enumerate() {
            let row = fixture
                .rows
                .iter()
                .find(|row| &row.occurrence_id() == id)
                .unwrap();
            assert_eq!(layer.row(index).unwrap(), *row.vector.as_ref().unwrap());
        }
        let file_results = [
            baseline(&layer, &fixture, &request),
            optimized(&layer, &fixture, &request),
        ];
        for result in &file_results {
            assert_eq!(
                result.measurements["row_read_calls"],
                json!(ids.len() - tombstones.len())
            );
            assert_eq!(
                result.measurements["row_read_bytes"],
                json!((ids.len() - tombstones.len()) * 8 * 4)
            );
            assert_eq!(result.measurements["row_file_opens_during_query"], 0);
        }
        for result in std::iter::once(stored).chain(file_results) {
            assert_eq!(result.ranked.len(), expected.len());
            for (actual, (id, score)) in result.ranked.iter().zip(&expected) {
                assert_eq!(&actual.occurrence_id, id);
                assert_eq!(actual.class, OccurrenceClass::CanonicalClaims);
                assert_eq!(actual.score.to_bits(), score.to_bits());
            }
        }
        assert!(matches!(
            layer.row(ids.len()),
            Err(RowFault::Unavailable(_))
        ));
        layer.file.set_len(layer.file_bytes - 1).unwrap();
        assert!(matches!(
            layer.row(ids.len() - 1),
            Err(RowFault::Unavailable(_))
        ));
    }
}
