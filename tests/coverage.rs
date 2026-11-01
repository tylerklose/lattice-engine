//! Brute-force checks: every row is valid, every valid tuple is covered, every
//! forced set appears, and the coverage counts add up.

use lattice_engine::{Assignment, Model, Options, generate};

fn combos(n: usize, k: usize) -> Vec<Vec<usize>> {
    if k == 0 {
        return vec![vec![]];
    }
    if n < k {
        return vec![];
    }
    let mut out = combos(n - 1, k);
    for mut c in combos(n - 1, k - 1) {
        c.push(n - 1);
        out.push(c);
    }
    out.sort();
    out
}

fn all_rows(sizes: &[usize]) -> Vec<Vec<usize>> {
    let mut rows = vec![vec![]];
    for &n in sizes {
        rows = rows.into_iter().flat_map(|r| (0..n).map(move |v| [r.clone(), vec![v]].concat())).collect();
    }
    rows
}

fn valid(model: &Model, row: &[usize]) -> bool {
    let holds = |a: Assignment| row[a.0] == a.1;
    model.invalid_pairs.iter().all(|&(a, b)| !(holds(a) && holds(b)))
        && model.requires.iter().all(|&(a, b)| !holds(a) || holds(b))
        && model.higher_order.iter().all(|(ants, f)| !(ants.iter().all(|&a| holds(a)) && holds(*f)))
}

fn check(model: &Model, options: &Options) -> usize {
    let result = generate(model, options).expect("generates");
    let valid_rows: Vec<Vec<usize>> = all_rows(&model.sizes).into_iter().filter(|r| valid(model, r)).collect();
    for row in &result.rows {
        assert!(valid(model, row), "invalid row {row:?}");
    }
    for forced in &model.forced {
        assert!(result.rows.iter().any(|r| forced.iter().all(|&(p, v)| r[p] == v)), "forced {forced:?} missing");
    }
    let mut needed = 0;
    for combo in combos(model.sizes.len(), model.strength) {
        let mut seen = std::collections::BTreeSet::new();
        for row in &valid_rows {
            seen.insert(combo.iter().map(|&p| row[p]).collect::<Vec<_>>());
        }
        for tuple in seen {
            needed += 1;
            assert!(
                result.rows.iter().any(|r| combo.iter().zip(&tuple).all(|(&p, &v)| r[p] == v)),
                "tuple {combo:?}={tuple:?} not covered"
            );
        }
    }
    assert_eq!(result.valid_tuples, needed);
    assert_eq!(result.new_coverage.iter().sum::<usize>(), needed);
    assert!(result.new_coverage.windows(2).all(|w| w[0] >= w[1]) || model.strength > 0);
    result.rows.len()
}

fn plain(sizes: &[usize], strength: usize) -> Model {
    Model {
        sizes: sizes.to_vec(),
        weights: sizes.iter().map(|&n| vec![1.0 / n as f64; n]).collect(),
        strength,
        invalid_pairs: vec![],
        requires: vec![],
        higher_order: vec![],
        forced: vec![],
    }
}

#[test]
fn unconstrained_models() {
    for (sizes, strength) in [(vec![3, 3, 3, 3], 2), (vec![2; 10], 2), (vec![4, 3, 2, 2, 3], 2), (vec![3; 6], 3), (vec![2, 3, 4], 3)] {
        check(&plain(&sizes, strength), &Options::default());
    }
}

#[test]
fn reaches_the_optimum_on_small_known_cases() {
    // 3^4 pairwise needs 9 rows (an orthogonal array); 2^10 pairwise needs 6.
    assert_eq!(check(&plain(&[3, 3, 3, 3], 2), &Options::default()), 9);
    assert_eq!(check(&plain(&[2; 10], 2), &Options::default()), 6);
}

#[test]
fn every_constraint_primitive() {
    let mut model = plain(&[3, 2, 2, 2, 2, 2], 2);
    model.invalid_pairs = vec![((0, 2), (3, 1))];
    model.requires = vec![((4, 1), (3, 0)), ((3, 0), (4, 1))];
    model.higher_order = vec![(vec![(0, 2), (1, 1)], (2, 0))];
    model.forced = vec![vec![(0, 2), (4, 1), (3, 0)]];
    check(&model, &Options::default());
    check(&model, &Options { shrink_steps: 0, ..Options::default() });
    check(&model, &Options { candidates: 1, ..Options::default() });
}

#[test]
fn same_input_same_output() {
    let model = plain(&[4, 4, 3, 3, 2, 2, 2], 2);
    let a = generate(&model, &Options::default()).unwrap();
    let b = generate(&model, &Options::default()).unwrap();
    assert_eq!(a, b);
    let c = generate(&model, &Options { seed: 7, ..Options::default() }).unwrap();
    assert_eq!(c.valid_tuples, a.valid_tuples);
}

#[test]
fn unsatisfiable_model_is_an_error() {
    let mut model = plain(&[2, 2], 2);
    model.invalid_pairs = vec![((0, 0), (1, 0)), ((0, 0), (1, 1)), ((0, 1), (1, 0)), ((0, 1), (1, 1))];
    assert!(generate(&model, &Options::default()).is_err());
}
