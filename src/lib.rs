//! Deterministic t-way covering-array generation with constraints.
//!
//! The engine works on a compiled model: value counts per parameter, weights, and
//! constraints already reduced to four primitives (invalid pairs, requirements,
//! higher-order exclusions and forced interactions). The Ruby and Python Lattice
//! packages compile schemas to this form, and their pure implementations follow
//! the same steps, so all three give byte-identical rows for the same input.

use std::collections::HashMap;

pub mod shrink;

pub type Assignment = (usize, usize);

/// A model reduced to what generation needs.
#[derive(Debug, Clone)]
pub struct Model {
    pub sizes: Vec<usize>,
    pub weights: Vec<Vec<f64>>,
    pub strength: usize,
    /// Each entry forbids two assignments in the same row (stored both ways).
    pub invalid_pairs: Vec<(Assignment, Assignment)>,
    /// `(a, b)`: a row with `a` must also contain `b`.
    pub requires: Vec<(Assignment, Assignment)>,
    /// `(antecedents, forbidden)`: if every antecedent holds, `forbidden` may not.
    pub higher_order: Vec<(Vec<Assignment>, Assignment)>,
    /// Assignment sets that must appear together in at least one row.
    pub forced: Vec<Vec<Assignment>>,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub seed: u64,
    /// Candidate rows built per greedy step; the one covering the most wins.
    pub candidates: usize,
    /// Tabu-search steps for removing rows after the greedy build (0 = off).
    pub shrink_steps: u64,
    /// A removal attempt gives up after this many steps without progress.
    pub patience: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options { seed: 42, candidates: 16, shrink_steps: 1_000_000, patience: 10_000 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Generated {
    /// Rows ordered so each adds the most new coverage.
    pub rows: Vec<Vec<usize>>,
    /// Valid tuples each row covers for the first time, in row order.
    pub new_coverage: Vec<usize>,
    /// Valid tuples in total (tuples no valid row can contain are left out).
    pub valid_tuples: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The model is malformed: the message says how.
    InvalidInput(String),
    NoValidAssignments,
    DeadEnd,
    InvalidRow(Vec<usize>),
    RowCoveredNothing,
}

/// SplitMix64: small, fully specified, and easy to reproduce in any language.
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

const NONE: usize = usize::MAX;

/// Uncovered flags for every t-way parameter combination.
struct Coverage {
    sizes: Vec<usize>,
    combos: Vec<Vec<usize>>,
    strides: Vec<Vec<usize>>,
    flags: Vec<Vec<u8>>,
    remaining: Vec<usize>,
    total: usize,
    value_counts: Vec<Vec<usize>>,
    /// For each parameter: (combo index, the other parameters in it).
    by_parameter: Vec<Vec<(usize, Vec<usize>)>>,
}

fn combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    if k > n {
        return out;
    }
    let mut current: Vec<usize> = (0..k).collect();
    loop {
        out.push(current.clone());
        let mut i = k;
        loop {
            if i == 0 {
                return out;
            }
            i -= 1;
            if current[i] != i + n - k {
                break;
            }
            if i == 0 {
                return out;
            }
        }
        current[i] += 1;
        for j in i + 1..k {
            current[j] = current[j - 1] + 1;
        }
    }
}

impl Coverage {
    fn new(sizes: &[usize], strength: usize) -> Self {
        let combos = combinations(sizes.len(), strength);
        let mut strides = Vec::with_capacity(combos.len());
        let mut flags = Vec::with_capacity(combos.len());
        let mut remaining = Vec::with_capacity(combos.len());
        let mut value_counts: Vec<Vec<usize>> = sizes.iter().map(|&n| vec![0; n]).collect();
        let mut by_parameter: Vec<Vec<(usize, Vec<usize>)>> = vec![Vec::new(); sizes.len()];
        for (i, combo) in combos.iter().enumerate() {
            let mut stride = vec![0; combo.len()];
            let mut span = 1;
            for k in (0..combo.len()).rev() {
                stride[k] = span;
                span *= sizes[combo[k]];
            }
            strides.push(stride);
            flags.push(vec![1u8; span]);
            remaining.push(span);
            for &p in combo {
                for v in 0..sizes[p] {
                    value_counts[p][v] += span / sizes[p];
                }
                by_parameter[p].push((i, combo.iter().copied().filter(|&q| q != p).collect()));
            }
        }
        let total = remaining.iter().sum();
        Coverage { sizes: sizes.to_vec(), combos, strides, flags, remaining, total, value_counts, by_parameter }
    }

    fn offset_of(&self, combo_i: usize, row: &[usize]) -> usize {
        let combo = &self.combos[combo_i];
        let stride = &self.strides[combo_i];
        let mut index = 0;
        for k in 0..combo.len() {
            index += stride[k] * row[combo[k]];
        }
        index
    }

    fn values_at(&self, combo_i: usize, mut index: usize) -> Vec<usize> {
        let combo = &self.combos[combo_i];
        let mut values = vec![0; combo.len()];
        for k in (0..combo.len()).rev() {
            let n = self.sizes[combo[k]];
            values[k] = index % n;
            index /= n;
        }
        values
    }

    fn mark(&mut self, combo_i: usize, index: usize) -> bool {
        if self.flags[combo_i][index] == 0 {
            return false;
        }
        self.flags[combo_i][index] = 0;
        self.remaining[combo_i] -= 1;
        let values = self.values_at(combo_i, index);
        for (k, &p) in self.combos[combo_i].iter().enumerate() {
            self.value_counts[p][values[k]] -= 1;
        }
        true
    }

    fn gain(&self, row: &[usize]) -> usize {
        (0..self.combos.len()).map(|i| self.flags[i][self.offset_of(i, row)] as usize).sum()
    }

    fn cover_row(&mut self, row: &[usize]) -> usize {
        let mut newly = 0;
        for i in 0..self.combos.len() {
            let index = self.offset_of(i, row);
            if self.mark(i, index) {
                newly += 1;
            }
        }
        newly
    }

    /// The combination with the most uncovered tuples (lowest index on ties).
    fn best_combo(&self) -> Option<usize> {
        let mut best: Option<usize> = None;
        for i in 0..self.combos.len() {
            match best {
                None => best = Some(i),
                Some(b) if self.remaining[i] > self.remaining[b] => best = Some(i),
                _ => {}
            }
        }
        best.filter(|&b| self.remaining[b] > 0)
    }

    fn tuple_at(&self, combo_i: usize, index: usize) -> Vec<Assignment> {
        self.combos[combo_i].iter().copied().zip(self.values_at(combo_i, index)).collect()
    }

    fn uncovered_in(&self, combo_i: usize) -> Vec<Vec<Assignment>> {
        self.flags[combo_i]
            .iter()
            .enumerate()
            .filter(|(_, f)| **f != 0)
            .map(|(index, _)| self.tuple_at(combo_i, index))
            .collect()
    }
}

/// Compiled constraints with incremental checks.
struct Constraints {
    invalid: HashMap<Assignment, Vec<Assignment>>,
    requires: HashMap<Assignment, Vec<Assignment>>,
    higher: Vec<(Vec<Assignment>, Assignment)>,
    touched: Vec<usize>,
    is_touched: Vec<bool>,
    sizes: Vec<usize>,
    cache: HashMap<Vec<usize>, bool>,
}

impl Constraints {
    fn new(model: &Model) -> Self {
        let mut invalid: HashMap<Assignment, Vec<Assignment>> = HashMap::new();
        let mut requires: HashMap<Assignment, Vec<Assignment>> = HashMap::new();
        let mut is_touched = vec![false; model.sizes.len()];
        for &(a, b) in &model.invalid_pairs {
            invalid.entry(a).or_default().push(b);
            invalid.entry(b).or_default().push(a);
            is_touched[a.0] = true;
            is_touched[b.0] = true;
        }
        for &(a, b) in &model.requires {
            requires.entry(a).or_default().push(b);
            is_touched[a.0] = true;
            is_touched[b.0] = true;
        }
        for (antecedents, forbidden) in &model.higher_order {
            for &(p, _) in antecedents {
                is_touched[p] = true;
            }
            is_touched[forbidden.0] = true;
        }
        let touched = (0..model.sizes.len()).filter(|&p| is_touched[p]).collect();
        Constraints {
            invalid,
            requires,
            higher: model.higher_order.clone(),
            touched,
            is_touched,
            sizes: model.sizes.clone(),
            cache: HashMap::new(),
        }
    }

    fn any(&self) -> bool {
        !self.touched.is_empty()
    }

    /// The assignments (p, v) adds to `row`, requirements included, or None.
    fn extend(&self, row: &[usize], p: usize, v: usize) -> Option<Vec<Assignment>> {
        if row[p] != NONE {
            return if row[p] == v { Some(Vec::new()) } else { None };
        }
        if !self.any() {
            return Some(vec![(p, v)]);
        }
        let mut added: Vec<Assignment> = Vec::new();
        let mut queue = vec![(p, v)];
        while let Some((q, w)) = queue.pop() {
            let current = if row[q] != NONE { Some(row[q]) } else { added.iter().find(|a| a.0 == q).map(|a| a.1) };
            if let Some(c) = current {
                if c != w {
                    return None;
                }
                continue;
            }
            if let Some(blocked) = self.invalid.get(&(q, w)) {
                for &(r, x) in blocked {
                    if row[r] == x || added.iter().any(|&a| a == (r, x)) {
                        return None;
                    }
                }
            }
            added.push((q, w));
            if let Some(next) = self.requires.get(&(q, w)) {
                queue.extend(next.iter().copied());
            }
        }
        if !self.higher.is_empty() {
            let value = |r: usize| if row[r] != NONE { Some(row[r]) } else { added.iter().find(|a| a.0 == r).map(|a| a.1) };
            for (antecedents, forbidden) in &self.higher {
                if value(forbidden.0) == Some(forbidden.1) && antecedents.iter().all(|&(a, b)| value(a) == Some(b)) {
                    return None;
                }
            }
        }
        Some(added)
    }

    /// Whether every touched parameter can still be given a value.
    fn completable(&mut self, row: &[usize]) -> bool {
        if !self.any() {
            return true;
        }
        let key: Vec<usize> = self.touched.iter().map(|&p| row[p]).collect();
        if let Some(&cached) = self.cache.get(&key) {
            return cached;
        }
        let mut partial = vec![NONE; row.len()];
        for &p in &self.touched {
            partial[p] = row[p];
        }
        let result = self.search(&mut partial);
        self.cache.insert(key, result);
        result
    }

    fn search(&self, partial: &mut Vec<usize>) -> bool {
        let Some(&p) = self.touched.iter().find(|&&p| partial[p] == NONE) else {
            return self.row_valid(partial);
        };
        for v in 0..self.sizes[p] {
            if let Some(added) = self.extend(partial, p, v) {
                for &(q, w) in &added {
                    partial[q] = w;
                }
                let ok = self.search(partial);
                for &(q, _) in &added {
                    partial[q] = NONE;
                }
                if ok {
                    return true;
                }
            }
        }
        false
    }

    /// Checks a row whose touched parameters are all set.
    fn row_valid(&self, row: &[usize]) -> bool {
        for &p in &self.touched {
            let v = row[p];
            if let Some(blocked) = self.invalid.get(&(p, v)) {
                if blocked.iter().any(|&(r, x)| row[r] == x) {
                    return false;
                }
            }
            if let Some(required) = self.requires.get(&(p, v)) {
                if required.iter().any(|&(r, x)| row[r] != x) {
                    return false;
                }
            }
        }
        for (antecedents, forbidden) in &self.higher {
            if row[forbidden.0] == forbidden.1 && antecedents.iter().all(|&(a, b)| row[a] == b) {
                return false;
            }
        }
        true
    }

    fn can_complete(&mut self, assignments: &[Assignment]) -> bool {
        let mut row = vec![NONE; self.sizes.len()];
        for &(p, v) in assignments {
            match self.extend(&row, p, v) {
                Some(added) => {
                    for (q, w) in added {
                        row[q] = w;
                    }
                }
                None => return false,
            }
        }
        self.completable(&row)
    }
}

fn build_row(
    start: &[Assignment],
    attempt: usize,
    rng: &mut SplitMix64,
    model: &Model,
    constraints: &mut Constraints,
    coverage: &Coverage,
) -> Option<Vec<usize>> {
    let n = model.sizes.len();
    let mut row = vec![NONE; n];
    for &(p, v) in start {
        let added = constraints.extend(&row, p, v)?;
        for (q, w) in added {
            row[q] = w;
        }
    }
    let mut open: Vec<usize> = (0..n).filter(|&p| row[p] == NONE).collect();
    let totals: Vec<usize> = coverage.value_counts.iter().map(|c| c.iter().sum()).collect();
    open.sort_by(|&a, &b| totals[b].cmp(&totals[a]).then(a.cmp(&b)));
    if attempt > 0 {
        for i in (1..open.len()).rev() {
            let j = (rng.next() % (i as u64 + 1)) as usize;
            open.swap(i, j);
        }
    }
    let mut trial = vec![NONE; n];
    for &p in &open {
        if row[p] != NONE {
            continue;
        }
        // key: (gain, value count, weight, -value); strictly greater wins.
        let mut best: Option<((usize, usize, f64, usize), Vec<Assignment>)> = None;
        for v in 0..model.sizes[p] {
            let Some(added) = constraints.extend(&row, p, v) else { continue };
            if constraints.is_touched[p] {
                trial.copy_from_slice(&row);
                for &(q, w) in &added {
                    trial[q] = w;
                }
                if !constraints.completable(&trial) {
                    continue;
                }
            }
            let mut gain = 0usize;
            for (combo_i, others) in &coverage.by_parameter[p] {
                if others.iter().any(|&q| row[q] == NONE) {
                    continue;
                }
                let combo = &coverage.combos[*combo_i];
                let stride = &coverage.strides[*combo_i];
                let mut index = 0;
                for k in 0..combo.len() {
                    let value = if combo[k] == p { v } else { row[combo[k]] };
                    index += stride[k] * value;
                }
                gain += coverage.flags[*combo_i][index] as usize;
            }
            let key = (gain, coverage.value_counts[p][v], model.weights[p][v], v);
            let better = match &best {
                None => true,
                Some((b, _)) => {
                    (key.0, key.1).cmp(&(b.0, b.1)).then(key.2.partial_cmp(&b.2).unwrap()).then(b.3.cmp(&key.3))
                        == std::cmp::Ordering::Greater
                }
            };
            if better {
                best = Some((key, added));
            }
        }
        let (_, added) = best?;
        for (q, w) in added {
            row[q] = w;
        }
    }
    Some(row)
}

fn same_assignments(a: &[Assignment], b: &[Assignment]) -> bool {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    a.sort();
    b.sort();
    a == b
}

/// Rejects malformed models before any indexing can go out of range.
fn check(model: &Model) -> Result<(), Error> {
    let bad = |message: String| Err(Error::InvalidInput(message));
    let n = model.sizes.len();
    if model.strength == 0 || model.strength > n {
        return bad(format!("strength {} must be between 1 and the parameter count {n}", model.strength));
    }
    if let Some(p) = model.sizes.iter().position(|&size| size == 0) {
        return bad(format!("parameter {p} has no values"));
    }
    if model.weights.len() != n {
        return bad(format!("{} weight lists for {n} parameters", model.weights.len()));
    }
    for (p, weights) in model.weights.iter().enumerate() {
        if weights.len() != model.sizes[p] {
            return bad(format!("parameter {p} has {} values but {} weights", model.sizes[p], weights.len()));
        }
        if weights.iter().any(|w| !w.is_finite()) {
            return bad(format!("parameter {p} has a weight that is not a finite number"));
        }
    }
    let in_range = |&(p, v): &Assignment| p < n && v < model.sizes[p];
    let pairs = model.invalid_pairs.iter().chain(&model.requires);
    let singles = model.higher_order.iter().flat_map(|(a, f)| a.iter().chain(std::iter::once(f)));
    let forced = model.forced.iter().flatten();
    for assignment in pairs.flat_map(|(a, b)| [a, b]).chain(singles).chain(forced) {
        if !in_range(assignment) {
            return bad(format!("assignment {assignment:?} is out of range"));
        }
    }
    Ok(())
}

pub fn generate(model: &Model, options: &Options) -> Result<Generated, Error> {
    check(model)?;
    let mut constraints = Constraints::new(model);
    if !constraints.can_complete(&[]) {
        return Err(Error::NoValidAssignments);
    }
    let mut coverage = Coverage::new(&model.sizes, model.strength);

    let mut infeasible = 0;
    if constraints.any() {
        for i in 0..coverage.combos.len() {
            if !coverage.combos[i].iter().any(|&p| constraints.is_touched[p]) {
                continue;
            }
            for index in 0..coverage.flags[i].len() {
                let tuple = coverage.tuple_at(i, index);
                if !constraints.can_complete(&tuple) {
                    coverage.mark(i, index);
                    infeasible += 1;
                }
            }
        }
    }
    let valid_tuples = coverage.total - infeasible;
    let need: Vec<Vec<bool>> = coverage.flags.iter().map(|f| f.iter().map(|&x| x != 0).collect()).collect();
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut rng = SplitMix64::new(options.seed);
    let mut seeds: std::collections::VecDeque<Vec<Assignment>> = model.forced.iter().cloned().collect();

    loop {
        let (start, starts) = if let Some(forced) = seeds.pop_front() {
            (forced.clone(), vec![forced])
        } else {
            let Some(best) = coverage.best_combo() else { break };
            let starts = coverage.uncovered_in(best);
            (starts[0].clone(), starts)
        };
        let forced_start = model.forced.iter().any(|f| same_assignments(f, &start));
        let starts = if forced_start { vec![start.clone()] } else { starts };

        let mut best_row: Option<(usize, Vec<usize>)> = None;
        for attempt in 0..options.candidates {
            let begin = if attempt % 2 == 0 || starts.len() == 1 {
                &start
            } else {
                &starts[(rng.next() % starts.len() as u64) as usize]
            };
            let Some(row) = build_row(begin, attempt, &mut rng, model, &mut constraints, &coverage) else { continue };
            let gain = coverage.gain(&row);
            if best_row.as_ref().is_none_or(|(g, _)| gain > *g) {
                best_row = Some((gain, row));
            }
        }
        let (_, row) = best_row.ok_or(Error::DeadEnd)?;
        if !constraints.row_valid(&row) {
            return Err(Error::InvalidRow(row));
        }
        let newly = coverage.cover_row(&row);
        if newly == 0 && seeds.is_empty() {
            return Err(Error::RowCoveredNothing);
        }
        rows.push(row);
    }
    let rows = prune(rows, &coverage);
    let space = shrink::Space::new(&model.sizes, &coverage.combos, &coverage.strides, need);
    let rows = if options.shrink_steps > 0 {
        let valid = |row: &[usize]| constraints.row_valid(row);
        shrink::shrink(&space, rows, &model.forced, options.shrink_steps, options.patience, &mut rng, &valid)
    } else {
        rows
    };
    let (rows, new_coverage) = shrink::order_by_coverage(&space, rows);
    Ok(Generated { rows, new_coverage, valid_tuples })
}

/// Drops rows whose every tuple another row also covers, last row first.
fn prune(rows: Vec<Vec<usize>>, coverage: &Coverage) -> Vec<Vec<usize>> {
    let mut counts: HashMap<(usize, usize), usize> = HashMap::new();
    let keys: Vec<Vec<(usize, usize)>> = rows
        .iter()
        .map(|row| (0..coverage.combos.len()).map(|i| (i, coverage.offset_of(i, row))).collect())
        .collect();
    for row_keys in &keys {
        for key in row_keys {
            *counts.entry(*key).or_insert(0) += 1;
        }
    }
    let mut keep = vec![true; rows.len()];
    for r in (0..rows.len()).rev() {
        if keys[r].iter().all(|key| counts[key] > 1) {
            keep[r] = false;
            for key in &keys[r] {
                *counts.get_mut(key).unwrap() -= 1;
            }
        }
    }
    rows.into_iter().zip(keep).filter(|(_, k)| *k).map(|(row, _)| row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_matches_reference_values() {
        // Reference values for SplitMix64 seeded with 0 (Vigna's published sequence).
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn combinations_in_lexicographic_order() {
        assert_eq!(combinations(4, 2), vec![vec![0, 1], vec![0, 2], vec![0, 3], vec![1, 2], vec![1, 3], vec![2, 3]]);
        assert_eq!(combinations(3, 3), vec![vec![0, 1, 2]]);
    }
}
