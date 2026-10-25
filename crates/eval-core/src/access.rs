use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::campaign::{SampleError, SampleLedger, SampleRecord};
use crate::pairs::ArmKind;

/// The `task_arm` key of every memory write a sample makes. The key reads
/// `task` and `arm` only, so every retry of a sample shares its key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteKey {
    pub task: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryWrite {
    pub write_id: String,
    pub pool: String,
    pub key: WriteKey,
    /// The ledger sample that made the write.
    pub writer: String,
    /// The run that made the write: the current run, or a run named in some
    /// sample's lineage.
    pub eval_run_id: String,
    /// The write's position in its pool, recorded for audit.
    pub sequence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRead {
    /// The ledger sample whose history pool served the read.
    pub reader: String,
    pub pool: String,
    pub write_id: String,
    /// The read's position in its pool, recorded for audit.
    pub sequence: u32,
}

/// The runner's record of every memory write and read in a run's pools.
/// `read_counts` is the runner's completeness attestation: one entry per
/// ledger sample, the number of reads it recorded for that sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessRecord {
    pub writes: Vec<MemoryWrite>,
    pub reads: Vec<MemoryRead>,
    pub read_counts: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessError {
    /// The ledger fails `SampleLedger::validate`; nothing in the record is
    /// read.
    Ledger(SampleError),
    RepeatedWrite {
        write_id: String,
    },
    UnknownWriter {
        write_id: String,
        writer: String,
    },
    /// A write whose key is not its writer's `WriteKey`, so own-task
    /// attribution by key would misname the writer's task.
    KeyDisagreesWithWriter {
        write_id: String,
        key: WriteKey,
        writer: WriteKey,
    },
    /// A write made under a run that is neither the current run nor in any
    /// sample's lineage.
    UnknownRun {
        write_id: String,
        eval_run_id: String,
    },
    UnknownReader {
        reader: String,
    },
    UnknownWrite {
        reader: String,
        write_id: String,
    },
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
            if writes.insert(&write.write_id, write).is_some() {
                return Err(AccessError::RepeatedWrite {
                    write_id: write.write_id.clone(),
                });
            }
            let Some(writer) = ledger.samples.get(&write.writer) else {
                return Err(AccessError::UnknownWriter {
                    write_id: write.write_id.clone(),
                    writer: write.writer.clone(),
                });
            };
            let writer = WriteKey::from(writer);
            if write.key != writer {
                return Err(AccessError::KeyDisagreesWithWriter {
                    write_id: write.write_id.clone(),
                    key: write.key.clone(),
                    writer,
                });
            }
            if !known_runs.contains(write.eval_run_id.as_str()) {
                return Err(AccessError::UnknownRun {
                    write_id: write.write_id.clone(),
                    eval_run_id: write.eval_run_id.clone(),
                });
            }
        }
        let mut recorded: BTreeMap<&str, usize> = BTreeMap::new();
        for read in &self.reads {
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
            *recorded.entry(&read.reader).or_default() += 1;
        }
        if let Some(sample) = self
            .read_counts
            .keys()
            .find(|sample| !ledger.samples.contains_key(*sample))
        {
            return Err(AccessError::UnknownReader {
                reader: sample.clone(),
            });
        }
        for sample in ledger.samples.keys() {
            let attested = self.read_counts.get(sample).copied();
            let recorded = recorded.get(sample.as_str()).copied().unwrap_or(0);
            if attested.and_then(|count| usize::try_from(count).ok()) != Some(recorded) {
                return Err(AccessError::ReadCountMismatch {
                    sample: sample.clone(),
                    attested,
                    recorded,
                });
            }
        }
        Ok(())
    }
}
