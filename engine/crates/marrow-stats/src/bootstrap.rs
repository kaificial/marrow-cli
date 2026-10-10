//! Percentile bootstrap confidence intervals (Efron 1981): resample with replacement, refit, and
//! read the interval off the spread of the refitted values.
//!
//! What is resampled is a cluster, not a single lifetime: lines written together live and die
//! together (a deleted function takes all its lines at once), and resampling them one by one
//! would treat those deaths as independent and make the intervals too narrow. Resampling whole
//! clusters keeps whatever dependence there is inside them (Davison and Hinkley 1997, §3.8).

use rayon::prelude::*;

use crate::turnbull::{turnbull, Estimate, Observation};

/// A two-sided confidence interval. `None` at an end means the value lies beyond the data, like a
/// median survival time the curve never falls far enough to reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interval {
    pub low: Option<f64>,
    pub high: Option<f64>,
}

/// SplitMix64 (Steele, Lea and Flood 2014): small, fast, and enough for drawing resamples. A fixed
/// seed makes a report come out the same every time it runs.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A uniform index below `n` (Lemire's multiply-shift; its bias is below 2^-40 here).
    fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next()) * n as u128) >> 64) as usize
    }
}

/// Fits `replicates` resamples of `observations` and returns, for each value `statistics` reads
/// off a fit, the central `level` interval of those values across the resamples. `clusters[i]`
/// names the cluster observation `i` belongs to; each resample draws as many clusters as there
/// are, with replacement, and takes every observation in each one it draws.
///
/// Resamples are independent, so they run in parallel. Each draws from its own stream, seeded
/// from `seed` and its own number, so the result is the same however many threads there are.
pub fn bootstrap(
    observations: &[Observation],
    clusters: &[usize],
    replicates: usize,
    level: f64,
    seed: u64,
    statistics: impl Fn(&Estimate) -> Vec<Option<f64>> + Sync,
) -> Vec<Interval> {
    if observations.is_empty() || replicates == 0 {
        return Vec::new();
    }
    assert_eq!(
        clusters.len(),
        observations.len(),
        "every observation belongs to a cluster"
    );
    // Identical lifetimes are fitted once, weighted by how often the resample drew them.
    let mut distinct: Vec<Observation> = observations.to_vec();
    distinct.sort_by(|a, b| a.left.total_cmp(&b.left).then(a.right.total_cmp(&b.right)));
    distinct.dedup();
    let index: Vec<usize> = observations
        .iter()
        .map(|observation| {
            distinct
                .binary_search_by(|probe| {
                    probe
                        .left
                        .total_cmp(&observation.left)
                        .then(probe.right.total_cmp(&observation.right))
                })
                .expect("every lifetime is among the distinct ones")
        })
        .collect();
    // Each cluster, as the distinct lifetimes in it (with repeats).
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut numbered: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (observation, &cluster) in clusters.iter().enumerate() {
        let next = members.len();
        let slot = *numbered.entry(cluster).or_insert(next);
        if slot == members.len() {
            members.push(Vec::new());
        }
        members[slot].push(index[observation]);
    }
    let samples: Vec<Vec<Option<f64>>> = (0..replicates)
        .into_par_iter()
        .map(|replicate| {
            let mut rng = Rng::new(seed ^ (replicate as u64).wrapping_mul(0xd1b5_4a32_d192_ed03));
            let mut counts = vec![0.0; distinct.len()];
            for _ in 0..members.len() {
                for &member in &members[rng.below(members.len())] {
                    counts[member] += 1.0;
                }
            }
            let weighted: Vec<(Observation, f64)> = distinct
                .iter()
                .copied()
                .zip(counts.iter().copied())
                .collect();
            statistics(&turnbull(&weighted))
        })
        .collect();
    let width = samples.first().map_or(0, Vec::len);
    (0..width)
        .map(|column| {
            // A value beyond the data sorts after every real one.
            let mut values: Vec<Option<f64>> = samples.iter().map(|row| row[column]).collect();
            values.sort_by(|a, b| match (a, b) {
                (Some(a), Some(b)) => a.total_cmp(b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });
            let tail = (1.0 - level) / 2.0;
            let low = ((tail * replicates as f64).floor() as usize).min(replicates - 1);
            let high = (((1.0 - tail) * replicates as f64).ceil() as usize)
                .saturating_sub(1)
                .min(replicates - 1);
            Interval {
                low: values[low],
                high: values[high],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{bootstrap, Rng};
    use crate::turnbull::{turnbull, Observation};

    fn lifetimes() -> Vec<Observation> {
        (0..60)
            .map(|i| {
                let t = f64::from(i % 17) + 1.0;
                if i % 3 == 0 {
                    Observation::alive(t)
                } else {
                    Observation::died(t - 1.0, t)
                }
            })
            .collect()
    }

    fn alone(count: usize) -> Vec<usize> {
        (0..count).collect()
    }

    #[test]
    fn the_same_seed_gives_the_same_intervals() {
        let read =
            |estimate: &crate::Estimate| vec![Some(estimate.survival(5.0)), estimate.median()];
        let first = bootstrap(&lifetimes(), &alone(60), 100, 0.95, 7, read);
        let again = bootstrap(&lifetimes(), &alone(60), 100, 0.95, 7, read);
        assert_eq!(first, again);
        let other = bootstrap(&lifetimes(), &alone(60), 100, 0.95, 8, read);
        assert_ne!(first, other, "a different seed draws different resamples");
    }

    #[test]
    fn the_interval_brackets_the_estimate() {
        let observations = lifetimes();
        let weighted: Vec<(Observation, f64)> = observations
            .iter()
            .map(|&observation| (observation, 1.0))
            .collect();
        let fitted = turnbull(&weighted).survival(5.0);
        let band = bootstrap(&observations, &alone(60), 200, 0.95, 1, |estimate| {
            vec![Some(estimate.survival(5.0))]
        });
        let (low, high) = (band[0].low.unwrap(), band[0].high.unwrap());
        assert!(low < fitted && fitted < high, "{low} {fitted} {high}");
        assert!(
            high - low < 0.4,
            "sixty lines narrow it down: {low}..{high}"
        );
    }

    #[test]
    fn lines_that_die_together_widen_the_interval() {
        // Ten files of twenty lines: half the files die whole at day 3, half live on.
        let mut observations = Vec::new();
        let mut files = Vec::new();
        for file in 0..10 {
            for _ in 0..20 {
                observations.push(if file % 2 == 0 {
                    Observation::died(2.0, 3.0)
                } else {
                    Observation::alive(10.0)
                });
                files.push(file);
            }
        }
        let width = |clusters: &[usize]| {
            let band = bootstrap(&observations, clusters, 400, 0.95, 5, |estimate| {
                vec![Some(estimate.survival(5.0))]
            });
            band[0].high.unwrap() - band[0].low.unwrap()
        };
        let as_lines = width(&alone(observations.len()));
        let as_files = width(&files);
        assert!(
            as_files > 2.0 * as_lines,
            "two hundred lines are really ten files: {as_lines} vs {as_files}"
        );
    }

    #[test]
    fn indexes_stay_below_the_bound() {
        let mut rng = Rng::new(3);
        assert!((0..10_000).all(|_| rng.below(7) < 7));
    }
}
