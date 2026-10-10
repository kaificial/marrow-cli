//! Turnbull's estimator (Turnbull 1976), fitted by his self-consistency algorithm, which is the
//! EM algorithm for this problem.

/// How much earlier than its right end a death known to the second is placed, when the line was
/// last seen alive at the very moment it was found dead. A death needs an interval of positive
/// width, and a millisecond is well below anything marrow's clocks can resolve.
const INSTANT: f64 = 1e-3 / 86_400.0;

/// The relative rise in log-likelihood below which the fit has converged.
const TOLERANCE: f64 = 1e-12;
const MAX_ITERATIONS: usize = 100_000;

/// One line's lifetime, measured from when it entered observation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observation {
    /// The last time the line was known to be alive.
    pub left: f64,
    /// The first time it was known to be dead, or infinity if it never was.
    pub right: f64,
}

impl Observation {
    /// Died at some time after `last_alive`, no later than `found_dead`.
    ///
    /// A line last seen alive at the very moment it was found dead died at that moment, which
    /// is placed in the instant just before it. That keeps Kaplan and Meier's convention at a
    /// tie: a line censored at the same time was alive then, so it can't share the death. At
    /// time zero the instant lies just before zero, and must not be moved to just after it,
    /// where a line censored at zero could share it.
    pub fn died(last_alive: f64, found_dead: f64) -> Observation {
        let right = found_dead.max(0.0);
        let left = last_alive.max(0.0).min(right);
        Observation {
            left: if left < right { left } else { right - INSTANT },
            right,
        }
    }

    /// Still alive when last seen: right-censored.
    pub fn alive(last_seen: f64) -> Observation {
        Observation {
            left: last_seen.max(0.0),
            right: f64::INFINITY,
        }
    }

    pub fn is_death(self) -> bool {
        self.right.is_finite()
    }
}

/// The fitted distribution of lifetimes: probability mass on each of Turnbull's innermost
/// intervals, the only places the likelihood lets a death fall.
#[derive(Clone, Debug)]
pub struct Estimate {
    /// Innermost intervals `(start, end]`, sorted and disjoint.
    intervals: Vec<(f64, f64)>,
    mass: Vec<f64>,
    pub iterations: usize,
    pub converged: bool,
}

impl Estimate {
    /// The probability of surviving past `t`.
    ///
    /// Within an innermost interval the estimator doesn't say when the mass falls, only that it
    /// falls somewhere inside. Here it is spread evenly across the interval, which is neither
    /// the optimistic reading (every death at the interval's end) nor the pessimistic one (every
    /// death at its start). Mass in an interval with no end is never lost: those are lines that
    /// outlived every observation.
    pub fn survival(&self, t: f64) -> f64 {
        let mut survival = 0.0;
        for (&(start, end), &mass) in self.intervals.iter().zip(&self.mass) {
            let beyond = if !end.is_finite() || t <= start {
                1.0
            } else if t >= end {
                0.0
            } else {
                (end - t) / (end - start)
            };
            survival += mass * beyond;
        }
        survival.clamp(0.0, 1.0)
    }

    /// The first time survival falls to one half, or None if it never does within the data.
    pub fn median(&self) -> Option<f64> {
        self.quantile(0.5)
    }

    /// The first time survival falls to `level`.
    pub fn quantile(&self, level: f64) -> Option<f64> {
        // Survival is continuous and falls linearly between breakpoints, so the crossing lies
        // on the segment where it first drops to the level.
        let mut breakpoints: Vec<f64> = self
            .intervals
            .iter()
            .flat_map(|&(start, end)| [start, end])
            .filter(|t| t.is_finite())
            .collect();
        breakpoints.sort_by(f64::total_cmp);
        breakpoints.dedup();
        let mut previous: Option<(f64, f64)> = None;
        for t in breakpoints {
            let survival = self.survival(t);
            if survival <= level {
                // A lifetime can't be negative, though a death at the moment of entry sits in
                // the instant just before zero.
                return Some(
                    match previous {
                        Some((t0, s0)) if s0 > survival => {
                            t0 + (t - t0) * (s0 - level) / (s0 - survival)
                        }
                        _ => t,
                    }
                    .max(0.0),
                );
            }
            previous = Some((t, survival));
        }
        None
    }

    /// The fitted innermost intervals and the probability mass on each, for checking the fit
    /// against another implementation.
    pub fn support(&self) -> impl Iterator<Item = ((f64, f64), f64)> + '_ {
        self.intervals
            .iter()
            .copied()
            .zip(self.mass.iter().copied())
    }

    /// The survival curve's corners, for drawing it.
    pub fn curve(&self) -> Vec<(f64, f64)> {
        let mut times: Vec<f64> = self
            .intervals
            .iter()
            .flat_map(|&(start, end)| [start, end])
            .filter(|t| t.is_finite())
            .collect();
        times.push(0.0);
        times.sort_by(f64::total_cmp);
        times.dedup();
        times.into_iter().map(|t| (t, self.survival(t))).collect()
    }
}

/// Fits the estimator to lifetimes, each with a weight (a count of identical lifetimes).
pub fn turnbull(observations: &[(Observation, f64)]) -> Estimate {
    let observations: Vec<(Observation, f64)> = observations
        .iter()
        .copied()
        .filter(|(_, weight)| *weight > 0.0)
        .collect();
    let intervals = innermost_intervals(&observations);
    if intervals.is_empty() {
        return Estimate {
            intervals,
            mass: Vec::new(),
            iterations: 0,
            converged: true,
        };
    }
    // Each lifetime can only have ended in the innermost intervals it contains, and those form a
    // contiguous run because the intervals are sorted and disjoint.
    let ranges: Vec<(usize, usize)> = observations
        .iter()
        .map(|(observation, _)| {
            let first = intervals.partition_point(|&(start, _)| start < observation.left);
            let after = intervals.partition_point(|&(_, end)| end <= observation.right);
            (first, after)
        })
        .collect();
    let total: f64 = observations.iter().map(|(_, weight)| weight).sum();
    let m = intervals.len();
    let mut mass = vec![1.0 / m as f64; m];
    let mut prefix = vec![0.0; m + 1];
    let mut spread = vec![0.0; m + 1];
    let mut iterations = 0;
    let mut converged = false;
    let mut previous = f64::NEG_INFINITY;
    while iterations < MAX_ITERATIONS {
        iterations += 1;
        for j in 0..m {
            prefix[j + 1] = prefix[j] + mass[j];
        }
        spread.iter_mut().for_each(|value| *value = 0.0);
        // Self-consistency: each lifetime shares its weight among its intervals in proportion
        // to their current mass. A difference array adds every lifetime's share to its whole
        // run at once.
        let mut log_likelihood = 0.0;
        for ((_, weight), &(first, after)) in observations.iter().zip(&ranges) {
            let covered = prefix[after] - prefix[first];
            if covered > 0.0 {
                log_likelihood += weight * covered.ln();
                spread[first] += weight / covered;
                spread[after] -= weight / covered;
            }
        }
        // Stop when the likelihood, which each step can only raise, stops rising. Stopping on
        // how far the mass moves instead would never stop where the likelihood is flat: there
        // the mass drifts towards its limit like 1/k, and a millionth of it can take a million
        // steps, while making no difference the likelihood, or a report, could see.
        if log_likelihood - previous <= TOLERANCE * log_likelihood.abs().max(1.0) {
            converged = true;
            break;
        }
        previous = log_likelihood;
        let mut running = 0.0;
        for j in 0..m {
            running += spread[j];
            mass[j] *= running / total;
        }
    }
    Estimate {
        intervals,
        mass,
        iterations,
        converged,
    }
}

/// Turnbull's innermost intervals: every `(l, r]` where a left end `l` of some lifetime is
/// followed, with no other end in between, by a right end `r`. Lifetimes are `(left, right]`, so
/// at a tie a right end comes first: `(1, 3]` and `(3, 5]` don't overlap.
fn innermost_intervals(observations: &[(Observation, f64)]) -> Vec<(f64, f64)> {
    let mut ends: Vec<(f64, bool)> = observations
        .iter()
        .flat_map(|(observation, _)| [(observation.left, false), (observation.right, true)])
        .collect();
    ends.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut intervals = Vec::new();
    let mut open: Option<f64> = None;
    for (value, is_right) in ends {
        if is_right {
            if let Some(start) = open.take() {
                intervals.push((start, value));
            }
        } else {
            open = Some(value);
        }
    }
    intervals
}

#[cfg(test)]
mod tests {
    use super::{turnbull, Observation};

    fn fit(observations: &[Observation]) -> super::Estimate {
        let weighted: Vec<(Observation, f64)> = observations
            .iter()
            .map(|&observation| (observation, 1.0))
            .collect();
        turnbull(&weighted)
    }

    /// Klein and Moeschberger (2003), §4.2: remission times in weeks for the 21 patients given
    /// 6-mercaptopurine (Freireich et al. 1963). A `+` time, here `alive`, is censored.
    fn six_mp() -> Vec<Observation> {
        let died = |t: f64| Observation::died(t, t);
        vec![
            died(6.0),
            died(6.0),
            died(6.0),
            Observation::alive(6.0),
            died(7.0),
            Observation::alive(9.0),
            died(10.0),
            Observation::alive(10.0),
            Observation::alive(11.0),
            died(13.0),
            died(16.0),
            Observation::alive(17.0),
            Observation::alive(19.0),
            Observation::alive(20.0),
            died(22.0),
            died(23.0),
            Observation::alive(25.0),
            Observation::alive(32.0),
            Observation::alive(32.0),
            Observation::alive(34.0),
            Observation::alive(35.0),
        ]
    }

    /// The product-limit estimator itself, written from Kaplan and Meier (1958), for exact
    /// deaths and right-censored times, with deaths ordered before censoring at a tie.
    fn kaplan_meier(times: &[(f64, bool)], at: f64) -> f64 {
        let mut distinct: Vec<f64> = times
            .iter()
            .filter(|(_, died)| *died)
            .map(|(t, _)| *t)
            .collect();
        distinct.sort_by(f64::total_cmp);
        distinct.dedup();
        let mut survival = 1.0;
        for t in distinct.into_iter().filter(|t| *t <= at) {
            let at_risk = times.iter().filter(|(time, _)| *time >= t).count() as f64;
            let deaths = times
                .iter()
                .filter(|(time, died)| *died && *time == t)
                .count() as f64;
            survival *= 1.0 - deaths / at_risk;
        }
        survival
    }

    #[test]
    fn with_exact_deaths_it_is_kaplan_meier_on_the_textbook_example() {
        let estimate = fit(&six_mp());
        assert!(estimate.converged);
        // The product-limit values printed in Klein and Moeschberger, Table 4.1A.
        for (week, printed) in [
            (6.0, 0.857),
            (7.0, 0.807),
            (10.0, 0.753),
            (13.0, 0.690),
            (16.0, 0.627),
            (22.0, 0.538),
            (23.0, 0.448),
        ] {
            let fitted = estimate.survival(week);
            assert!((fitted - printed).abs() < 0.0005, "week {week}: {fitted}");
        }
        // And the product-limit formula computed here, everywhere in between.
        let times: Vec<(f64, bool)> = six_mp()
            .iter()
            .map(|o| (if o.is_death() { o.right } else { o.left }, o.is_death()))
            .collect();
        for step in 0..=80 {
            let t = step as f64 * 0.5;
            let expected = kaplan_meier(&times, t);
            let fitted = estimate.survival(t);
            // Kaplan-Meier drops at the instant of death; Turnbull spreads that drop over the
            // millisecond before it, so only the instant itself can differ.
            assert!(
                (fitted - expected).abs() < 1e-6,
                "t {t}: {fitted} vs {expected}"
            );
        }
        let median = estimate.median().expect("the curve falls below one half");
        assert!(
            (median - 23.0).abs() < 1e-6,
            "the textbook median is 23 weeks: {median}"
        );
    }

    #[test]
    fn overlapping_deaths_put_all_their_mass_where_they_overlap() {
        let estimate = fit(&[Observation::died(0.0, 2.0), Observation::died(1.0, 3.0)]);
        assert_eq!(estimate.survival(1.0), 1.0);
        assert!((estimate.survival(1.5) - 0.5).abs() < 1e-9);
        assert_eq!(estimate.survival(2.0), 0.0);
        assert!((estimate.median().unwrap() - 1.5).abs() < 1e-9);
    }

    #[test]
    fn a_death_that_could_be_in_either_place_is_shared_between_them() {
        // Self-consistency with these three has the symmetric answer, half in each interval.
        let estimate = fit(&[
            Observation::died(0.0, 1.0),
            Observation::died(2.0, 3.0),
            Observation::died(0.0, 3.0),
        ]);
        assert!((estimate.survival(1.0) - 0.5).abs() < 1e-8);
        assert!((estimate.survival(2.0) - 0.5).abs() < 1e-8);
        assert!(estimate.survival(3.0).abs() < 1e-8);
    }

    #[test]
    fn nothing_dies_and_survival_stays_at_one() {
        let estimate = fit(&[Observation::alive(3.0), Observation::alive(10.0)]);
        assert_eq!(estimate.survival(0.0), 1.0);
        assert_eq!(estimate.survival(100.0), 1.0);
        assert_eq!(estimate.median(), None);
    }

    #[test]
    fn a_weight_is_the_same_as_repeating_a_lifetime() {
        let weighted = turnbull(&[
            (Observation::died(0.0, 2.0), 2.0),
            (Observation::died(1.0, 5.0), 1.0),
            (Observation::alive(4.0), 3.0),
        ]);
        let repeated = fit(&[
            Observation::died(0.0, 2.0),
            Observation::died(0.0, 2.0),
            Observation::died(1.0, 5.0),
            Observation::alive(4.0),
            Observation::alive(4.0),
            Observation::alive(4.0),
        ]);
        for step in 0..=12 {
            let t = step as f64 * 0.5;
            assert!(
                (weighted.survival(t) - repeated.survival(t)).abs() < 1e-9,
                "{t}"
            );
        }
    }

    #[test]
    fn a_line_seen_alive_the_moment_it_was_found_dead_still_dies() {
        let estimate = fit(&[Observation::died(4.0, 4.0), Observation::alive(9.0)]);
        assert!((estimate.survival(4.0) - 0.5).abs() < 1e-9);
        assert!((estimate.survival(3.9) - 1.0).abs() < 1e-9);
    }
}
