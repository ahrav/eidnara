use std::collections::BTreeSet;

/// A frozen witness matrix: a fixed nonempty list of declared cells, each of which must fire exactly once before `finish`.
pub struct WitnessMatrix {
    declared: &'static [&'static str],
    fired: BTreeSet<String>,
}

impl WitnessMatrix {
    /// Panics when the declared list is empty or names a cell twice.
    pub fn new(declared: &'static [&'static str]) -> Self {
        assert!(!declared.is_empty(), "a witness matrix declares cells");
        let unique: BTreeSet<&str> = declared.iter().copied().collect();
        assert_eq!(
            unique.len(),
            declared.len(),
            "a witness matrix declares each cell once"
        );
        Self {
            declared,
            fired: BTreeSet::new(),
        }
    }

    /// Records `cell` as fired; panics when `cell` is undeclared or already fired.
    pub fn fire(&mut self, cell: &str) {
        assert!(
            self.declared.contains(&cell),
            "{cell} is not a declared cell"
        );
        assert!(self.fired.insert(cell.to_owned()), "{cell} fired twice");
    }

    /// Runs `witness`, then fires `cell`; a panicking witness leaves the cell unfired.
    pub fn run(&mut self, cell: &str, witness: impl FnOnce()) {
        witness();
        self.fire(cell);
    }

    /// Panics unless every declared cell fired.
    pub fn finish(self) {
        let unfired: Vec<&str> = self
            .declared
            .iter()
            .copied()
            .filter(|cell| !self.fired.contains(*cell))
            .collect();
        assert!(unfired.is_empty(), "unfired witness cells: {unfired:?}");
    }
}
