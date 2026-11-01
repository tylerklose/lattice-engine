//! Reads a compiled model as JSON on stdin and prints the generated rows as JSON.
use lattice_engine::{Model, Options, generate};
use serde::Deserialize;
use std::io::Read;
use std::time::Instant;

#[derive(Deserialize)]
struct Input {
    sizes: Vec<usize>,
    weights: Vec<Vec<f64>>,
    strength: usize,
    invalid_pairs: Vec<[usize; 4]>,
    requires: Vec<[usize; 4]>,
    higher_order: Vec<(Vec<[usize; 2]>, [usize; 2])>,
    forced: Vec<Vec<[usize; 2]>>,
    #[serde(default = "default_seed")]
    seed: u64,
    #[serde(default = "default_candidates")]
    candidates: usize,
    max_rows: Option<usize>,
}

fn default_seed() -> u64 { 42 }
fn default_candidates() -> usize { 16 }

fn main() {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).expect("read stdin");
    let input: Input = serde_json::from_str(&text).expect("valid model JSON");
    let pair = |a: [usize; 4]| ((a[0], a[1]), (a[2], a[3]));
    let model = Model {
        sizes: input.sizes,
        weights: input.weights,
        strength: input.strength,
        invalid_pairs: input.invalid_pairs.into_iter().map(pair).collect(),
        requires: input.requires.into_iter().map(pair).collect(),
        higher_order: input
            .higher_order
            .into_iter()
            .map(|(a, f)| (a.into_iter().map(|x| (x[0], x[1])).collect(), (f[0], f[1])))
            .collect(),
        forced: input.forced.into_iter().map(|f| f.into_iter().map(|x| (x[0], x[1])).collect()).collect(),
    };
    let options = Options { seed: input.seed, candidates: input.candidates, max_rows: input.max_rows };
    let started = Instant::now();
    match generate(&model, &options) {
        Ok(result) => {
            let seconds = started.elapsed().as_secs_f64();
            println!("{}", serde_json::json!({"rows": result.rows, "valid_tuples": result.valid_tuples, "seconds": seconds}));
        }
        Err(error) => {
            eprintln!("{error:?}");
            std::process::exit(1);
        }
    }
}
