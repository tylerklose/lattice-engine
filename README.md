# lattice-engine

The generation engine behind [Lattice](https://github.com/tylerklose/lattice), shared by the Python package (`lattice-cli`) and the Ruby gem (`latticerb`).

Given parameters, their values and constraints, it finds a small set of rows (a covering array) in which every valid combination of `strength` values appears at least once, and no row breaks a constraint.

```rust
use lattice_engine::{Model, Options, generate};

let model = Model {
    sizes: vec![3, 3, 2, 2],                  // values per parameter
    weights: vec![vec![1.0 / 3.0; 3], vec![1.0 / 3.0; 3], vec![0.5; 2], vec![0.5; 2]],
    strength: 2,                              // every pair
    invalid_pairs: vec![((0, 2), (3, 1))],    // parameter 0 = value 2 never with parameter 3 = value 1
    requires: vec![],
    higher_order: vec![],
    forced: vec![],
};
let result = generate(&model, &Options::default()).unwrap();
assert_eq!(result.new_coverage.iter().sum::<usize>(), result.valid_tuples);
```

## How it works

1. **Build.** Rows are added greedily. Each row is the best of `candidates` attempts (default 16), and each attempt picks, value by value, whatever covers the most combinations still uncovered.
2. **Shrink.** A row is removed and the gaps are repaired with tabu search, which changes values in the remaining rows without breaking any constraint or forced interaction. This repeats while repairs succeed within `shrink_steps` steps, giving up on a removal after `patience` steps without progress.

Rows come back ordered so each adds the most new coverage, with the count each adds.

The output depends only on the model and the options: the random numbers come from SplitMix64, the search uses integer arithmetic, and the budget is a step count rather than a time limit. The same input gives the same rows on every platform.

## Constraints

Constraints are given as four primitives, which the Lattice packages compile schemas into:

- `invalid_pairs`: two assignments that may not share a row.
- `requires`: a row with the first assignment must also have the second.
- `higher_order`: when every antecedent holds, the forbidden assignment may not.
- `forced`: assignment sets that must appear together in some row.

`Engine` answers feasibility questions about a model: `feasibility()` reports the first impossible forced interaction or an unsatisfiable model, and `can_complete()` says whether a partial assignment fits in some valid row. Malformed models are returned as `Error::InvalidInput`, never a panic.

## Command line

The `lattice-engine` binary (the default `cli` feature) reads a compiled model as JSON on stdin and prints the rows as JSON. Most people want the `lattice` command from the Lattice packages instead, which reads schemas in YAML or JSON.

## License

Apache-2.0
