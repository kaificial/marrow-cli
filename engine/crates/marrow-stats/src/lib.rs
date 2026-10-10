//! Survival estimation for lines of code: when a line dies, marrow only knows it died between
//! two observations, and a line still alive is only known to have lived at least so long.
//!
//! The estimator is Turnbull's nonparametric maximum-likelihood estimator, which handles both
//! interval-censored deaths and right-censored survivors, and reduces exactly to the
//! Kaplan-Meier product-limit estimator when every death time is known. Confidence intervals come
//! from resampling lines (the bootstrap), because the estimator has no simple variance formula
//! once deaths are interval-censored.
//!
//! References:
//! - E. L. Kaplan and P. Meier (1958). Nonparametric estimation from incomplete observations.
//!   Journal of the American Statistical Association 53(282), 457-481.
//! - B. W. Turnbull (1976). The empirical distribution function with arbitrarily grouped,
//!   censored and truncated data. Journal of the Royal Statistical Society B 38(3), 290-295.
//! - B. Efron (1981). Censored data and the bootstrap. Journal of the American Statistical
//!   Association 76(374), 312-319.
//! - J. P. Klein and M. L. Moeschberger (2003). Survival Analysis: Techniques for Censored and
//!   Truncated Data, 2nd ed., Springer. §4.2, whose 6-MP example the tests check against.

mod bootstrap;
mod turnbull;

pub use bootstrap::{bootstrap, Interval, Rng};
pub use turnbull::{turnbull, Estimate, Observation};
