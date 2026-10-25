use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::is_lower_hex;
use serde::{Deserialize, Serialize};

use crate::blank;
use crate::campaign::{SampleError, SampleLedger, SampleRecord};
use crate::pairs::ArmKind;

/// The [`WriteKeyScheme::TaskArm`](crate::WriteKeyScheme::TaskArm) key of
/// every memory write a sample makes. The key reads `task` and `arm` only, so
/// every retry of a sample shares its key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteKey {
    /// The writing sample's task; own-task exclusion compares this field.
    pub task: String,
    /// The writing sample's arm.
    pub arm: ArmKind,
}

impl From<&SampleRecord> for WriteKey {
    fn from(record: &SampleRecord) -> Self {
        Self {
            task: record.task.clone(),
            arm: record.arm,
        }
    }
}

/// True when a write under `key` is `target`'s own task's, under any arm;
/// the runner drops such writes from `target`'s pool at read time.
pub fn excludes(target: &SampleRecord, key: &WriteKey) -> bool {
    key.task == target.task
}

/// One memory write the runner admitted into a history pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryWrite {
    /// Unique across the record; reads name the write by it.
    pub write_id: String,
    /// The history pool the write entered.
    pub pool: String,
    /// The writer's `WriteKey`, the key the runner stored the write under.
    pub key: WriteKey,
    /// The ledger sample that made the write.
    pub writer: String,
    /// The run that made the write: the current run, or a run named in some
    /// sample's lineage.
    pub eval_run_id: String,
    /// The write's position in its pool, recorded for audit.
    pub sequence: u32,
}

/// One memory read: a pool served `write_id` to `reader`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRead {
    /// The ledger sample whose history pool served the read.
    pub reader: String,
    /// The pool that served the read; it equals the named write's pool.
    pub pool: String,
    /// The write the pool served.
    pub write_id: String,
    /// The read's position in its pool, recorded for audit.
    pub sequence: u32,
}

/// The runner's record of every memory write and read in a run's pools.
/// `read_counts` is the runner's completeness attestation: one entry per
/// ledger sample, the number of reads it recorded for that sample, with an
/// explicit `0` for a sample that read nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessRecord {
    /// Every write admitted into the run's pools.
    pub writes: Vec<MemoryWrite>,
    /// Every read the run's pools served.
    pub reads: Vec<MemoryRead>,
    /// The attested read count per ledger sample id.
    pub read_counts: BTreeMap<String, u32>,
}

/// The refusals of [`AccessRecord::check_disjoint`], one per defect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessError {
    /// The ledger fails `SampleLedger::validate`; nothing in the record is
    /// read.
    Ledger(SampleError),
    /// The current run id is not a 64-character lowercase hex run id.
    MalformedRun { eval_run_id: String },
    /// A blank `write_id` or `pool` on a write, or a blank `pool` on a read
    /// of the write `write_id`; blank means empty or whitespace.
    Blank {
        write_id: String,
        field: &'static str,
    },
    /// Two writes share one `write_id`.
    RepeatedWrite { write_id: String },
    /// A write whose `writer` is no ledger sample.
    UnknownWriter { write_id: String, writer: String },
    /// A write whose key is not its writer's `WriteKey`, so own-task
    /// attribution by key would misname the writer's task.
    KeyDisagreesWithWriter {
        write_id: String,
        key: WriteKey,
        writer_key: WriteKey,
    },
    /// A write made under a run that is neither the current run nor in any
    /// sample's lineage.
    UnknownRun {
        write_id: String,
        eval_run_id: String,
    },
    /// A read whose `reader` is no ledger sample.
    UnknownReader { reader: String },
    /// A `read_counts` entry for a sample the ledger lacks.
    UnknownAttestedSample { sample: String },
    /// A read naming a write the record lacks.
    UnknownWrite { reader: String, write_id: String },
    /// A read from a pool other than the one its write entered.
    CrossPoolRead {
        reader: String,
        write_id: String,
        read_pool: String,
        write_pool: String,
    },
    /// The attested read count for `sample` differs from the reads the
    /// record holds for it; `attested: None` is a sample with no entry.
    ReadCountMismatch {
        sample: String,
        attested: Option<u32>,
        recorded: usize,
    },
    /// `sample` read one of its own task's writes: a write in its pool under
    /// its task, made by the current run or a run in its lineage.
    OwnTaskRead {
        sample: String,
        read: Box<MemoryRead>,
        key: WriteKey,
    },
}

debug_display!(AccessError);

impl AccessRecord {
    /// Proves that no recorded read served a sample one of its own task's
    /// writes. The proof covers the supplied record only; recording
    /// completeness is the runner's attestation in `read_counts`. The check
    /// is order-free: sequences are carried for audit and never compared.
    pub fn check_disjoint(
        &self,
        ledger: &SampleLedger,
        eval_run_id: &str,
    ) -> Result<(), AccessError> {
        ledger.validate().map_err(AccessError::Ledger)?;
        if !is_lower_hex(eval_run_id, 64) {
            return Err(AccessError::MalformedRun {
                eval_run_id: eval_run_id.to_string(),
            });
        }
        let blank_field = |write_id: &str, field| AccessError::Blank {
            write_id: write_id.to_string(),
            field,
        };
        let known_runs: BTreeSet<&str> = std::iter::once(eval_run_id)
            .chain(
                ledger
                    .samples
                    .values()
                    .flat_map(|sample| sample.lineage.iter().map(String::as_str)),
            )
            .collect();
        let mut writes: BTreeMap<&str, &MemoryWrite> = BTreeMap::new();
        for write in &self.writes {
            for (field, text) in [("write_id", &write.write_id), ("pool", &write.pool)] {
                if blank(text) {
                    return Err(blank_field(&write.write_id, field));
                }
            }
            if writes.insert(&write.write_id, write).is_some() {
                return Err(AccessError::RepeatedWrite {
                    write_id: write.write_id.clone(),
                });
            }
            let Some(writer_record) = ledger.samples.get(&write.writer) else {
                return Err(AccessError::UnknownWriter {
                    write_id: write.write_id.clone(),
                    writer: write.writer.clone(),
                });
            };
            let writer_key = WriteKey::from(writer_record);
            if write.key != writer_key {
                return Err(AccessError::KeyDisagreesWithWriter {
                    write_id: write.write_id.clone(),
                    key: write.key.clone(),
                    writer_key,
                });
            }
            if !known_runs.contains(write.eval_run_id.as_str()) {
                return Err(AccessError::UnknownRun {
                    write_id: write.write_id.clone(),
                    eval_run_id: write.eval_run_id.clone(),
                });
            }
        }
        let mut recorded_counts: BTreeMap<&str, usize> = BTreeMap::new();
        for read in &self.reads {
            if blank(&read.pool) {
                return Err(blank_field(&read.write_id, "pool"));
            }
            let Some(sample) = ledger.samples.get(&read.reader) else {
                return Err(AccessError::UnknownReader {
                    reader: read.reader.clone(),
                });
            };
            let Some(write) = writes.get(read.write_id.as_str()) else {
                return Err(AccessError::UnknownWrite {
                    reader: read.reader.clone(),
                    write_id: read.write_id.clone(),
                });
            };
            if read.pool != write.pool {
                return Err(AccessError::CrossPoolRead {
                    reader: read.reader.clone(),
                    write_id: read.write_id.clone(),
                    read_pool: read.pool.clone(),
                    write_pool: write.pool.clone(),
                });
            }
            let own_run =
                write.eval_run_id == eval_run_id || sample.lineage.contains(&write.eval_run_id);
            if own_run && excludes(sample, &write.key) {
                return Err(AccessError::OwnTaskRead {
                    sample: read.reader.clone(),
                    read: Box::new(read.clone()),
                    key: write.key.clone(),
                });
            }
            *recorded_counts.entry(&read.reader).or_default() += 1;
        }
        if let Some(sample_id) = self
            .read_counts
            .keys()
            .find(|sample_id| !ledger.samples.contains_key(*sample_id))
        {
            return Err(AccessError::UnknownAttestedSample {
                sample: sample_id.clone(),
            });
        }
        for sample_id in ledger.samples.keys() {
            let attested = self.read_counts.get(sample_id).copied();
            let recorded = recorded_counts
                .get(sample_id.as_str())
                .copied()
                .unwrap_or(0);
            // A missing entry and a count past `usize` both compare unequal to
            // `Some(recorded)`, so each is a mismatch.
            if attested.and_then(|count| usize::try_from(count).ok()) != Some(recorded) {
                return Err(AccessError::ReadCountMismatch {
                    sample: sample_id.clone(),
                    attested,
                    recorded,
                });
            }
        }
        Ok(())
    }
}
