//! Shrinks a covering array: remove a row, then repair the gaps with tabu search,
//! and repeat while repairs succeed within a fixed step budget.
//!
//! Each step takes an uncovered tuple and, over every row, considers rewriting that
//! row's values for the tuple's parameters so it covers the tuple. The move that
//! leaves the fewest tuples uncovered wins (lowest row on ties), except that a
//! recently changed (row, parameter) may not change again for `TABU_TENURE` steps
//! unless the move beats the best state seen. One step in `WALK_ONE_IN` instead
//! takes the first valid move from a random row. Only integer arithmetic is used,
//! so other languages reproduce it exactly.

use crate::{Assignment, SplitMix64};

const NONE: usize = usize::MAX;
pub const TABU_TENURE: u64 = 4;
pub const WALK_ONE_IN: u64 = 10;

pub struct Space<'a> {
    pub sizes: &'a [usize],
    pub combos: &'a [Vec<usize>],
    pub strides: &'a [Vec<usize>],
    /// Combos containing each parameter.
    pub combos_of: Vec<Vec<usize>>,
    /// Which tuples must be covered (valid ones).
    pub need: Vec<Vec<bool>>,
}

impl<'a> Space<'a> {
    pub fn new(sizes: &'a [usize], combos: &'a [Vec<usize>], strides: &'a [Vec<usize>], need: Vec<Vec<bool>>) -> Self {
        let mut combos_of = vec![Vec::new(); sizes.len()];
        for (i, combo) in combos.iter().enumerate() {
            for &p in combo {
                combos_of[p].push(i);
            }
        }
        Space { sizes, combos, strides, combos_of, need }
    }

    fn offset(&self, c: usize, row: &[usize]) -> usize {
        let combo = &self.combos[c];
        let stride = &self.strides[c];
        let mut index = 0;
        for k in 0..combo.len() {
            index += stride[k] * row[combo[k]];
        }
        index
    }

    fn tuple(&self, c: usize, mut index: usize) -> Vec<Assignment> {
        let combo = &self.combos[c];
        let mut values = vec![0; combo.len()];
        for k in (0..combo.len()).rev() {
            let n = self.sizes[combo[k]];
            values[k] = index % n;
            index /= n;
        }
        combo.iter().copied().zip(values).collect()
    }
}

struct State<'a, 'b> {
    space: &'b Space<'a>,
    rows: Vec<Vec<usize>>,
    counts: Vec<Vec<u32>>,
    uncovered: Vec<(usize, usize)>,
    position: Vec<Vec<usize>>,
    last_change: Vec<Vec<u64>>,
    forced: &'b [Vec<Assignment>],
    forced_count: Vec<u32>,
    stamp: Vec<u64>,
    stamp_now: u64,
}

impl<'a, 'b> State<'a, 'b> {
    fn new(space: &'b Space<'a>, rows: Vec<Vec<usize>>, forced: &'b [Vec<Assignment>]) -> Self {
        let counts = space.need.iter().map(|n| vec![0u32; n.len()]).collect();
        let position = space.need.iter().map(|n| vec![NONE; n.len()]).collect();
        let mut state = State {
            space,
            rows: Vec::new(),
            counts,
            uncovered: Vec::new(),
            position,
            last_change: Vec::new(),
            forced,
            forced_count: vec![0; forced.len()],
            stamp: vec![0; space.combos.len()],
            stamp_now: 0,
        };
        for c in 0..space.combos.len() {
            for index in 0..space.need[c].len() {
                if space.need[c][index] {
                    state.add_uncovered(c, index);
                }
            }
        }
        for row in rows {
            state.push_row(row);
        }
        state
    }

    fn add_uncovered(&mut self, c: usize, index: usize) {
        self.position[c][index] = self.uncovered.len();
        self.uncovered.push((c, index));
    }

    fn remove_uncovered(&mut self, c: usize, index: usize) {
        let at = self.position[c][index];
        let last = self.uncovered.pop().unwrap();
        if at < self.uncovered.len() {
            self.uncovered[at] = last;
            self.position[last.0][last.1] = at;
        }
        self.position[c][index] = NONE;
    }

    fn count(&mut self, c: usize, index: usize, up: bool) {
        if !self.space.need[c][index] {
            return;
        }
        if up {
            self.counts[c][index] += 1;
            if self.counts[c][index] == 1 {
                self.remove_uncovered(c, index);
            }
        } else {
            self.counts[c][index] -= 1;
            if self.counts[c][index] == 0 {
                self.add_uncovered(c, index);
            }
        }
    }

    fn holds(row: &[usize], set: &[Assignment]) -> bool {
        set.iter().all(|&(p, v)| row[p] == v)
    }

    fn push_row(&mut self, row: Vec<usize>) {
        for c in 0..self.space.combos.len() {
            let index = self.space.offset(c, &row);
            self.count(c, index, true);
        }
        for f in 0..self.forced.len() {
            if Self::holds(&row, &self.forced[f]) {
                self.forced_count[f] += 1;
            }
        }
        self.last_change.push(vec![0; self.space.sizes.len()]);
        self.rows.push(row);
    }

    fn remove_row(&mut self, r: usize) {
        let row = self.rows.remove(r);
        self.last_change.remove(r);
        for c in 0..self.space.combos.len() {
            let index = self.space.offset(c, &row);
            self.count(c, index, false);
        }
        for f in 0..self.forced.len() {
            if Self::holds(&row, &self.forced[f]) {
                self.forced_count[f] -= 1;
            }
        }
    }

    /// Tuples only this row covers.
    fn unique(&self, r: usize) -> usize {
        let row = &self.rows[r];
        (0..self.space.combos.len())
            .filter(|&c| {
                let index = self.space.offset(c, row);
                self.space.need[c][index] && self.counts[c][index] == 1
            })
            .count()
    }

    fn removable(&self, r: usize) -> bool {
        (0..self.forced.len()).all(|f| self.forced_count[f] > 1 || !Self::holds(&self.rows[r], &self.forced[f]))
    }

    /// Net change in covered tuples if row `r` became `new`, which differs at `changed`.
    fn delta(&mut self, r: usize, new: &[usize], changed: &[usize]) -> i64 {
        self.stamp_now += 1;
        let mut delta = 0i64;
        for &p in changed {
            for &c in &self.space.combos_of[p] {
                if self.stamp[c] == self.stamp_now {
                    continue;
                }
                self.stamp[c] = self.stamp_now;
                let old = self.space.offset(c, &self.rows[r]);
                let now = self.space.offset(c, new);
                if self.space.need[c][old] && self.counts[c][old] == 1 {
                    delta -= 1;
                }
                if self.space.need[c][now] && self.counts[c][now] == 0 {
                    delta += 1;
                }
            }
        }
        delta
    }

    fn apply(&mut self, r: usize, new: Vec<usize>, changed: &[usize], step: u64) {
        self.stamp_now += 1;
        for &p in changed {
            for i in 0..self.space.combos_of[p].len() {
                let c = self.space.combos_of[p][i];
                if self.stamp[c] == self.stamp_now {
                    continue;
                }
                self.stamp[c] = self.stamp_now;
                let old = self.space.offset(c, &self.rows[r]);
                let now = self.space.offset(c, &new);
                self.count(c, now, true);
                self.count(c, old, false);
            }
            self.last_change[r][p] = step;
        }
        for f in 0..self.forced.len() {
            let before = Self::holds(&self.rows[r], &self.forced[f]);
            let after = Self::holds(&new, &self.forced[f]);
            if before && !after {
                self.forced_count[f] -= 1;
            } else if after && !before {
                self.forced_count[f] += 1;
            }
        }
        self.rows[r] = new;
    }

    /// Row `r` rewritten to cover `tuple`: (new row, changed parameters), if allowed.
    fn candidate(&self, r: usize, tuple: &[Assignment], valid: &dyn Fn(&[usize]) -> bool) -> Option<(Vec<usize>, Vec<usize>)> {
        let mut new = self.rows[r].clone();
        let mut changed = Vec::new();
        for &(p, v) in tuple {
            if new[p] != v {
                new[p] = v;
                changed.push(p);
            }
        }
        if changed.is_empty() || !valid(&new) {
            return None;
        }
        for f in 0..self.forced.len() {
            if self.forced_count[f] == 1 && Self::holds(&self.rows[r], &self.forced[f]) && !Self::holds(&new, &self.forced[f]) {
                return None;
            }
        }
        Some((new, changed))
    }
}

/// Removes rows while the tabu search can re-cover everything within `budget` steps.
pub fn shrink(
    space: &Space,
    rows: Vec<Vec<usize>>,
    forced: &[Vec<Assignment>],
    budget: u64,
    patience: u64,
    rng: &mut SplitMix64,
    valid: &dyn Fn(&[usize]) -> bool,
) -> Vec<Vec<usize>> {
    let mut state = State::new(space, rows.clone(), forced);
    let mut best = rows;
    let mut step: u64 = 0;
    while step < budget && state.rows.len() > 1 {
        let Some(r) = (0..state.rows.len()).filter(|&r| state.removable(r)).min_by_key(|&r| (state.unique(r), r)) else {
            break;
        };
        state.remove_row(r);
        let mut fewest = state.uncovered.len();
        let mut last_progress = step;
        while !state.uncovered.is_empty() && step < budget && step - last_progress < patience {
            step += 1;
            let pick = state.uncovered[(rng.next() % state.uncovered.len() as u64) as usize];
            let tuple = space.tuple(pick.0, pick.1);
            let n = state.rows.len();
            if rng.next() % WALK_ONE_IN == 0 {
                let from = (rng.next() % n as u64) as usize;
                for k in 0..n {
                    let r = (from + k) % n;
                    if let Some((new, changed)) = state.candidate(r, &tuple, valid) {
                        state.apply(r, new, &changed, step);
                        break;
                    }
                }
            } else {
                let mut chosen: Option<(i64, usize, Vec<usize>, Vec<usize>)> = None;
                for r in 0..n {
                    let Some((new, changed)) = state.candidate(r, &tuple, valid) else { continue };
                    let delta = state.delta(r, &new, &changed);
                    let tabu = changed.iter().any(|&p| state.last_change[r][p] != 0 && state.last_change[r][p] + TABU_TENURE > step);
                    let after = state.uncovered.len() as i64 - delta;
                    if tabu && after >= fewest as i64 {
                        continue;
                    }
                    if chosen.as_ref().is_none_or(|(d, ..)| delta > *d) {
                        chosen = Some((delta, r, new, changed));
                    }
                }
                if let Some((_, r, new, changed)) = chosen {
                    state.apply(r, new, &changed, step);
                }
            }
            if state.uncovered.len() < fewest {
                fewest = state.uncovered.len();
                last_progress = step;
            }
        }
        if state.uncovered.is_empty() {
            best = state.rows.clone();
        } else {
            break;
        }
    }
    best
}

/// Orders rows so each one adds the most new coverage (lowest index on ties).
/// Returns the rows and how many tuples each covers for the first time.
pub fn order_by_coverage(space: &Space, rows: Vec<Vec<usize>>) -> (Vec<Vec<usize>>, Vec<usize>) {
    let mut covered: Vec<Vec<bool>> = space.need.iter().map(|n| vec![false; n.len()]).collect();
    let mut left = rows;
    let mut out = Vec::with_capacity(left.len());
    let mut gains = Vec::with_capacity(left.len());
    while !left.is_empty() {
        let mut best = (0usize, 0usize);
        for (i, row) in left.iter().enumerate() {
            let gain = (0..space.combos.len())
                .filter(|&c| {
                    let index = space.offset(c, row);
                    space.need[c][index] && !covered[c][index]
                })
                .count();
            if i == 0 || gain > best.0 {
                best = (gain, i);
            }
        }
        let row = left.remove(best.1);
        for c in 0..space.combos.len() {
            let index = space.offset(c, &row);
            covered[c][index] = true;
        }
        out.push(row);
        gains.push(best.0);
    }
    (out, gains)
}
