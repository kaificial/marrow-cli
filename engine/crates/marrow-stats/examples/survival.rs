//! Fits the estimator to lifetimes read from stdin, one `left right` pair per line (`right` may be
//! `inf` for a survivor), and prints survival at each time given as an argument, then the median.
//! It exists so the estimator can be checked against an independent implementation.

use std::io::Read;

use marrow_stats::{turnbull, Observation};

fn main() {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("reads stdin");
    let observations: Vec<(Observation, f64)> = input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut parts = line.split_whitespace();
            let left: f64 = parts.next().expect("a left end").parse().expect("a number");
            let right: f64 = parts
                .next()
                .expect("a right end")
                .parse()
                .expect("a number");
            let observation = if right.is_finite() {
                Observation::died(left, right)
            } else {
                Observation::alive(left)
            };
            (observation, 1.0)
        })
        .collect();
    let estimate = turnbull(&observations);
    for time in std::env::args().skip(1) {
        let t: f64 = time.parse().expect("a time");
        println!("{t} {:.12}", estimate.survival(t));
    }
    for ((start, end), mass) in estimate.support() {
        println!("support {start:.17e} {end:.17e} {mass:.17e}");
    }
    match estimate.median() {
        Some(median) => println!("median {median:.12}"),
        None => println!("median none"),
    }
}
